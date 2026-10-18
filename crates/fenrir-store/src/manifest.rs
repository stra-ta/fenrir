//! Machine-readable campaign provenance.
//!
//! A result without the command, source revision, and machine context is not a
//! reproducible result.  The manifest is deliberately JSON so it can be read
//! by Kiln, Ygg, and small shell/Python tools without linking Fenrir.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignManifest {
    pub schema_version: u32,
    pub campaign_id: String,
    pub target: String,
    pub optimizer: String,
    pub budget: usize,
    pub duration_sec: u64,
    pub parallel: usize,
    pub seed: u64,
    pub backend: String,
    pub command: Vec<String>,
    pub commit_sha: String,
    pub dirty_tree: Option<bool>,
    pub compiler: String,
    pub kernel: String,
    pub cpu: String,
    pub architecture: String,
    pub build_type: String,
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub path: String,
    pub sha256: Option<String>,
    pub bytes: u64,
}

impl CampaignManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        target: impl Into<String>,
        optimizer: impl Into<String>,
        budget: usize,
        duration_sec: u64,
        parallel: usize,
        seed: u64,
        backend: impl Into<String>,
        root: &Path,
    ) -> Self {
        let command = std::env::args().collect();
        let campaign_id = ulid::Ulid::new().to_string();
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            campaign_id,
            target: target.into(),
            optimizer: optimizer.into(),
            budget,
            duration_sec,
            parallel,
            seed,
            backend: backend.into(),
            command,
            commit_sha: git_output(root, &["rev-parse", "HEAD"]).unwrap_or_default(),
            dirty_tree: git_dirty(root),
            compiler: command_output("rustc", &["--version"]).unwrap_or_default(),
            kernel: command_output("uname", &["-srv"]).unwrap_or_default(),
            cpu: cpu_model(),
            architecture: std::env::consts::ARCH.to_string(),
            build_type: std::env::var("FENRIR_BUILD_TYPE").unwrap_or_else(|_| "unknown".into()),
            dependencies: BTreeMap::new(),
            artifacts: Vec::new(),
        }
    }

    pub fn add_artifact(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("failed to stat artifact {}", path.display()))?;
        let record = ArtifactRecord {
            path: path.display().to_string(),
            sha256: sha256(path),
            bytes: metadata.len(),
        };
        if let Some(existing) = self
            .artifacts
            .iter_mut()
            .find(|item| item.path == record.path)
        {
            *existing = record;
        } else {
            self.artifacts.push(record);
        }
        Ok(())
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to publish campaign manifest {}", path.display()))?;
        Ok(())
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_dirty(root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

fn cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        command_output("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_default()
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
            if let Some(model) = text
                .lines()
                .find_map(|line| line.strip_prefix("model name\t: "))
            {
                return model.to_string();
            }
            if let Some(model) = text
                .lines()
                .find_map(|line| line.strip_prefix("Hardware\t: "))
            {
                return model.to_string();
            }
        }
        String::new()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        String::new()
    }
}

fn sha256(path: &Path) -> Option<String> {
    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_and_records_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("result.json");
        std::fs::write(&artifact, b"{}\n").unwrap();
        let mut manifest =
            CampaignManifest::capture("weir", "bayesian", 4, 1, 1, 7, "local", dir.path());
        manifest.add_artifact(&artifact).unwrap();
        let path = dir.path().join("results/manifest.json");
        manifest.write_atomic(&path).unwrap();
        let parsed: CampaignManifest =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(parsed.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(parsed.artifacts[0].bytes, 3);
        assert!(parsed.artifacts[0].sha256.is_some());
    }
}
