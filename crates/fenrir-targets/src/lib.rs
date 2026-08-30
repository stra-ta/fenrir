use fenrir_core::{Genome, SampledGenome, SearchSpace};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub mod loki;
pub mod norn;
pub mod orda;
pub mod weir;

/// A Fenrir evaluation target (e.g. Norn, Weir, Loki, Orda).
///
/// Each target owns:
/// - its default genome (defines the search space),
/// - the mapping from a sampled genome to the JSON the target's C++ adapter
///   expects on stdin,
/// - the path to the compiled adapter binary.
pub trait Target: Send + Sync {
    fn name(&self) -> &str;

    fn default_genome(&self) -> Genome;

    fn search_space(&self) -> SearchSpace {
        self.default_genome().search_space()
    }

    /// Path to the compiled adapter binary:
    /// `targets/{name}/build/{name}_fenrir_adapter`.
    fn adapter_path(&self) -> PathBuf;

    /// Map a sampled genome to the JSON document the adapter reads on stdin.
    fn genome_to_adapter_input(&self, genome: &SampledGenome) -> Value;
}

/// Target wrapper used for explicit adapter overrides.
///
/// Keeping the override at the target boundary makes `--adapter` effective
/// for both local and Kiln backends without teaching runners about each target
/// implementation.
struct AdapterOverride {
    inner: Box<dyn Target>,
    adapter_path: PathBuf,
}

impl Target for AdapterOverride {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn default_genome(&self) -> Genome {
        self.inner.default_genome()
    }

    fn search_space(&self) -> SearchSpace {
        self.inner.search_space()
    }

    fn adapter_path(&self) -> PathBuf {
        self.adapter_path.clone()
    }

    fn genome_to_adapter_input(&self, genome: &SampledGenome) -> Value {
        self.inner.genome_to_adapter_input(genome)
    }
}

/// Replace a resolved target's adapter path while preserving its genome and
/// input mapping.
pub fn with_adapter_path(target: Box<dyn Target>, adapter_path: PathBuf) -> Box<dyn Target> {
    Box::new(AdapterOverride {
        inner: target,
        adapter_path,
    })
}

/// Resolve a target by name. Returns `None` for unknown targets.
///
/// The adapter binary path is resolved in priority order:
/// 1. `FENRIR_ADAPTER_DIR` environment variable (directory holding the built
///    adapters).
/// 2. A workspace checkout at `<root>/targets/{name}/build/...`, where
///    `<root>` is derived from `CARGO_MANIFEST_DIR` (this crate lives at
///    `<root>/crates/fenrir-targets`).
pub fn get_target(name: &str) -> Option<Box<dyn Target>> {
    let adapter = if let Ok(dir) = std::env::var("FENRIR_ADAPTER_DIR") {
        PathBuf::from(dir).join(format!("{name}_fenrir_adapter"))
    } else {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        // crate lives at <root>/crates/fenrir-targets
        let root = Path::new(&manifest).join("..").join("..");
        root.join("targets")
            .join(name)
            .join("build")
            .join(format!("{name}_fenrir_adapter"))
    };
    match name {
        "norn" => Some(Box::new(norn::NornTarget::new(adapter))),
        "weir" => Some(Box::new(weir::WeirTarget::new(adapter))),
        "loki" => Some(Box::new(loki::LokiTarget::new(adapter))),
        "orda" => Some(Box::new(orda::OrdaTarget::new(adapter))),
        _ => None,
    }
}
