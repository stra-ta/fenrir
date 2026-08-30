//! Mixed-domain Bayesian optimisation.
//!
//! Fenrir's genome is deliberately small and heterogeneous.  A useful
//! optimiser for it cannot treat a choice such as an affinity policy as a
//! continuous number.  This module uses a light-weight Gaussian-process
//! surrogate with a mixed distance kernel and expected improvement.  The
//! implementation keeps the dependency footprint small by using the
//! `nalgebra` linear algebra already used by the CMA-ES optimiser.

use crate::{Evaluation, Optimizer};
use fenrir_core::{ParamType, SampledGenome, SearchSpace, Score};
use nalgebra::{DMatrix, DVector};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use serde::{Deserialize, Serialize};

const DEFAULT_INITIAL_RANDOM: usize = 8;
const DEFAULT_CANDIDATE_POOL: usize = 256;
const DEFAULT_NOISE: f64 = 1e-6;
const JITTER: f64 = 1e-9;

/// Tuning knobs for [`BayesianOptimizer`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BayesianConfig {
    /// Number of space-filling evaluations before fitting a surrogate.
    pub initial_random: usize,
    /// Number of random candidates scored by expected improvement per step.
    pub candidate_pool: usize,
    /// Extra weight for posterior uncertainty in the acquisition function.
    pub exploration: f64,
    /// Observation noise used to keep repeated/noisy measurements invertible.
    pub noise: f64,
}

