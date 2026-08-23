use crate::{Evaluation, Optimizer};
use fenrir_core::{SampledGenome, SearchSpace};
use rand::SeedableRng;
use rand::rngs::StdRng;

pub struct RandomSearch {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
    rng: StdRng,
}

impl RandomSearch {
    pub fn new(space: SearchSpace, budget: usize, seed: u64) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl Optimizer for RandomSearch {
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
        // Random search doesn't learn
    }

    fn is_done(&self) -> bool {
        self.evaluations >= self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenrir_core::Genome;
    #[test]
    fn same_seed_same_sequence() {
        let mut a = RandomSearch::new(Genome::default_norn().search_space(), 20, 42);
        let mut b = RandomSearch::new(Genome::default_norn().search_space(), 20, 42);
        for _ in 0..20 {
            let ga = a.suggest(&[]);
            let gb = b.suggest(&[]);
            assert_eq!(ga.len(), 1);
            assert_eq!(gb.len(), 1);
            assert_eq!(
                ga[0].values, gb[0].values,
                "same seed must give same genomes"
            );
        }
    }
}
