use crate::{Evaluation, Optimizer};
use fenrir_core::{SampledGenome, SearchSpace};

pub struct RandomSearch {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
}

impl RandomSearch {
    pub fn new(space: SearchSpace, budget: usize) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
        }
    }
}

impl Optimizer for RandomSearch {
    fn suggest(&mut self, _history: &[Evaluation]) -> Vec<SampledGenome> {
        if self.evaluations >= self.budget {
            return vec![];
        }
        let mut rng = rand::thread_rng();
        let genome = self.space.random_sample(&mut rng);
        self.evaluations += 1;
        vec![genome]
    }

    fn update(&mut self, _evaluations: &[Evaluation]) {
        // Random search doesn't learn
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget
    }
}