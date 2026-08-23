use crate::Target;
use fenrir_core::genome::{
    AffinitySpec, ArrivalProcess, BehaviorGenome, KeyDistribution, ParameterSpec, PayloadGenome,
    SizeDistribution, TopologyGenome, TrafficGenome,
};
use fenrir_core::{Genome, SampledGenome};
use serde_json::{json, Value};
use std::path::PathBuf;

/// Fenrir target for the Orda matching-engine / order-book.
///
/// The C++ adapter (`targets/orda/fenrir_adapter.cpp`) is a self-contained
/// faithful synthetic model of an order book: it reads the genome JSON on stdin
/// and prints metrics on stdout. No real Orda library is required for v0.
pub struct OrdaTarget {
    adapter_path: PathBuf,
}

impl OrdaTarget {
    pub fn new(adapter_path: PathBuf) -> Self {
        Self { adapter_path }
    }
}

impl Target for OrdaTarget {
    fn name(&self) -> &str {
        "orda"
    }

    fn default_genome(&self) -> Genome {
        // Orda search space.
        //
        // Note: `payload.size` is a Choice whose sampled value is the *index*
        // into the choice array (0, 1, 2), which the adapter interprets as the
        // price-locality mode (0=uniform, 1=clustered, 2=zipfian). The choice
        // array values themselves are set to [1,2,3] only so that
        // `Genome::validate()` (which requires all choice values to be positive)
        // passes; the adapter keys off the index, not the stored value.
        Genome {
            topology: TopologyGenome {
                producers: ParameterSpec::Range {
                    min: 1.0,
                    max: 1000.0,
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
                    max: 10_000_000.0,
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
                    values: vec![json!(1), json!(2), json!(3)],
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
                    max: 1_000_000_000.0,
                },
                think_time_ns: ParameterSpec::LogUniform {
                    min: 1.0,
                    max: 1_000_000.0,
                },
            },
            faults: None,
        }
    }

    fn adapter_path(&self) -> PathBuf {
        self.adapter_path.clone()
    }

    fn genome_to_adapter_input(&self, genome: &SampledGenome) -> Value {
        // Keep the same base shape as the Norn adapter:
        // topology, traffic, payload, behavior, duration_sec, warmup_sec.
        //
        // Genome knob -> Orda simulator knob mapping (documented for the adapter):
        //   topology.producers      -> symbol count
        //   topology.consumers      -> matching-thread count
        //   traffic.rate            -> order ingress rate (ops/s)
        //   payload.size            -> price locality mode (Choice index:
        //                              0=uniform, 1=clustered, 2=zipfian)
        //   behavior.read_write_ratio -> add/cancel ratio (1.0=all adds,
        //                              0.0=all cancels)
        //   behavior.lifetime_ns    -> order lifetime
        //   behavior.think_time_ns  -> inter-order think time
        //   traffic.burstiness      -> price-level churn (0..1)
        //   traffic.correlation     -> market/limit mix (0..1)
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
            // v is the Choice index (0,1,2) into the locality modes.
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

        obj.into()
    }
}
