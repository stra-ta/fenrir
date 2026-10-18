use crate::{Evaluation, NoveltyArchive, Optimizer};
use fenrir_core::{Observation, SampledGenome, Score, SearchSpace};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Number of bins per behavioral-feature axis (6x6 = up to 36 elite cells).
const N_BINS: usize = 6;

/// Axis 1 `throughput` bucketing.
///
/// We bin in log10 space because throughput spans orders of magnitude. The
/// plausible operating envelope is taken as [1e3, 1e8] ops/s (light load up to
/// a heavily saturated pipeline). `log10(1e3) = 3.0`, `log10(1e8) = 8.0`.
const THR_LOG_MIN: f64 = 3.0;
const THR_LOG_MAX: f64 = 8.0;

/// Axis 2 `tail` bucketing.
///
/// Tail amplification = `p99_latency_ns / max(p50_latency_ns, 1.0)`. We bin in
/// log10 space over [1.0, 1000.0]: a ratio of 1.0 means no tail, 1000x means a
/// near-pathological tail. `log10(1.0) = 0.0`, `log10(1000.0) = 3.0`.
const TAIL_LOG_MIN: f64 = 0.0;
const TAIL_LOG_MAX: f64 = 3.0;

/// Standard deviation of the per-dimension perturbation as a fraction of the
/// bound width. Matches the 0.1 * (hi - lo) scale used by CMA-ES per-axis step.
const PERTURB_FRACTION: f64 = 0.1;

/// Probability of returning a pure random sample versus a perturbed elite.
const RANDOM_PROB: f64 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArchiveEntry {
    cell: (usize, usize),
    score: Score,
    genome: SampledGenome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MapElitesState {
    evaluations: usize,
    rng: ChaCha12Rng,
    archive: Vec<ArchiveEntry>,
    #[serde(default)]
    novelty_embeddings: Vec<Vec<f32>>,
}

/// A genuine MAP-Elites (Quality-Diversity) optimizer.
///
/// It maintains an archive mapping each (behavioral-feature) cell to the best
/// elite (highest `Score`, where higher means more pathological) seen so far.
/// `suggest` returns exactly one candidate: either a uniform random sample or a
/// Gaussian perturbation of a stored elite, driving illumination of the
/// feature space rather than greedy maximization of a single objective.
pub struct MapElitesOptimizer {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
    /// Seeded RNG derived from the campaign seed.
    rng: ChaCha12Rng,
    /// cell (throughput_bin, tail_bin) -> (best_score, elite_genome)
    archive: HashMap<(usize, usize), (Score, SampledGenome)>,
    /// Optional Ygg embeddings observed alongside evaluations.  The archive
    /// remains usable when instrumentation is absent; such evaluations are
    /// simply not added here.
    novelty: NoveltyArchive,
}

impl MapElitesOptimizer {
    pub fn new(space: SearchSpace, budget: usize, seed: u64) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
            rng: ChaCha12Rng::seed_from_u64(seed),
            archive: HashMap::new(),
            novelty: NoveltyArchive::new(),
        }
    }

    /// Map a positive feature value to a bin index over a log10-spaced range.
    ///
    /// Values outside `[10^log_min, 10^log_max]` are clamped to the edge bins;
    /// non-positive (invalid) values collapse to bin 0.
    fn bin_index(value: f64, log_min: f64, log_max: f64, n: usize) -> usize {
        let lv = value.log10();
        if !lv.is_finite() {
            return 0;
        }
        let t = ((lv - log_min) / (log_max - log_min)).clamp(0.0, 1.0);
        let idx = (t * n as f64).floor() as usize;
        idx.min(n - 1)
    }

    /// Compute the archive cell for an evaluation from its observation metrics.
    /// Returns `None` if a required metric is missing (then we skip archiving).
    fn cell_of(observation: &Observation) -> Option<(usize, usize)> {
        let throughput = observation.metrics.get_scalar("throughput_ops_s")?;
        let p50 = observation.metrics.get_scalar("p50_latency_ns")?;
        let p99 = observation.metrics.get_scalar("p99_latency_ns")?;
        if !throughput.is_finite() || !p50.is_finite() || !p99.is_finite() {
            return None;
        }
        let tail = p99 / p50.max(1.0);
        if !tail.is_finite() || tail <= 0.0 {
            return None;
        }
        let t_bin = Self::bin_index(throughput, THR_LOG_MIN, THR_LOG_MAX, N_BINS);
        let a_bin = Self::bin_index(tail, TAIL_LOG_MIN, TAIL_LOG_MAX, N_BINS);
        Some((t_bin, a_bin))
    }

    /// Gaussian-perturb an elite genome, mimicking `cmaes.rs`'s `to_genome`
    /// rounding so categorical indices stay integers.
    fn perturb(space: &SearchSpace, elite: &SampledGenome, rng: &mut impl Rng) -> SampledGenome {
        let mut v = space.to_vector(elite);
        let normal = Normal::new(0.0, 1.0).expect("standard normal is always valid");
        for i in 0..space.dimension() {
            let (lo, hi) = space.bounds[i];
            let sigma = PERTURB_FRACTION * (hi - lo).max(1e-9);
            v[i] += normal.sample(rng) * sigma;
        }
        // Repaired point: clip then round categorical indices, exactly as in
        // cmaes.rs to_genome.
        space.clip(&mut v);
        for &i in space
            .categorical_indices
            .iter()
            .chain(space.discrete_indices.iter())
        {
            let (lo, hi) = space.bounds[i];
            v[i] = v[i].round().clamp(lo, hi);
        }
        space.from_vector(&v)
    }

    /// Number of occupied archive cells (for diagnostics / tests).
    pub fn archive_len(&self) -> usize {
        self.archive.len()
    }

    /// Snapshot of the archive as (cell, score, genome) triples.
    pub fn archive_snapshot(&self) -> Vec<((usize, usize), Score, SampledGenome)> {
        self.archive
            .iter()
            .map(|(cell, (score, genome))| (*cell, *score, genome.clone()))
            .collect()
    }

    /// Number of Ygg execution embeddings retained for novelty diagnostics.
    pub fn novelty_len(&self) -> usize {
        self.novelty.len()
    }
}

