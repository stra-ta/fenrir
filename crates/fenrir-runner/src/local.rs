use anyhow::{Context, Result};
use fenrir_core::{
    Artifacts, HardwareCounters, MetricValue, Metrics, Observation, OsMetrics, RunStatus,
    SampledGenome,
};
use fenrir_targets::Target;
use serde_json::Value;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;
use ulid::Ulid;

pub async fn run_adapter(
    target: &dyn Target,
    genome: &SampledGenome,
    duration_sec: u64,
    seed: u64,
) -> Result<Observation> {
    let mut input = target.genome_to_adapter_input(genome);
    input["seed"] = serde_json::json!(seed);
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

    if let Some(app_metrics) = output.get("metrics").and_then(|m| m.get("application"))
        && let Some(obj) = app_metrics.as_object()
    {
        for (key, value) in obj {
            if let Some(f) = value.as_f64() {
                metrics
                    .application
                    .insert(key.clone(), MetricValue::Scalar(f));
            }
        }
    }

    if let Some(hw) = output.get("metrics").and_then(|m| m.get("hardware"))
        && let Some(obj) = hw.as_object()
    {
        for (key, value) in obj {
            if let Some(f) = value.as_f64() {
                metrics
                    .application
                    .insert(format!("hardware.{}", key), MetricValue::Scalar(f));
            }
        }
    }

    if let Some(os) = output.get("metrics").and_then(|m| m.get("os"))
        && let Some(obj) = os.as_object()
    {
        for (key, value) in obj {
            if let Some(f) = value.as_f64() {
                metrics
                    .application
                    .insert(format!("os.{}", key), MetricValue::Scalar(f));
            }
        }
    }

    // Ygg and other instrumentation producers may add a namespaced scalar
    // block.  Preserve the namespace in the metric name so objectives can
    // opt into it explicitly (`ygg.embedding_novelty`, for example).
    if let Some(ygg) = output.get("metrics").and_then(|m| m.get("ygg"))
        && let Some(obj) = ygg.as_object()
    {
        for (key, value) in obj {
            if let Some(f) = value.as_f64() {
                metrics
                    .application
                    .insert(format!("ygg.{key}"), MetricValue::Scalar(f));
                if key == "embedding_novelty" {
                    metrics.ygg_novelty = Some(f);
                }
            } else if key == "embedding" {
                if let Some(values) = value.as_array() {
                    let embedding: Vec<f32> = values
                        .iter()
                        .filter_map(Value::as_f64)
                        .map(|value| value as f32)
                        .collect();
                    if !embedding.is_empty() && embedding.len() == values.len() {
                        metrics.ygg_embedding = Some(embedding);
                    }
                }
            } else if key == "regime" {
                metrics.ygg_regime = value.as_str().map(ToString::to_string);
            }
        }
    }

    if let Some(path) = output
        .get("metrics")
        .and_then(|m| m.get("trace_path"))
        .and_then(Value::as_str)
        .or_else(|| {
            output
                .get("artifacts")
                .and_then(|a| a.get("trace_path"))
                .and_then(Value::as_str)
        })
    {
        metrics.trace_path = Some(path.to_string());
    }

    if let Some(metadata) = output.get("metadata").and_then(Value::as_object) {
        for (key, value) in metadata {
            if let Some(value) = value.as_str() {
                metrics.metadata.insert(key.clone(), value.to_string());
            } else if let Some(value) = value.as_i64() {
                metrics.metadata.insert(key.clone(), value.to_string());
            } else if let Some(value) = value.as_f64() {
                metrics.metadata.insert(key.clone(), value.to_string());
            } else if let Some(value) = value.as_bool() {
                metrics.metadata.insert(key.clone(), value.to_string());
            }
        }
    }

    metrics.hardware = HardwareCounters::default();
    metrics.os = OsMetrics::default();

    Ok(metrics)
}
