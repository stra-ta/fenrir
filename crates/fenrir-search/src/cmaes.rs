use crate::{Evaluation, Optimizer};
use fenrir_core::{SampledGenome, SearchSpace};
use nalgebra::{DMatrix, DVector};
use rand::thread_rng;
use rand_distr::{Distribution, Normal};

/// Maximum number of generations without improvement before a BIPOP restart.
const MAX_STAGNATION: u64 = 50;
/// Step-size divisor used for the "small" population restart branch of BIPOP.
const RESTART_LO_INV: f64 = 8.0;

/// Internal CMA-ES state for a single run (a generation is a population of `lambda`
/// candidates sampled from the current distribution).
struct CmaState {
    n: usize,
    lambda: usize,
    mu: usize,
    weights: Vec<f64>,
    mueff: f64,
    cc: f64,
    cs: f64,
    c1: f64,
    cmu: f64,
    damps: f64,
    chi_n: f64,
    mean: DVector<f64>,
    sigma: f64,
    c: DMatrix<f64>,
    pc: DVector<f64>,
    ps: DVector<f64>,
    gen_count: u64,
}

pub struct CmaEsOptimizer {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,

    /// The (possibly restarted) CMA-ES state.
    state: Option<CmaState>,

    /// Evaluations buffered since the last generation update.
    buffer: Vec<(DVector<f64>, f64)>,

    /// Center of the search space, used as the mean for fresh restarts.
    center: DVector<f64>,
    /// Initial step size derived from the search-space ranges.
    init_sigma: f64,
    /// Base population size `4 + floor(3 * ln(n))`.
    base_lambda: usize,

    /// Best score seen across all generations (for stagnation tracking).
    best_score: Option<f64>,
    stagnation: u64,
    /// On restart, alternate between a large and a small population branch.
    restart_large: bool,
}

impl CmaEsOptimizer {
    pub fn new(space: SearchSpace, budget: usize) -> Self {
        let n = space.dimension();
        let center = DVector::from_fn(n, |i, _| {
            let (lo, hi) = space.bounds[i];
            (lo + hi) / 2.0
        });
        let init_sigma = 0.3;
        let base_lambda = if n == 0 {
            4
        } else {
            4 + (3.0 * (n as f64).ln()) as usize
        };

        Self {
            space,
            budget,
            evaluations: 0,
            state: None,
            buffer: Vec::new(),
            center,
            init_sigma,
            base_lambda,
            best_score: None,
            stagnation: 0,
            restart_large: false,
        }
    }

    /// Build a fresh CMA-ES state centred at `mean` with the given step size
    /// and population size. Weights and the standard CMA-ES parameters are
    /// derived from `lambda`.
    fn make_state(&self, mean: &DVector<f64>, sigma: f64, lambda: usize) -> CmaState {
        let n = mean.len();
        let mu = (lambda / 2).max(1);
        let raw: Vec<f64> = (0..mu)
            .map(|i| ((lambda as f64 + 1.0) / 2.0).ln() - ((i + 1) as f64).ln())
            .collect();
        let sum_w: f64 = raw.iter().sum();
        let weights: Vec<f64> = raw.iter().map(|w| w / sum_w).collect();
        let mueff = 1.0 / weights.iter().map(|w| w * w).sum::<f64>();
        let nf = n as f64;
        let cc = (4.0 + mueff / nf) / (nf + 4.0 + 2.0 * mueff / nf);
        let cs = (mueff + 2.0) / (nf + mueff + 5.0);
        let c1 = 2.0 / ((nf + 1.3).powi(2) + mueff);
        let cmu = (2.0 * (mueff - 2.0 + 1.0 / mueff) / ((nf + 2.0).powi(2) + 2.0 * mueff))
            .min(1.0 - c1);
        let damps = 1.0 + 2.0 * ((mueff - 1.0) / (nf + 1.0)).max(0.0).sqrt() + cs;
        let chi_n = nf.sqrt() * (1.0 - 1.0 / (4.0 * nf) + 1.0 / (21.0 * nf * nf));

        CmaState {
            n,
            lambda,
            mu,
            weights,
            mueff,
            cc,
            cs,
            c1,
            cmu,
            damps,
            chi_n,
            mean: mean.clone(),
            sigma,
            // Initial per-dimension step is `0.3 * (max - min)`; we encode the
            // per-dimension scale in the covariance so the effective step along
            // each axis is `sigma * range_i == 0.3 * range_i`.
            c: {
                let mut c = DMatrix::zeros(n, n);
                for i in 0..n {
                    let (lo, hi) = self.space.bounds[i];
                    let r = (hi - lo).max(1e-6);
                    c[(i, i)] = r * r;
                }
                c
            },
            pc: DVector::zeros(n),
            ps: DVector::zeros(n),
            gen_count: 0,
        }
    }