impl Optimizer for MapElitesOptimizer {
    fn suggest(&mut self, _history: &[Evaluation]) -> Vec<SampledGenome> {
        // The orchestrator loop consumes exactly the first element; we always
        // return exactly one candidate.
        if self.evaluations >= self.budget {
            return vec![];
        }
        let mut rng = &mut self.rng;

        let candidate = if !self.archive.is_empty() && !rng.gen_bool(RANDOM_PROB) {
            // Pick a random elite and perturb it.
            let cells: Vec<(usize, usize)> = self.archive.keys().copied().collect();
            let pick = &self.archive[&cells[rng.gen_range(0..cells.len())]];
            Self::perturb(&self.space, &pick.1, &mut rng)
        } else {
            // Uniform random exploration.
            self.space.random_sample(&mut rng)
        };

        vec![candidate]
    }

    fn update(&mut self, evaluations: &[Evaluation]) {
        for ev in evaluations {
            // Ygg is an optional instrumentation path.  Record embeddings
            // before archive filtering so infeasible executions can still
            // identify novel behavioral regimes.
            let _ = self.novelty.observe(&ev.observation, 5);

            // Only feasible (finite) scores populate the archive.
            if !ev.score.feasible() {
                continue;
            }
            // Behavioral cell is derived from the observation; skip if a metric
            // is missing or degenerate.
            let cell = match Self::cell_of(&ev.observation) {
                Some(c) => c,
                None => continue,
            };
            let improved = match self.archive.get(&cell) {
                Some((stored_score, _)) => ev.score.value() > stored_score.value(),
                None => true,
            };
            if improved {
                self.archive.insert(cell, (ev.score, ev.genome.clone()));
            }
        }
        self.evaluations += evaluations.len();
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget
    }

