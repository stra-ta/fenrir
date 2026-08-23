use crate::Target;
use fenrir_core::{Genome, SampledGenome};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct WeirTarget {
    adapter_path: PathBuf,
}

impl WeirTarget {
    pub fn new(adapter_path: PathBuf) -> Self {
        Self { adapter_path }
    }
}

impl Target for WeirTarget {
    fn name(&self) -> &str {
        "weir"
    }

    fn default_genome(&self) -> Genome {
        // Weir is a durable event gateway (WR01 frames, WAL with group commit).
        // Knobs map onto the Norn-shaped genome document so the C++ adapter can
        // reuse the same JSON interpreter:
        //   topology.producers       -> connection count
        //   topology.consumers       -> pipeline / WAL group-commit window depth
        //   traffic.rate             -> rate limit (ops/s)
        //   payload.size             -> frame payload bytes
        //   behavior.read_write_ratio-> write fraction (WAL sync cost)
        //   behavior.lifetime_ns     -> connection lifetime
        //   behavior.think_time_ns   -> inter-request think time
        //   traffic.burstiness       -> traffic burstiness (tail amplifier)
        //   traffic.correlation      -> payload-size correlation
        //
        // Built via serde_json::from_value to avoid depending on the non-exported
        // inner genome structs/enums. The `type` tags match fenrir-core's
        // ParameterSpec serde tags ("Range", "Choice", "LogUniform").
        let doc = json!({
            "topology": {
                "producers": { "type": "Range", "min": 1.0, "max": 1000.0, "log": false },
                "consumers": { "type": "Range", "min": 1.0, "max": 1000.0, "log": false },
                "affinity": "core"
            },
            "traffic": {
                "arrival_process": "poisson",
                "rate": { "type": "LogUniform", "min": 10_000.0, "max": 10_000_000.0 },
                "burstiness": { "type": "Range", "min": 0.0, "max": 1.0, "log": false },
                "correlation": { "type": "Range", "min": 0.0, "max": 1.0, "log": false }
            },
            "payload": {
                "size_distribution": "fixed",
                "size": { "type": "Choice", "values": [64, 256, 1024, 4096, 16384, 65536] },
                "key_distribution": "uniform"
            },
            "behavior": {
                "read_write_ratio": { "type": "Range", "min": 0.0, "max": 1.0, "log": false },
                "lifetime_ns": { "type": "LogUniform", "min": 1_000.0, "max": 10_000_000.0 },
                "think_time_ns": { "type": "LogUniform", "min": 1.0, "max": 1_000_000.0 }
            },
            "faults": null
        });
        serde_json::from_value(doc).expect("weir default genome should deserialize")
    }

    fn adapter_path(&self) -> PathBuf {
        self.adapter_path.clone()
    }

    fn genome_to_adapter_input(&self, genome: &SampledGenome) -> Value {
        // Same document shape as Norn so the C++ side reuses its interpreter.
        let mut obj = serde_json::Map::new();

        // Topology (producers -> connections, consumers -> window/pipeline depth)
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

        // Traffic (rate -> rate limit ops/s, burstiness/correlation -> shaping)
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

        // Payload (size -> frame payload bytes).
        // NOTE: for Choice params, SampledGenome stores the *index* into the
        // choice list (see fenrir-core SearchSpace::random_sample), not the
        // literal value. Weir's payload.size choice is an explicit byte table,
        // so map the index back to the real frame size for the C++ side.
        let size_choices: [u64; 6] = [64, 256, 1024, 4096, 16384, 65536];
        let mut payload = serde_json::Map::new();
        payload.insert("size_distribution".into(), json!("fixed"));
        if let Some(v) = genome.values.get("payload.size") {
            let idx = (*v as i64).clamp(0, (size_choices.len() - 1) as i64) as usize;
            payload.insert("size".into(), json!(size_choices[idx]));
        }
        payload.insert("key_distribution".into(), json!("uniform"));
        obj.insert("payload".into(), payload.into());

        // Behavior (read_write_ratio -> write fraction, lifetime/think -> timing)
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
