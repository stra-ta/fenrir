pub mod bayesian;
pub mod cmaes;
pub mod map_elites;
pub mod novelty;
pub mod random;

pub use bayesian::BayesianOptimizer;
pub use cmaes::CmaEsOptimizer;
pub use map_elites::MapElitesOptimizer;
pub use novelty::{NoveltyArchive, cosine_distance};
pub use random::RandomSearch;

use anyhow::{Result, anyhow};
use fenrir_core::{Observation, SampledGenome, Score};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evaluation {
    pub iteration: u64,
    pub genome: SampledGenome,
    pub score: Score,
    pub observation: Observation,
}

/// Portable checkpoint envelope for an optimiser.
///
/// The search space is supplied by the campaign plan and is intentionally not
/// duplicated here.  A checkpoint is valid only when the caller constructs
/// the same optimiser (same algorithm, search-space order, and budget) before
/// restoring it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizerCheckpoint {
    pub algorithm: String,
    pub budget: usize,
    pub state: serde_json::Value,
}

pub trait Optimizer {
    fn suggest(&mut self, history: &[Evaluation]) -> Vec<SampledGenome>;
    fn update(&mut self, evaluations: &[Evaluation]);
    fn is_done(&self) -> bool;

    /// Serialize all mutable search state, including RNG state where the
    /// optimiser has one.  Implementations that do not support persistence
    /// return `None` rather than silently pretending a resume is equivalent.
    fn checkpoint(&self) -> Option<OptimizerCheckpoint> {
        None
    }

    /// Restore a checkpoint created by the same optimiser implementation.
    fn restore(&mut self, _checkpoint: &OptimizerCheckpoint) -> Result<()> {
        Err(anyhow!("optimizer does not support checkpoint restore"))
    }
}
