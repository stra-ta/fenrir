pub mod kiln;
pub mod local;
pub mod repeat;

use anyhow::Result;
use fenrir_core::{Observation, SampledGenome};
use fenrir_targets::Target;

/// Evaluate a single genome against a target using the local subprocess adapter.
///
/// The target supplies both the adapter binary path and the genome→adapter JSON
/// mapping, so this is target-agnostic.
pub async fn evaluate_genome(
    target: &dyn Target,
    genome: &SampledGenome,
    duration_sec: u64,
    seed: u64,
) -> Result<Observation> {
    local::run_adapter(target, genome, duration_sec, seed).await
}