    fn init_state(&mut self) {
        self.state = Some(self.make_state(&self.center, self.init_sigma, self.base_lambda));
    }

    /// Convert a raw CMA-ES sample vector into a `SampledGenome`, clipping to
    /// the search-space bounds and rounding categorical parameters to integers.
    fn to_genome(&self, v: &DVector<f64>) -> SampledGenome {
        let mut v = v.clone();
        self.space.clip(&mut v);
        for &i in &self.space.categorical_indices {
            let (lo, hi) = self.space.bounds[i];
            v[i] = v[i].round().clamp(lo, hi);
        }
        self.space.from_vector(&v)
    }

    /// Re-initialise the state with a different population size / step size
    /// (BIPOP restart), keeping the best score found so far.
    fn restart(&mut self) {
        self.restart_large = !self.restart_large;
        let (sigma, lambda) = if self.restart_large {
            (self.init_sigma * 2.0, self.base_lambda)
        } else {
            (self.init_sigma / RESTART_LO_INV, (self.base_lambda / 2).max(4))
        };
        self.state = Some(self.make_state(&self.center, sigma, lambda));
        self.stagnation = 0;
        self.buffer.clear();
    }

    /// Perform the CMA-ES mean / evolution-path / covariance / step-size update
    /// for one generation given its evaluated population.
    fn update_generation(&mut self, population: &[(DVector<f64>, f64)]) {
        if population.len() < 2 {
            return;
        }

        // Rank by score descending (maximisation). We only need the ordering
        // here; CMA-ES minimises, so higher score = better candidate.
        let mut sorted = population.to_vec();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));

        let gen_best = sorted[0].1;
        if self.best_score.map_or(true, |b| gen_best > b) {
            self.best_score = Some(gen_best);
            self.stagnation = 0;
        } else {
            self.stagnation += 1;
        }

        let st = self.state.as_mut().expect("state present during update");
        let n = st.n;
        let mu = st.mu;

        let mean_old = st.mean.clone();

        // New mean: weighted combination of the `mu` best candidates.
        let mut mean_new = DVector::zeros(n);
        for i in 0..mu {
            mean_new = mean_new + sorted[i].0.clone() * st.weights[i];
        }
        let y_w = (&mean_new - &mean_old) / st.sigma;

        // Eigen-decomposition of the covariance for the path / sampling math.
        let (b, sqrt_evals) = eigendecompose(&st.c);
        let bt_yw = b.transpose() * &y_w;
        let inv = bt_yw.component_mul(&sqrt_evals.map(|s| 1.0 / s));
        let invsqrt_yw = b * inv;

        let cs = st.cs;
        let mueff = st.mueff;
        let cc = st.cc;
        let c1 = st.c1;
        let cmu = st.cmu;
        let damps = st.damps;
        let chi_n = st.chi_n;

        let ps_new =
            (1.0 - cs) * st.ps.clone() + (cs * (2.0 - cs) * mueff).sqrt() * invsqrt_yw;
        let ps_norm = ps_new.norm();

        let t = st.gen_count as f64 + 1.0;
        let denom = (1.0 - (1.0 - cs).powf(2.0 * t)).sqrt();
        let hsig = if ps_norm / denom / chi_n < 1.4 + 2.0 / (n as f64 + 1.0) {
            1.0
        } else {
            0.0
        };

        let pc_new = (1.0 - cc) * st.pc.clone()
            + hsig * (cc * (2.0 - cc) * mueff).sqrt() * y_w.clone();

        // Covariance update: rank-one (pc) + rank-mu (weighted y_i y_i^T).
        let mut c_new = st.c.clone() * (1.0 - c1 - cmu);
        c_new = c_new + pc_new.clone() * pc_new.clone().transpose() * c1;
        for i in 0..mu {
            let y_i = (&sorted[i].0 - &mean_old) / st.sigma;
            c_new = c_new + y_i.clone() * y_i.clone().transpose() * (cmu * st.weights[i]);
        }
        // Keep the matrix positive-definite.
        c_new += DMatrix::from_diagonal(&DVector::from_element(n, 1e-12));

        let sigma_new = st.sigma * ((cs / damps) * (ps_norm / chi_n - 1.0)).exp();

        st.mean = mean_new;
        st.c = c_new;
        st.pc = pc_new;
        st.ps = ps_new;
        st.sigma = sigma_new;
        st.gen_count += 1;
    }
}

