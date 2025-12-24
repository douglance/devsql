use crate::config::Config;
use crate::error::Result;
use crate::streaming;
use std::path::PathBuf;
use walkdir::WalkDir;

pub struct TranscriptDataSource {
    config: Config,
}

impl TranscriptDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let transcripts_dir = self.config.transcripts_dir();
        if !transcripts_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = Vec::new();
        for entry in WalkDir::new(&transcripts_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) {
                    let metadata = std::fs::metadata(path).ok();
                    sessions.push(SessionInfo {
                        session_id: session_id.to_string(),
                        path: path.to_path_buf(),
                        size_bytes: metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                        modified: metadata.and_then(|m| m.modified().ok()),
                    });
                }
            }
        }

        sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        Ok(sessions)
    }

    pub async fn load_session(&self, session_id: &str) -> Result<Vec<serde_json::Value>> {
        let path = self.config.transcripts_dir().join(format!("{}.jsonl", session_id));
        streaming::read_jsonl_raw(path).await
    }

    pub async fn load_all_sessions(&self) -> Result<Vec<(String, Vec<serde_json::Value>)>> {
        let sessions = self.list_sessions()?;
        let mut all = Vec::new();

        for session in sessions {
            match streaming::read_jsonl_raw(&session.path).await {
                Ok(entries) => all.push((session.session_id, entries)),
                Err(e) => tracing::debug!("Failed to load session {}: {}", session.session_id, e),
            }
        }

        Ok(all)
    }

    pub async fn search_in_sessions(&self, pattern: &regex::Regex) -> Result<Vec<SearchResult>> {
        let sessions = self.list_sessions()?;
        let mut results = Vec::new();

        for session in sessions {
            match streaming::read_jsonl_raw(&session.path).await {
                Ok(entries) => {
                    for (idx, entry) in entries.iter().enumerate() {
                        let entry_str = serde_json::to_string(entry).unwrap_or_default();
                        if pattern.is_match(&entry_str) {
                            results.push(SearchResult {
                                session_id: session.session_id.clone(),
                                entry_index: idx,
                                entry: entry.clone(),
                            });
                        }
                    }
                }
                Err(e) => tracing::debug!("Failed to load session {}: {}", session.session_id, e),
            }
        }

        Ok(results)
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl SessionInfo {
    pub fn formatted_time(&self) -> String {
        self.modified
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn size_human(&self) -> String {
        if self.size_bytes < 1024 {
            format!("{} B", self.size_bytes)
        } else if self.size_bytes < 1024 * 1024 {
            format!("{:.1} KB", self.size_bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", self.size_bytes as f64 / (1024.0 * 1024.0))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub session_id: String,
    pub entry_index: usize,
    pub entry: serde_json::Value,
}
