use crate::error::{Error, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
}

impl Config {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        if !data_dir.exists() {
            return Err(Error::InvalidPath(format!(
                "Data directory does not exist: {}",
                data_dir.display()
            )));
        }

        Ok(Self { data_dir })
    }

    pub fn default_data_dir() -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".claude"))
            .unwrap_or_else(|| PathBuf::from(".claude"))
    }

    pub fn transcripts_dir(&self) -> PathBuf {
        self.data_dir.join("transcripts")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data_dir.join("history.jsonl")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.data_dir.join("projects")
    }

    pub fn todos_dir(&self) -> PathBuf {
        self.data_dir.join("todos")
    }

    pub fn stats_file(&self) -> PathBuf {
        self.data_dir.join("stats-cache.json")
    }
}
