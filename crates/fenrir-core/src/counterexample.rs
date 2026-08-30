//! Counterexample storage and deterministic genome shrinking.

use crate::{Observation, SampledGenome, SearchSpace};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const COUNTEREXAMPLE_SCHEMA_VERSION: u32 = 1;

/// A minimized, replayable failure or pathological measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counterexample {
    pub schema_version: u32,
    pub campaign_id: String,
    pub target: String,
    pub seed: u64,
    pub genome: SampledGenome,
    pub original_genome: Option<SampledGenome>,
    pub observation: Option<Observation>,
    /// Replays the workload RNG.  It does not imply equal wall-clock timing.
    pub replay_kind: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Counterexample {
    pub fn new(
        campaign_id: impl Into<String>,
        target: impl Into<String>,
        seed: u64,
        genome: SampledGenome,
    ) -> Self {
        Self {
            schema_version: COUNTEREXAMPLE_SCHEMA_VERSION,
            campaign_id: campaign_id.into(),
            target: target.into(),
            seed,
            original_genome: None,
            observation: None,
            genome,
            replay_kind: "seed".into(),
            notes: Vec::new(),
        }
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to publish counterexample {}", path.display()))?;
        Ok(())
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let value: Self = serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("invalid counterexample {}", path.display()))?;
        if value.schema_version != COUNTEREXAMPLE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported counterexample schema version {}",
                value.schema_version
            )
        }
        Ok(value)
    }
}

/// Minimize a candidate while a caller-provided predicate remains true.
///
/// The predicate should run the actual target or a faithful model and return
/// true only when the failure/pathology still reproduces.  Search coordinates
/// are shrunk independently, with categorical dimensions tried at each
/// valid category rather than treating their indices as ordered values.
pub fn minimize_genome<F>(
    space: &SearchSpace,
    candidate: &SampledGenome,
    mut reproduces: F,
) -> SampledGenome
where
    F: FnMut(&SampledGenome) -> bool,
{
    let mut current = candidate.clone();
    for index in 0..space.dimension() {
        let name = &space.param_names[index];
        let (lo, hi) = space.bounds[index];
        let original = current.values.get(name).copied().unwrap_or(lo);
        let mut probes = Vec::new();

        if space.param_types[index] == crate::search_space::ParamType::Categorical {
            let first = lo.ceil() as i64;
            let last = hi.floor() as i64;
            for value in first..=last {
                probes.push(value as f64);
            }
        } else {
            probes.extend([lo, (lo + hi) * 0.5, 0.0].into_iter().map(|v| v.clamp(lo, hi)));
            let low = lo;
            let mut high = original.clamp(lo, hi);
            for _ in 0..12 {
                let mid = (low + high) * 0.5;
                probes.push(mid);
                high = mid;
                if (high - low).abs() <= 1e-12 {
                    break;
                }
            }
        }

        probes.sort_by(f64::total_cmp);
        probes.dedup_by(|a, b| (*a - *b).abs() <= 1e-12);
        for value in probes {
            if (value - original).abs() <= 1e-12 {
                continue;
            }
            let mut trial = current.clone();
            trial.values.insert(name.clone(), value);
            if reproduces(&trial) {
                current = trial;
                // Prefer the smallest deterministic witness for this
                // dimension.  Continuing through larger probes would undo a
                // successful shrink.
                break;
            }
        }
    }

    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Genome;

    #[test]
    fn minimizer_shrinks_continuous_and_categorical_coordinates() {
        let space = Genome::default_norn().search_space();
        let mut candidate = space.random_sample(&mut rand::thread_rng());
        candidate.values.insert("topology.producers".into(), 8.0);
        candidate.values.insert("payload.size".into(), 2.0);
        let minimized = minimize_genome(&space, &candidate, |genome| {
            genome.values["topology.producers"] >= 1.0
                && genome.values["payload.size"] == 2.0
        });
        assert_eq!(minimized.values["topology.producers"], 1.0);
        assert_eq!(minimized.values["payload.size"], 2.0);
    }
}
