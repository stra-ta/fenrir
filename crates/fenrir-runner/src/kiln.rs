//! Kiln execution backend (Fenrir → Kiln experiment runner).
//!
//! Each Fenrir genome evaluation is mapped onto a single-run Kiln experiment
//! that invokes the target's adapter binary and captures its stdout. The
//! captured adapter stdout is parsed back into a Fenrir `Observation` using
//! the same extractor as the local backend (`crate::local::extract_metrics`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use fenrir_core::{Artifacts, Observation, RunStatus, SampledGenome};
use fenrir_targets::Target;
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::process::Command;
use ulid::Ulid;

/// Locate the `kiln` binary, checking in priority order:
/// 1. `KILN_BIN` environment variable.
/// 2. The local debug build at the known project path.
/// 3. `kiln` resolved from `PATH`.
fn locate_kiln() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("KILN_BIN") {
        return Some(PathBuf::from(p));
    }
    let debug = PathBuf::from("/Users/nguyenhuyvu/projects/kiln/target/debug/kiln");
    if debug.exists() {
        return Some(debug);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("kiln");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Read exactly one subdirectory of `parent`, erroring if the count differs.
fn single_subdir(parent: &Path) -> Result<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(parent)
        .with_context(|| format!("failed to read directory {}", parent.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    match entries.len() {
        1 => Ok(entries.into_iter().next().unwrap().path()),
        n => bail!(
            "expected exactly one run directory under {}, found {}",
            parent.display(),
            n
        ),
    }
}

pub async fn run_with_kiln(
    target: &dyn Target,
    genome: &SampledGenome,
    duration_sec: u64,
) -> Result<Observation> {
    // 1. Locate the kiln binary.
    let kiln_bin = locate_kiln().ok_or_else(|| {
        anyhow::anyhow!(
            "kiln binary not found; build it with `cd /Users/nguyenhuyvu/projects/kiln && cargo build` or set KILN_BIN"
        )
    })?;

    // 2. Write the genome JSON to a temp file (adapter reads it via --genome).
    let input = target.genome_to_adapter_input(genome);
    let input_json =
        serde_json::to_string(&input).context("failed to serialize genome to adapter input")?;
    let mut genome_file = NamedTempFile::new().context("failed to create genome temp file")?;
    genome_file
        .write_all(input_json.as_bytes())
        .context("failed to write genome temp file")?;
    genome_file.flush().context("failed to flush genome temp file")?;
    let genome_path = genome_file.path().to_path_buf();

    // 3. Generate a single-run Kiln spec in a temp file.
    //
    // An empty matrix expands to exactly one parameter set (one run). We
    // disable git and environment capture so the run does not depend on the
    // host being inside a git repository. `exclusive_machine = false` keeps
    // the run self-contained within the isolated root directory.
    let adapter_path = target.adapter_path();
    let command = format!(
        "{} --genome {} --duration {}",
        adapter_path.display(),
        genome_path.display(),
        duration_sec
    );
    let timeout_sec = duration_sec + 60;

    let spec = format!(
        "schema = 1\n\
         name = \"fenrir-kiln-eval\"\n\
         \n\
         [run]\n\
         command = \"{command}\"\n\
         timeout = \"{timeout_sec}s\"\n\
         \n\
         [capture]\n\
         git = false\n\
         environment = false\n\
         stdout = true\n\
         stderr = true\n\
         \n\
         [execution]\n\
         exclusive_machine = false\n"
    );

    let mut spec_file = NamedTempFile::new().context("failed to create spec temp file")?;
    spec_file
        .write_all(spec.as_bytes())
        .context("failed to write spec temp file")?;
    spec_file.flush().context("failed to flush spec temp file")?;
    let spec_path = spec_file.path().to_path_buf();

    // Isolated output root so we can deterministically discover the run dir.
    let kiln_root = tempfile::tempdir().context("failed to create kiln root temp dir")?;
    let root_path = kiln_root.path().to_path_buf();

    // 4. Run `kiln run <spec> --root <root>`.
    let output = Command::new(&kiln_bin)
        .arg("run")
        .arg(&spec_path)
        .arg("--root")
        .arg(&root_path)
        .output()
        .await
        .with_context(|| format!("failed to spawn kiln at {}", kiln_bin.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "kiln run failed (exit {:?}):\nSTDERR:\n{}\nSTDOUT:\n{}",
            output.status.code(),
            stderr,
            stdout
        );
    }

    // 5. Read the captured adapter stdout artifact.
    //
    // Layout: <root>/.kiln/experiments/<experiment_id>/runs/<run_id>/stdout.log
    // A single-run spec yields exactly one experiment and one run directory.
    let experiments_dir = root_path.join(".kiln").join("experiments");
    let campaign_dir = single_subdir(&experiments_dir)
        .context("failed to locate kiln experiment output directory")?;
    let runs_dir = campaign_dir.join("runs");
    let run_dir = single_subdir(&runs_dir).context("failed to locate kiln run output directory")?;
    let stdout_path = run_dir.join("stdout.log");

    let stdout = std::fs::read_to_string(&stdout_path).with_context(|| {
        format!(
            "failed to read captured adapter stdout at {}",
            stdout_path.display()
        )
    })?;

    let adapter_output: Value = serde_json::from_str(&stdout)
        .with_context(|| format!("failed to parse adapter stdout JSON:\n{stdout}"))?;

    let metrics = crate::local::extract_metrics(&adapter_output)?;

    // 6. Build and return the Observation.
    Ok(Observation {
        run_id: Ulid::new().to_string(),
        genome: genome.clone(),
        metrics,
        artifacts: Artifacts::default(),
        status: RunStatus::Success,
        timestamp_ns: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
        duration_ns: duration_sec * 1_000_000_000,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn runs_norn_via_kiln() {
        let target = fenrir_targets::get_target("norn").expect("norn target should exist");

        // Build a representative sampled genome without pulling in `rand`
        // (this crate does not declare it). Missing keys would simply fall
        // back to the adapter's built-in defaults, so any valid subset runs.
        let mut values = HashMap::new();
        values.insert("topology.producers".to_string(), 2.0);
        values.insert("topology.consumers".to_string(), 2.0);
        values.insert("topology.affinity".to_string(), 1.0);
        values.insert("traffic.rate".to_string(), 1_000_000.0);
        values.insert("traffic.burstiness".to_string(), 0.0);
        values.insert("traffic.correlation".to_string(), 0.0);
        values.insert("payload.size".to_string(), 192.0);
        values.insert("behavior.read_write_ratio".to_string(), 0.5);
        values.insert("behavior.lifetime_ns".to_string(), 10_000.0);
        values.insert("behavior.think_time_ns".to_string(), 0.0);
        let genome = SampledGenome {
            values,
            target_config: serde_json::Value::Null,
        };

        let obs = run_with_kiln(&*target, &genome, 3)
            .await
            .expect("kiln run should succeed");

        let tp = obs.metrics.get_scalar("throughput_ops_s");
        assert!(
            tp.is_some(),
            "expected throughput_ops_s metric, got application metrics: {:?}",
            obs.metrics.application
        );
        let tp = tp.unwrap();
        assert!(tp.is_finite(), "throughput must be finite, got {tp}");
    }
}