    fn checkpoint(&self) -> Option<crate::OptimizerCheckpoint> {
        let archive = self
            .archive
            .iter()
            .map(|(cell, (score, genome))| ArchiveEntry {
                cell: *cell,
                score: *score,
                genome: genome.clone(),
            })
            .collect();
        Some(crate::OptimizerCheckpoint {
            algorithm: "map-elites".into(),
            budget: self.budget,
            state: serde_json::to_value(MapElitesState {
                evaluations: self.evaluations,
                rng: self.rng.clone(),
                archive,
                novelty_embeddings: self.novelty.embeddings().to_vec(),
            })
            .expect("map-elites optimiser state is serializable"),
        })
    }

    fn restore(&mut self, checkpoint: &crate::OptimizerCheckpoint) -> anyhow::Result<()> {
        if checkpoint.algorithm != "map-elites" || checkpoint.budget != self.budget {
            anyhow::bail!("checkpoint does not match map-elites optimizer")
        }
        let state: MapElitesState = serde_json::from_value(checkpoint.state.clone())?;
        if state.evaluations > self.budget {
            anyhow::bail!("checkpoint evaluations exceed optimizer budget")
        }
        if state
            .archive
            .iter()
            .any(|entry| entry.genome.values.len() != self.space.dimension())
        {
            anyhow::bail!("checkpoint search-space dimension does not match genome")
        }
        self.evaluations = state.evaluations;
        self.rng = state.rng;
        self.archive = state
            .archive
            .into_iter()
            .map(|entry| (entry.cell, (entry.score, entry.genome)))
            .collect();
        self.novelty = NoveltyArchive::new();
        self.novelty
            .embeddings_mut()
            .extend(state.novelty_embeddings);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenrir_core::{Artifacts, MetricValue, Metrics, RunStatus};
    use std::collections::HashMap as StdHashMap;

    #[test]
    fn archive_is_illuminated_and_nonempty() {
        let space = fenrir_core::Genome::default_norn().search_space();
        let mut opt = MapElitesOptimizer::new(space, 40, 42);

        let mut rng = rand::thread_rng();

        while !opt.is_done() {
            let g = opt.suggest(&[]);
            let genome = g.into_iter().next().unwrap();

            // Build a synthetic observation whose metrics feed the feature axes.
            let throughput = 1e3 * (1.0 + rng.gen_range(0.0..1.0) * 1e5); // ~1e3 .. 1e8
            let p50 = 1000.0;
            let p99 = 1000.0 * (1.0 + rng.gen_range(0.0..1.0) * 50.0); // tail 1x .. 50x

            let mut application = StdHashMap::new();
            application.insert(
                "throughput_ops_s".to_string(),
                MetricValue::Scalar(throughput),
            );
            application.insert("p50_latency_ns".to_string(), MetricValue::Scalar(p50));
            application.insert("p99_latency_ns".to_string(), MetricValue::Scalar(p99));

            let metrics = Metrics {
                application,
                ..Default::default()
            };

            let observation = Observation {
                run_id: "t".into(),
                genome: genome.clone(),
                metrics,
                artifacts: Artifacts::default(),
                status: RunStatus::Success,
                timestamp_ns: 0,
                duration_ns: 0,
            };

            // Positive, feasible score -> qualifies for archiving.
            let score = Score(p99);
            let eval = Evaluation {
                iteration: 0,
                genome,
                score,
                observation,
            };
            opt.update(&[eval]);
        }

        assert!(opt.archive_len() > 0, "archive must be non-empty");
        eprintln!("archive_len = {}", opt.archive_len());
    }

    #[test]
    fn ygg_embeddings_are_retained_for_novelty_diagnostics() {
        let space = fenrir_core::Genome::default_norn().search_space();
        let genome = space.random_sample(&mut rand::thread_rng());
        let metrics = Metrics {
            ygg_embedding: Some(vec![1.0, 0.0]),
            ..Default::default()
        };
        let observation = Observation {
            run_id: "ygg".into(),
            genome: genome.clone(),
            metrics,
            artifacts: Artifacts::default(),
            status: RunStatus::Success,
            timestamp_ns: 0,
            duration_ns: 0,
        };
        let evaluation = Evaluation {
            iteration: 0,
            genome,
            score: Score(1.0),
            observation,
        };

        let mut optimizer = MapElitesOptimizer::new(space, 1, 7);
        optimizer.update(&[evaluation]);
        assert_eq!(optimizer.novelty_len(), 1);
    }
}
