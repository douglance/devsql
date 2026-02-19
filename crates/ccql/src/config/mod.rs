use crate::error::{Error, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    codex_data_dir: PathBuf,
}

impl Config {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        let codex_data_dir = Self::resolve_codex_data_dir();
        Self::new_with_codex_data_dir(data_dir, codex_data_dir)
    }

    pub fn new_with_codex_data_dir(data_dir: PathBuf, codex_data_dir: PathBuf) -> Result<Self> {
        if !data_dir.exists() {
            return Err(Error::InvalidPath(format!(
                "Data directory does not exist: {}",
                data_dir.display()
            )));
        }

        Ok(Self {
            data_dir,
            codex_data_dir,
        })
    }

    pub fn default_data_dir() -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".claude"))
            .unwrap_or_else(|| PathBuf::from(".claude"))
    }

    pub fn default_codex_data_dir() -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".codex"))
            .unwrap_or_else(|| PathBuf::from(".codex"))
    }

    fn resolve_codex_data_dir() -> PathBuf {
        std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(Self::default_codex_data_dir)
    }

    pub fn transcripts_dir(&self) -> PathBuf {
        self.data_dir.join("transcripts")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data_dir.join("history.jsonl")
    }

    pub fn codex_data_dir(&self) -> PathBuf {
        self.codex_data_dir.clone()
    }

    pub fn jhistory_file(&self) -> PathBuf {
        self.codex_data_dir().join("history.jsonl")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jhistory_path_uses_injected_codex_dir() {
        let temp = tempfile::tempdir().expect("temp dir");
        let codex_dir = temp.path().join("my-codex");
        std::fs::create_dir_all(&codex_dir).expect("create codex dir");

        let config = Config::new_with_codex_data_dir(temp.path().to_path_buf(), codex_dir.clone())
            .expect("config");

        assert_eq!(config.codex_data_dir(), codex_dir);
        assert_eq!(config.jhistory_file(), codex_dir.join("history.jsonl"));
    }
}
