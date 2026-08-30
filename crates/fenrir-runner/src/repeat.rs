//! Repeated target evaluation helpers.
//!
//! A genome seed replay answers whether the generated workload is the same.
//! Repeating that workload with derived seeds measures timing noise separately.
//! This module keeps those two questions explicit in the result.

use anyhow::Result;
use fenrir_core::{Observation, SampledGenome};
use fenrir_targets::Target;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RobustnessStats {
    pub values: Vec<f64>,
    pub min: f64,
    pub median: f64,
    pub mean: f64,
    pub max: f64,
    pub stddev: f64,
    /// Coefficient-of-variation adjusted score.  Larger is better for a
    /// maximisation objective with a stable high value.
    pub robustness_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatedObservation {
    pub seed: u64,
    pub observations: Vec<Observation>,
    pub p99_latency_ns: RobustnessStats,
}

pub fn summarize(values: &[f64]) -> RobustnessStats {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return RobustnessStats {
            values: Vec::new(),
            min: f64::NAN,
            median: f64::NAN,
            mean: f64::NAN,
            max: f64::NAN,
            stddev: f64::NAN,
            robustness_score: f64::NAN,
        };
    }
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let variance = sorted
        .iter()
        .map(|value| (*value - mean).powi(2))
        .sum::<f64>()
        / sorted.len() as f64;
    let stddev = variance.sqrt();
    let median = sorted[sorted.len() / 2];
    let cv = stddev / mean.abs().max(1e-12);
    RobustnessStats {
        values: sorted.clone(),
        min: sorted[0],
        median,
        mean,
        max: *sorted.last().unwrap_or(&median),
        stddev,
        robustness_score: median / (1.0 + cv),
    }
}

pub async fn evaluate_repeated(
    target: &dyn Target,
    genome: &SampledGenome,
    duration_sec: u64,
    seed: u64,
    repeats: usize,
) -> Result<RepeatedObservation> {
    let repeats = repeats.max(1);
    let mut observations = Vec::with_capacity(repeats);
    for index in 0..repeats {
        let run_seed = seed.wrapping_add((index as u64).wrapping_mul(0x9E3779B97F4A7C15));
        observations.push(crate::evaluate_genome(target, genome, duration_sec, run_seed).await?);
    }
    let values: Vec<f64> = observations
        .iter()
        .filter_map(|observation| observation.metrics.get_scalar("p99_latency_ns"))
        .collect();
    Ok(RepeatedObservation {
        seed,
        observations,
        p99_latency_ns: summarize(&values),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_noise_separately_from_center() {
        let stats = summarize(&[10.0, 12.0, 14.0]);
        assert_eq!(stats.median, 12.0);
        assert_eq!(stats.min, 10.0);
        assert_eq!(stats.max, 14.0);
        assert!(stats.robustness_score < stats.median);
    }

    #[test]
    fn summary_ignores_nonfinite_samples() {
        let stats = summarize(&[1.0, f64::NAN, 3.0]);
        assert_eq!(stats.values, vec![1.0, 3.0]);
    }
}
