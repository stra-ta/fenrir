pub mod random;
pub mod cmaes;
pub mod bayesian;
pub mod map_elites;

pub use random::RandomSearch;
pub use cmaes::CmaEsOptimizer;
pub use bayesian::BayesianOptimizer;
pub use map_elites::MapElitesOptimizer;

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