impl Default for BayesianConfig {
    fn default() -> Self {
        Self {
            initial_random: DEFAULT_INITIAL_RANDOM,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            exploration: 0.01,
            noise: DEFAULT_NOISE,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrainingPoint {
    coordinates: Vec<f64>,
    score: f64,
    feasible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BayesianState {
    evaluations: usize,
    rng: ChaCha12Rng,
    observations: Vec<TrainingPoint>,
    config: BayesianConfig,
}

/// A seeded Bayesian optimiser over continuous, log-continuous, discrete,
/// and categorical genome dimensions.
pub struct BayesianOptimizer {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
    rng: ChaCha12Rng,
    observations: Vec<TrainingPoint>,
    config: BayesianConfig,
}

impl BayesianOptimizer {
    pub fn new(space: SearchSpace, budget: usize, seed: u64) -> Self {
        Self::with_config(space, budget, seed, BayesianConfig::default())
    }

    pub fn with_config(
        space: SearchSpace,
        budget: usize,
        seed: u64,
        config: BayesianConfig,
    ) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
            rng: ChaCha12Rng::seed_from_u64(seed),
            observations: Vec::new(),
            config: BayesianConfig {
                initial_random: config.initial_random,
                candidate_pool: config.candidate_pool.max(8),
                exploration: config.exploration.max(0.0),
                noise: config.noise.max(JITTER),
            },
        }
    }

    pub fn config(&self) -> &BayesianConfig {
        &self.config
    }

    pub fn observation_count(&self) -> usize {
        self.observations.len()
    }

    /// Return observations in coordinate space for diagnostics and checkpoint
    /// tooling.  The returned values are copies, so callers cannot mutate the
    /// surrogate accidentally.
    pub fn observations(&self) -> Vec<(Vec<f64>, Score, bool)> {
        self.observations
            .iter()
            .map(|point| {
                (
                    point.coordinates.clone(),
                    Score(point.score),
                    point.feasible,
                )
            })
            .collect()
    }

    fn normalized_coordinate(&self, index: usize, value: f64) -> f64 {
        let (lo, hi) = self.space.bounds[index];
        if (hi - lo).abs() <= f64::EPSILON {
            return 0.0;
        }
        match self.space.param_types[index] {
            ParamType::LogContinuous if lo > 0.0 && hi > 0.0 => {
                let llo = lo.ln();
                let lhi = hi.ln();
                ((value.max(lo).ln() - llo) / (lhi - llo)).clamp(0.0, 1.0)
            }
            _ => ((value - lo) / (hi - lo)).clamp(0.0, 1.0),
        }
    }

    /// Matern-like mixed-domain kernel.  Continuous values use a normalized
    /// distance; discrete and categorical values use a Hamming distance.  The
    /// latter prevents the surrogate from inventing an ordering between enum
    /// choices while still allowing all choices into one model.
    fn kernel(&self, lhs: &[f64], rhs: &[f64]) -> f64 {
        let mut squared_distance = 0.0;
        for i in 0..self.space.dimension() {
            let distance = match self.space.param_types[i] {
                ParamType::Categorical => {
                    if lhs[i].round() == rhs[i].round() {
                        0.0
                    } else {
                        1.0
                    }
                }
                ParamType::Discrete => {
                    (lhs[i] - rhs[i]).abs()
                        / (self.space.bounds[i].1 - self.space.bounds[i].0).max(1.0)
                }
                _ => {
                    (self.normalized_coordinate(i, lhs[i])
                        - self.normalized_coordinate(i, rhs[i]))
                        .abs()
                }
            };
            squared_distance += distance * distance;
        }
        (-0.5 * squared_distance).exp()
    }

    fn feasible_probability(&self, candidate: &[f64]) -> f64 {
        if self.observations.is_empty() {
            return 1.0;
        }

        let mut weighted_feasible = 0.0;
        let mut weight_total = 0.0;
        for point in &self.observations {
            let weight = self.kernel(candidate, &point.coordinates).max(JITTER);
            weight_total += weight;
            if point.feasible {
                weighted_feasible += weight;
            }
        }
        // A small prior keeps unexplored regions eligible when all nearby
        // observations are infeasible, while still respecting constraints.
        let empirical = weighted_feasible / weight_total.max(JITTER);
        let global = self
            .observations
            .iter()
            .filter(|point| point.feasible)
            .count() as f64
            / self.observations.len() as f64;
        (0.85 * empirical + 0.15 * global).clamp(0.0, 1.0)
    }

    fn posterior(&self, candidate: &[f64]) -> Option<(f64, f64)> {
        let feasible: Vec<&TrainingPoint> = self
            .observations
            .iter()
            .filter(|point| point.feasible && point.score.is_finite())
            .collect();
        if feasible.is_empty() {
            return None;
        }

        let n = feasible.len();
        let mut matrix = DMatrix::zeros(n, n);
        let mut targets = DVector::zeros(n);
        let mean = feasible.iter().map(|point| point.score).sum::<f64>() / n as f64;
        let variance = feasible
            .iter()
            .map(|point| (point.score - mean).powi(2))
            .sum::<f64>()
            / n.max(2) as f64;
        let scale = variance.sqrt().max(JITTER);

        for row in 0..n {
            targets[row] = (feasible[row].score - mean) / scale;
            for col in 0..n {
                matrix[(row, col)] = self.kernel(
                    &feasible[row].coordinates,
                    &feasible[col].coordinates,
                );
            }
            matrix[(row, row)] += self.config.noise + JITTER;
        }

        let cholesky = matrix.cholesky()?;
        let alpha = cholesky.solve(&targets);
        let mut cross = DVector::zeros(n);
        for (i, point) in feasible.iter().enumerate() {
            cross[i] = self.kernel(candidate, &point.coordinates);
        }
        let mean_std = cross.dot(&alpha);
        let solved = cholesky.solve(&cross);
        let variance_std = (1.0 - cross.dot(&solved)).max(0.0);
        Some((mean + scale * mean_std, scale * variance_std.sqrt()))
    }

    fn normal_pdf(value: f64) -> f64 {
        (-0.5 * value * value).exp() / (2.0 * std::f64::consts::PI).sqrt()
    }

    fn normal_cdf(value: f64) -> f64 {
        // Abramowitz and Stegun 7.1.26, maximum error about 7.5e-8.
        let sign = if value < 0.0 { -1.0 } else { 1.0 };
        let x = value.abs() / 2.0_f64.sqrt();
        let t = 1.0 / (1.0 + 0.3275911 * x);
        let poly = (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t
            - 0.284496736)
            * t
            + 0.254829592)
            * t;
        let erf = sign * (1.0 - poly * (-x * x).exp());
        0.5 * (1.0 + erf)
    }

    fn best_feasible_score(&self) -> Option<f64> {
        self.observations
            .iter()
            .filter(|point| point.feasible && point.score.is_finite())
            .map(|point| point.score)
            .max_by(f64::total_cmp)
    }

    fn acquisition(&self, candidate: &[f64]) -> f64 {
        let feasibility = self.feasible_probability(candidate);
        let Some(best) = self.best_feasible_score() else {
            // No feasible observation exists yet.  Search for a region with a
            // high modelled chance of satisfying the constraints.
            return feasibility;
        };
        let Some((mean, sigma)) = self.posterior(candidate) else {
            return feasibility;
        };

        let sigma = sigma.max(JITTER);
        let improvement = mean - best;
        let z = improvement / sigma;
        let expected_improvement = improvement * Self::normal_cdf(z) + sigma * Self::normal_pdf(z);
        feasibility * (expected_improvement.max(0.0) + self.config.exploration * sigma)
    }

    fn suggest_bayesian(&mut self) -> SampledGenome {
        let mut best = self.space.random_sample(&mut self.rng);
        let mut best_value = f64::NEG_INFINITY;
        for _ in 0..self.config.candidate_pool {
            let candidate = self.space.random_sample(&mut self.rng);
            let vector = self.space.to_vector(&candidate);
            let value = self.acquisition(vector.as_slice());
            if value > best_value {
                best_value = value;
                best = candidate;
            }
        }
        best
    }
}

impl Optimizer for BayesianOptimizer {
    fn suggest(&mut self, _history: &[Evaluation]) -> Vec<SampledGenome> {
        if self.evaluations >= self.budget || self.space.dimension() == 0 {
            return vec![];
        }
        let genome = if self.observations.len() < self.config.initial_random {
            self.space.random_sample(&mut self.rng)
        } else {
            self.suggest_bayesian()
        };
        self.evaluations += 1;
        vec![genome]
    }

