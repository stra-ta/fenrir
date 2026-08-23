pub mod bayesian;
pub mod cmaes;
pub mod map_elites;
pub mod random;

pub use bayesian::BayesianOptimizer;
pub use cmaes::CmaEsOptimizer;
pub use map_elites::MapElitesOptimizer;
pub use random::RandomSearch;

use fenrir_core::{Observation, SampledGenome, Score};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evaluation {
    pub iteration: u64,
    pub genome: SampledGenome,
    pub score: Score,
    pub observation: Observation,
}

pub trait Optimizer {
    fn suggest(&mut self, history: &[Evaluation]) -> Vec<SampledGenome>;
    fn update(&mut self, evaluations: &[Evaluation]);
    fn is_done(&self) -> bool;
}
