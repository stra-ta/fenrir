use crate::{Evaluation, Optimizer};
use fenrir_core::{SampledGenome, SearchSpace};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use serde::{Deserialize, Serialize};

pub struct RandomSearch {
    space: SearchSpace,
    budget: usize,
    evaluations: usize,
    rng: ChaCha12Rng,
}

#[derive(Debug, Serialize, Deserialize)]
struct RandomState {
    evaluations: usize,
    rng: ChaCha12Rng,
}

impl RandomSearch {
    pub fn new(space: SearchSpace, budget: usize, seed: u64) -> Self {
        Self {
            space,
            budget,
            evaluations: 0,
            rng: ChaCha12Rng::seed_from_u64(seed),
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

    fn checkpoint(&self) -> Option<crate::OptimizerCheckpoint> {
        Some(crate::OptimizerCheckpoint {
            algorithm: "random".into(),
            budget: self.budget,
            state: serde_json::to_value(RandomState {
                evaluations: self.evaluations,
                rng: self.rng.clone(),
            })
            .expect("random optimiser state is serializable"),
        })
    }

    fn restore(&mut self, checkpoint: &crate::OptimizerCheckpoint) -> anyhow::Result<()> {
        if checkpoint.algorithm != "random" || checkpoint.budget != self.budget {
            anyhow::bail!("checkpoint does not match random optimizer")
        }
        let state: RandomState = serde_json::from_value(checkpoint.state.clone())?;
        if state.evaluations > self.budget {
            anyhow::bail!("checkpoint evaluations exceed optimizer budget")
        }
        self.evaluations = state.evaluations;
        self.rng = state.rng;
        Ok(())
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