impl Optimizer for CmaEsOptimizer {
    fn suggest(&mut self, _history: &[Evaluation]) -> Vec<SampledGenome> {
        if self.space.dimension() == 0 || self.evaluations >= self.budget {
            return vec![];
        }
        if self.state.is_none() {
            self.init_state();
        }

        let mut rng = thread_rng();
        let v = sample_point(self.state.as_ref().unwrap(), &mut rng);
        let genome = self.to_genome(&v);
        self.evaluations += 1;
        vec![genome]
    }

    fn update(&mut self, evaluations: &[Evaluation]) {
        if evaluations.is_empty() || self.state.is_none() {
            return;
        }

        // Buffer the evaluated population. We use the actual evaluated vector
        // (after clip / categorical rounding) as the candidate position: this is
        // the standard "repaired point" fed back to CMA-ES for integer/categorical
        // parameters.
        for ev in evaluations {
            let v = self.space.to_vector(&ev.genome);
            let score = ev.score.value();
            self.buffer.push((v, score));
        }

        let lambda = self.state.as_ref().unwrap().lambda;
        let budget_done = self.evaluations >= self.budget;
        let trigger = self.buffer.len() >= lambda || (budget_done && !self.buffer.is_empty());
        if !trigger {
            return;
        }

        // Take a full generation (or the remainder when the budget is exhausted).
        let take = if budget_done {
            self.buffer.len()
        } else {
            lambda
        };
        let generation: Vec<(DVector<f64>, f64)> = self.buffer.drain(..take).collect();
        // Any overflow (parallel > lambda) is discarded from the CMA-ES buffer;
        // it is already recorded in the store by the caller.
        self.buffer.clear();

        self.update_generation(&generation);

        if self.stagnation >= MAX_STAGNATION {
            self.restart();
        }
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget || self.space.dimension() == 0
    }
}

/// Draw one candidate from the current CMA-ES distribution:
/// `x = mean + sigma * C^{1/2} z`, with `z ~ N(0, I)`.
fn sample_point(state: &CmaState, rng: &mut impl rand::Rng) -> DVector<f64> {
    let n = state.n;
    let (b, sqrt_evals) = eigendecompose(&state.c);
    let z = gaussian_vec(rng, n);
    let temp = b.transpose() * z;
    let temp = temp.component_mul(&sqrt_evals);
    let y = b * temp;
    state.mean.clone() + state.sigma * y
}

/// Symmetric eigendecomposition `(B, sqrt(diag(λ)))` of `C`.
fn eigendecompose(c: &DMatrix<f64>) -> (DMatrix<f64>, DVector<f64>) {
    let eig = c.clone().symmetric_eigen();
    let sqrt_evals = eig.eigenvalues.map(|v| (v.max(1e-20)).sqrt());
    (eig.eigenvectors.clone(), sqrt_evals)
}

fn gaussian_vec(rng: &mut impl rand::Rng, n: usize) -> DVector<f64> {
    let normal = Normal::new(0.0, 1.0).expect("invalid standard normal");
    DVector::from_fn(n, |_, _| normal.sample(rng))
}
