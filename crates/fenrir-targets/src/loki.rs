use crate::Target;
use fenrir_core::genome::{
    AffinitySpec, ArrivalProcess, BehaviorGenome, FaultGenome, KeyDistribution, ParameterSpec,
    PayloadGenome, SizeDistribution, TopologyGenome, TrafficGenome,
};
use fenrir_core::{Genome, SampledGenome};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

/// Loki adversarial target: a fault-injection TCP proxy.
///
/// The C++ adapter (`targets/loki/fenrir_adapter.cpp`) is a self-contained
/// simulator that reads a genome on stdin, derives a fault schedule from the
/// `faults` block, and applies those faults to a synthetic upstream op stream,
/// measuring degraded throughput and elevated tail latency.
pub struct LokiTarget {
    adapter_path: PathBuf,
}

impl LokiTarget {
    pub fn new(adapter_path: PathBuf) -> Self {
        Self { adapter_path }
    }
}

impl Target for LokiTarget {
    fn name(&self) -> &str {
        "loki"
    }

    fn default_genome(&self) -> Genome {
        Genome {
            topology: TopologyGenome {
                producers: ParameterSpec::Range {
                    min: 1.0,
                    max: 32.0,
                    log: false,
                },
                consumers: ParameterSpec::Range {
                    min: 1.0,
                    max: 32.0,
                    log: false,
                },
                affinity: AffinitySpec::Core,
            },
            traffic: TrafficGenome {
                arrival_process: ArrivalProcess::Poisson,
                rate: ParameterSpec::LogUniform {
                    min: 10_000.0,
                    max: 5_000_000.0,
                },
                burstiness: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
                correlation: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
            },
            payload: PayloadGenome {
                size_distribution: SizeDistribution::Fixed,
                size: ParameterSpec::Choice {
                    values: vec![
                        serde_json::json!(64),
                        serde_json::json!(256),
                        serde_json::json!(512),
                        serde_json::json!(1024),
                        serde_json::json!(4096),
                    ],
                },
                key_distribution: KeyDistribution::Uniform,
            },
            behavior: BehaviorGenome {
                read_write_ratio: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
                lifetime_ns: ParameterSpec::LogUniform {
                    min: 1_000.0,
                    max: 10_000_000.0,
                },
                think_time_ns: ParameterSpec::LogUniform {
                    min: 1.0,
                    max: 100_000.0,
                },
            },
            faults: Some(FaultGenome {
                enabled: true,
                scenario_template: "latency+reorder+blackhole".to_string(),
                parameters: {
                    let mut m = HashMap::new();
                    m.insert(
                        "fault_probability".to_string(),
                        ParameterSpec::Range {
                            min: 0.0,
                            max: 1.0,
                            log: false,
                        },
                    );
                    m.insert(
                        "latency_mean_ns".to_string(),
                        ParameterSpec::LogUniform {
                            min: 1_000.0,
                            max: 1_000_000.0,
                        },
                    );
                    m.insert(
                        "reorder_prob".to_string(),
                        ParameterSpec::Range {
                            min: 0.0,
                            max: 1.0,
                            log: false,
                        },
                    );
                    m.insert(
                        "blackhole_prob".to_string(),
                        ParameterSpec::Range {
                            min: 0.0,
                            max: 1.0,
                            log: false,
                        },
                    );
                    m
                },
            }),
        }
    }

    fn adapter_path(&self) -> PathBuf {
        self.adapter_path.clone()
    }

    fn genome_to_adapter_input(&self, genome: &SampledGenome) -> Value {
        let mut obj = serde_json::Map::new();

        // Topology
        let mut topology = serde_json::Map::new();
        if let Some(v) = genome.values.get("topology.producers") {
            topology.insert("producers".into(), json!(*v as u64));
        }
        if let Some(v) = genome.values.get("topology.consumers") {
            topology.insert("consumers".into(), json!(*v as u64));
        }
        if let Some(v) = genome.values.get("topology.affinity") {
            let affinity = match *v as u64 {
                0 => "none",
                1 => "socket",
                2 => "core",
                3 => "smt_sibling",
                _ => "core",
            };
            topology.insert("affinity".into(), json!(affinity));
        }
        obj.insert("topology".into(), topology.into());

        // Traffic
        let mut traffic = serde_json::Map::new();
        traffic.insert("arrival_process".into(), json!("poisson"));
        if let Some(v) = genome.values.get("traffic.rate") {
            traffic.insert("rate".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("traffic.burstiness") {
            traffic.insert("burstiness".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("traffic.correlation") {
            traffic.insert("correlation".into(), json!(*v));
        }
        obj.insert("traffic".into(), traffic.into());

        // Payload
        let mut payload = serde_json::Map::new();
        payload.insert("size_distribution".into(), json!("fixed"));
        if let Some(v) = genome.values.get("payload.size") {
            payload.insert("size".into(), json!(*v as u64));
        }
        payload.insert("key_distribution".into(), json!("uniform"));
        obj.insert("payload".into(), payload.into());

        // Behavior
        let mut behavior = serde_json::Map::new();
        if let Some(v) = genome.values.get("behavior.read_write_ratio") {
            behavior.insert("read_write_ratio".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("behavior.lifetime_ns") {
            behavior.insert("lifetime_ns".into(), json!(*v as u64));
        }
        if let Some(v) = genome.values.get("behavior.think_time_ns") {
            behavior.insert("think_time_ns".into(), json!(*v as u64));
        }
        obj.insert("behavior".into(), behavior.into());

        obj.insert("duration_sec".into(), json!(10));
        obj.insert("warmup_sec".into(), json!(2));

        // Faults: built from the FaultGenome parameters sampled into
        // `genome.values` under the key `faults.<name>`.
        let mut faults = serde_json::Map::new();
        if let Some(v) = genome.values.get("faults.fault_probability") {
            faults.insert("fault_probability".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("faults.latency_mean_ns") {
            faults.insert("latency_mean_ns".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("faults.reorder_prob") {
            faults.insert("reorder_prob".into(), json!(*v));
        }
        if let Some(v) = genome.values.get("faults.blackhole_prob") {
            faults.insert("blackhole_prob".into(), json!(*v));
        }
        obj.insert("faults".into(), faults.into());

        obj.into()
    }
}
