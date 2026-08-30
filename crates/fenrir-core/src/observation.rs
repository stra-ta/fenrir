use crate::genome::SampledGenome;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub run_id: String,
    pub genome: SampledGenome,
    pub metrics: Metrics,
    pub artifacts: Artifacts,
    pub status: RunStatus,
    pub timestamp_ns: u64,
    pub duration_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Metrics {
    pub application: HashMap<String, MetricValue>,
    pub hardware: HardwareCounters,
    pub os: OsMetrics,
    pub trace_path: Option<String>,
    /// Scalar provenance labels attached by a target adapter or runner.
    /// Keeping labels outside `application` prevents an optimizer from
    /// accidentally treating a string such as a git SHA as a measurement.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional Ygg execution representation attached by a target runner.
    /// The vector is evidence for novelty search, not a performance metric.
    #[serde(default)]
    pub ygg_embedding: Option<Vec<f32>>,
    #[serde(default)]
    pub ygg_novelty: Option<f64>,
    #[serde(default)]
    pub ygg_regime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MetricValue {
    Scalar(f64),
    Histogram(HistogramData),
    Percentiles(PercentileData),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistogramData {
    pub buckets: Vec<(f64, u64)>,
    pub sum: f64,
    pub count: u64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PercentileData {
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
}

impl Metrics {
    pub fn get_scalar(&self, name: &str) -> Option<f64> {
        self.application.get(name).and_then(|v| match v {
            MetricValue::Scalar(s) => Some(*s),
            _ => None,
        })
    }

    pub fn get_metric(&self, name: &str, aggregate: crate::objective::Aggregate) -> Option<f64> {
        self.application.get(name).and_then(|v| match v {
            MetricValue::Scalar(s) => Some(*s),
            MetricValue::Percentiles(p) => match aggregate {
                crate::objective::Aggregate::P50 => Some(p.p50),
                crate::objective::Aggregate::P90 => Some(p.p90),
                crate::objective::Aggregate::P95 => Some(p.p95),
                crate::objective::Aggregate::P99 => Some(p.p99),
                crate::objective::Aggregate::P999 => Some(p.p999),
                crate::objective::Aggregate::Max => Some(p.max),
                crate::objective::Aggregate::Min => Some(p.p50),
                crate::objective::Aggregate::Mean => {
                    Some((p.p50 + p.p90 + p.p95 + p.p99 + p.p999 + p.max) / 6.0)
                }
                crate::objective::Aggregate::Median => Some(p.p50),
                _ => Some(p.p50),
            },
            MetricValue::Histogram(h) => match aggregate {
                crate::objective::Aggregate::Sum => Some(h.sum),
                crate::objective::Aggregate::Mean => {
                    if h.count > 0 {
                        Some(h.sum / h.count as f64)
                    } else {
                        None
                    }
                }
                _ => Some(h.max),
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HardwareCounters {
    pub cycles: Option<u64>,
    pub instructions: Option<u64>,
    pub branch_instructions: Option<u64>,
    pub branch_misses: Option<u64>,
    pub cache_references: Option<u64>,
    pub cache_misses: Option<u64>,
    pub llc_references: Option<u64>,
    pub llc_misses: Option<u64>,
    pub context_switches: Option<u64>,
    pub page_faults: Option<u64>,
    pub cpu_migrations: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OsMetrics {
    pub syscalls: Option<u64>,
    pub scheduler_latency_us: Option<f64>,
    pub io_wait_us: Option<f64>,
    pub rss_kb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Artifacts {
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub declared_files: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Timeout,
    Crash,
    Oom,
    Error(String),
}