    fn update(&mut self, evaluations: &[Evaluation]) {
        for evaluation in evaluations {
            let score = evaluation.score.value();
            self.observations.push(TrainingPoint {
                coordinates: self.space.to_vector(&evaluation.genome).as_slice().to_vec(),
                score,
                feasible: evaluation.score.feasible() && score.is_finite(),
            });
        }
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget || self.space.dimension() == 0
    }

    fn checkpoint(&self) -> Option<crate::OptimizerCheckpoint> {
        Some(crate::OptimizerCheckpoint {
            algorithm: "bayesian".into(),
            budget: self.budget,
            state: serde_json::to_value(BayesianState {
                evaluations: self.evaluations,
                rng: self.rng.clone(),
                observations: self.observations.clone(),
                config: self.config.clone(),
            })
            .expect("bayesian optimiser state is serializable"),
        })
    }

    fn restore(&mut self, checkpoint: &crate::OptimizerCheckpoint) -> anyhow::Result<()> {
        if checkpoint.algorithm != "bayesian" || checkpoint.budget != self.budget {
            anyhow::bail!("checkpoint does not match bayesian optimizer")
        }
        let state: BayesianState = serde_json::from_value(checkpoint.state.clone())?;
        if state.evaluations > self.budget {
            anyhow::bail!("checkpoint evaluations exceed optimizer budget")
        }
        if state.observations.iter().any(|p| p.coordinates.len() != self.space.dimension()) {
            anyhow::bail!("checkpoint search-space dimension does not match genome")
        }
        self.evaluations = state.evaluations;
        self.rng = state.rng;
        self.observations = state.observations;
        self.config = state.config;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenrir_core::{Artifacts, Genome, MetricValue, Metrics, Observation, RunStatus};
    use std::collections::HashMap;

    fn evaluation(genome: SampledGenome, score: f64, feasible: bool) -> Evaluation {
        let mut application = HashMap::new();
        application.insert(
            "throughput_ops_s".to_string(),
            MetricValue::Scalar(if feasible { 2_000_000.0 } else { 0.0 }),
        );
        let observation = Observation {
            run_id: "test".into(),
            genome: genome.clone(),
            metrics: Metrics {
                application,
                ..Metrics::default()
            },
            artifacts: Artifacts::default(),
            status: RunStatus::Success,
            timestamp_ns: 0,
            duration_ns: 1,
        };
        Evaluation {
            iteration: 0,
            genome,
            score: if feasible { Score(score) } else { Score::infeasible() },
            observation,
        }
    }

    #[test]
    fn uses_surrogate_after_initial_design() {
        let space = Genome::default_norn().search_space();
        let mut optimizer = BayesianOptimizer::with_config(
            space,
            20,
            7,
            BayesianConfig {
                initial_random: 2,
                candidate_pool: 32,
                exploration: 0.01,
                noise: 1e-6,
            },
        );
        for score in [1.0, 4.0] {
            let genome = optimizer.suggest(&[]).pop().unwrap();
            optimizer.update(&[evaluation(genome, score, true)]);
        }
        let genome = optimizer.suggest(&[]).pop().unwrap();
        assert_eq!(optimizer.observation_count(), 2);
        assert_eq!(genome.values.len(), optimizer.space.dimension());
    }

    #[test]
    fn mixed_categorical_coordinates_are_valid() {
        let space = Genome::default_norn().search_space();
        let mut optimizer = BayesianOptimizer::new(space, 3, 1);
        for _ in 0..3 {
            let genome = optimizer.suggest(&[]).pop().unwrap();
            assert!(genome.values["payload.size"] >= 0.0);
            assert!(genome.values["payload.size"] <= 5.0);
            optimizer.update(&[evaluation(genome, 1.0, true)]);
        }
    }

    #[test]
    fn constraint_feasibility_is_part_of_acquisition() {
        let space = Genome::default_norn().search_space();
        let mut optimizer = BayesianOptimizer::with_config(
            space,
            4,
            3,
            BayesianConfig {
                initial_random: 1,
                candidate_pool: 16,
                ..BayesianConfig::default()
            },
        );
        let first = optimizer.suggest(&[]).pop().unwrap();
        optimizer.update(&[evaluation(first, 100.0, false)]);
        let second = optimizer.suggest(&[]).pop().unwrap();
        assert_eq!(second.values.len(), optimizer.space.dimension());
    }
}
