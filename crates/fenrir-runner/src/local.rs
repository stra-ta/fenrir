use fenrir_core::{Observation, SampledGenome, Metrics, HardwareCounters, OsMetrics, Artifacts, RunStatus, MetricValue};
use fenrir_targets::Target;
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;
use ulid::Ulid;

pub async fn run_adapter(
    target: &dyn Target,
    genome: &SampledGenome,
    duration_sec: u64,
) -> Result<Observation> {
    let input = target.genome_to_adapter_input(genome);
    let input_json = serde_json::to_string(&input)?;
    let adapter_path = target.adapter_path();

    let mut cmd = Command::new(&adapter_path);
    cmd.arg("--genome")
        .arg("-")
        .arg("--duration")
        .arg(duration_sec.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn adapter: {:?}", adapter_path))?;

    // Write genome to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input_json.as_bytes()).await?;
        stdin.flush().await?;
        drop(stdin);
    }

    let start = Instant::now();
    let output = child
        .wait_with_output()
        .await
        .with_context(|| "Adapter process failed")?;
    let duration = start.elapsed();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Adapter failed with status {:?}: {}",
            output.status.code(),
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let adapter_output: Value = serde_json::from_str(&stdout)
        .with_context(|| format!("Failed to parse adapter output: {}", stdout))?;

    // Extract metrics from adapter output and construct Observation
    let metrics = extract_metrics(&adapter_output)?;

    let observation = Observation {
        run_id: Ulid::new().to_string(),
        genome: genome.clone(),
        metrics,
        artifacts: Artifacts::default(),
        status: RunStatus::Success,
        timestamp_ns: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
        duration_ns: duration.as_nanos() as u64,
    };

    Ok(observation)
}

/// Parse an adapter's stdout JSON (`{"metrics": {...}}`) into Fenrir metrics.
///
/// `metrics.application` is read as a flat scalar map. `metrics.hardware` and
/// `metrics.os` values that are not finite numbers (e.g. JSON `null`) are
/// skipped, so adapters may omit or null them.
pub(crate) fn extract_metrics(output: &Value) -> Result<Metrics> {
    let mut metrics = Metrics::default();

    if let Some(app_metrics) = output.get("metrics").and_then(|m| m.get("application")) {
        if let Some(obj) = app_metrics.as_object() {
            for (key, value) in obj {
                if let Some(f) = value.as_f64() {
                    metrics.application.insert(key.clone(), MetricValue::Scalar(f));
                }
            }
        }
    }

    if let Some(hw) = output.get("metrics").and_then(|m| m.get("hardware")) {
        if let Some(obj) = hw.as_object() {
            for (key, value) in obj {
                if let Some(f) = value.as_f64() {
                    metrics
                        .application
                        .insert(format!("hardware.{}", key), MetricValue::Scalar(f));
                }
            }
        }
    }

    if let Some(os) = output.get("metrics").and_then(|m| m.get("os")) {
        if let Some(obj) = os.as_object() {
            for (key, value) in obj {
                if let Some(f) = value.as_f64() {
                    metrics
                        .application
                        .insert(format!("os.{}", key), MetricValue::Scalar(f));
                }
            }
        }
    }

    metrics.hardware = HardwareCounters::default();
    metrics.os = OsMetrics::default();

    Ok(metrics)
}
