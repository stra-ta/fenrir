//! Ygg embedding novelty for Fenrir search.

use fenrir_core::Observation;

/// Cosine distance between two Ygg execution embeddings.
pub fn cosine_distance(lhs: &[f32], rhs: &[f32]) -> Option<f64> {
    if lhs.is_empty() || lhs.len() != rhs.len() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut lhs_norm = 0.0_f64;
    let mut rhs_norm = 0.0_f64;
    for (&a, &b) in lhs.iter().zip(rhs) {
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        let a = f64::from(a);
        let b = f64::from(b);
        dot += a * b;
        lhs_norm += a * a;
        rhs_norm += b * b;
    }
    if lhs_norm <= f64::EPSILON || rhs_norm <= f64::EPSILON {
        return None;
    }
    Some(1.0 - (dot / (lhs_norm.sqrt() * rhs_norm.sqrt())).clamp(-1.0, 1.0))
}

/// Small, deterministic novelty archive used by Fenrir's optional Ygg path.
///
/// Search remains useful when Ygg is unavailable.  Observations without an
/// embedding are simply skipped, and no synthetic embedding is invented.
#[derive(Debug, Clone, Default)]
pub struct NoveltyArchive {
    embeddings: Vec<Vec<f32>>,
}

impl NoveltyArchive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.embeddings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.embeddings.is_empty()
    }

    pub fn score(&self, embedding: &[f32], k: usize) -> Option<f64> {
        if self.embeddings.is_empty() {
            return None;
        }
        let mut distances: Vec<f64> = self
            .embeddings
            .iter()
            .filter_map(|other| cosine_distance(embedding, other))
            .collect();
        if distances.is_empty() {
            return None;
        }
        distances.sort_by(f64::total_cmp);
        let n = distances.len().min(k.max(1));
        Some(distances[..n].iter().sum::<f64>() / n as f64)
    }

    /// Insert the observation and return its novelty relative to prior
    /// observations.  Insertion happens after scoring, so a repeated trace has
    /// a zero-distance neighbour rather than inflating its own score.
    pub fn observe(&mut self, observation: &Observation, k: usize) -> Option<f64> {
        let embedding = observation.metrics.ygg_embedding.as_deref()?;
        let novelty = self.score(embedding, k).unwrap_or(1.0);
        self.embeddings.push(embedding.to_vec());
        Some(novelty)
    }

    pub fn embeddings(&self) -> &[Vec<f32>] {
        &self.embeddings
    }

    pub(crate) fn embeddings_mut(&mut self) -> &mut Vec<Vec<f32>> {
        &mut self.embeddings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenrir_core::{Artifacts, Metrics, Observation, RunStatus, SampledGenome};
    use serde_json::Value;
    use std::collections::HashMap;

    fn observation(embedding: Vec<f32>) -> Observation {
        Observation {
            run_id: "run".into(),
            genome: SampledGenome {
                values: HashMap::new(),
                target_config: Value::Null,
            },
            metrics: Metrics {
                ygg_embedding: Some(embedding),
                ..Metrics::default()
            },
            artifacts: Artifacts::default(),
            status: RunStatus::Success,
            timestamp_ns: 0,
            duration_ns: 0,
        }
    }

    #[test]
    fn novelty_is_zero_for_a_repeated_embedding() {
        let mut archive = NoveltyArchive::new();
        assert_eq!(archive.observe(&observation(vec![1.0, 0.0]), 1), Some(1.0));
        let second = archive.observe(&observation(vec![1.0, 0.0]), 1).unwrap();
        assert!(second.abs() < 1e-12);
    }
}
