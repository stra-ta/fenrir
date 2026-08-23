use crate::Target;
use fenrir_core::{Genome, SampledGenome};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct NornTarget {
    adapter_path: PathBuf,
}

impl NornTarget {
    pub fn new(adapter_path: PathBuf) -> Self {
        Self { adapter_path }
    }
}

impl Target for NornTarget {
    fn name(&self) -> &str {
        "norn"
    }

    fn default_genome(&self) -> Genome {
        Genome::default_norn()
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

        obj.into()
    }
}
