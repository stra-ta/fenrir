use crate::{Evaluation, Optimizer};
use fenrir_core::{SampledGenome, SearchSpace};
use rand::SeedableRng;
use rand::rngs::StdRng;

pub struct BayesianOptimizer {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
    rng: StdRng,
}

impl BayesianOptimizer {
    pub fn new(space: SearchSpace, budget: usize, seed: u64) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl Optimizer for BayesianOptimizer {
    fn suggest(&mut self, _history: &[Evaluation]) -> Vec<SampledGenome> {
        if self.evaluations >= self.budget {
            return vec![];
        }
        let mut rng = &mut self.rng;
        let genome = self.space.random_sample(&mut rng);
        self.evaluations += 1;
        vec![genome]
    }

    fn update(&mut self, _evaluations: &[Evaluation]) {
        // TODO: Implement BO
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget
    }
}
