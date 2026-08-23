use fenrir_search::Evaluation;
use anyhow::{Context, Result};
use serde_json;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub struct CampaignStore {
    jsonl_file: Option<BufWriter<File>>,
    jsonl_path: PathBuf,
}

impl CampaignStore {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path).context("Failed to create campaign directory")?;

        let jsonl_path = path.join("evaluations.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)
            .context("Failed to open JSONL file")?;
        let jsonl_file = Some(BufWriter::new(file));

        Ok(Self { jsonl_file, jsonl_path })
    }

    pub fn append(&mut self, eval: &Evaluation) -> Result<()> {
        if let Some(writer) = &mut self.jsonl_file {
            let line = serde_json::to_string(eval)?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<Evaluation>> {
        Self::load_from_jsonl(&self.jsonl_path)
    }

    pub fn load_from_jsonl<P: AsRef<Path>>(path: P) -> Result<Vec<Evaluation>> {
        let file = File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut evaluations = Vec::new();

        for line in std::io::BufRead::lines(reader) {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Evaluation>(trimmed) {
                Ok(eval) => evaluations.push(eval),
                // Skip malformed / partial lines (e.g. a truncated final record)
                // instead of failing the whole campaign load.
                Err(e) => {
                    eprintln!("warning: skipping unparseable evaluation line: {e}");
                }
            }
        }

        Ok(evaluations)
    }
}