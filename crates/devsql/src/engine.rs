//! Unified query engine that combines ccql and vcsql data

use crate::{Error, Result};
use chrono::DateTime;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::path::PathBuf;

/// Unified query engine that loads data from both Claude Code and Git
pub struct UnifiedEngine {
    conn: Connection,
    claude_data_dir: PathBuf,
    codex_data_dir: PathBuf,
    git_repo_path: PathBuf,
}

impl UnifiedEngine {
    /// Create a new unified engine
    pub fn new(claude_data_dir: PathBuf, git_repo_path: PathBuf) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let codex_data_dir = default_codex_data_dir();

        // Register custom DATE function that handles both epoch ms and ISO dates
        conn.create_scalar_function(
            "DATE",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let value: String = ctx.get(0)?;
                Ok(normalize_date(&value))
            },
        )?;

        Ok(Self {
            conn,
            claude_data_dir,
            codex_data_dir,
            git_repo_path,
        })
    }

    /// Load Claude Code tables needed for the query
    pub fn load_claude_tables(&mut self, tables: &[&str]) -> Result<()> {
        for table in tables {
            match *table {
                "history" => self.load_history()?,
                "jhistory" | "codex_history" => self.load_jhistory()?,
                "transcripts" => self.load_transcripts()?,
                "todos" => self.load_todos()?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Load Git tables needed for the query
    pub fn load_git_tables(&mut self, tables: &[&str]) -> Result<()> {
        for table in tables {
            match *table {
                "commits" => self.load_commits()?,
                "diffs" => self.load_diffs()?,
                "diff_files" => self.load_diff_files()?,
                "branches" => self.load_branches()?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Execute a SQL query and return results as JSON values
    pub fn query(&self, sql: &str) -> Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(sql)?;
        let column_names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let rows = stmt.query_map([], |row| {
            let mut obj = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                // Try different types in order
                let value: Value = if let Ok(v) = row.get::<_, i64>(i) {
                    Value::Number(v.into())
                } else if let Ok(v) = row.get::<_, f64>(i) {
                    serde_json::Number::from_f64(v)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                } else if let Ok(v) = row.get::<_, String>(i) {
                    Value::String(v)
                } else {
                    Value::Null
                };
                obj.insert(name.clone(), value);
            }
            Ok(Value::Object(obj))
        })?;

        let results: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    // --- Table loaders ---

    fn load_history(&mut self) -> Result<()> {
        // Create history table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                rowid INTEGER PRIMARY KEY,
                display TEXT,
                timestamp TEXT,
                project TEXT
            )",
            [],
        )?;

        // Load from ccql's history.jsonl
        let history_path = self.claude_data_dir.join("history.jsonl");
        if history_path.exists() {
            let content = std::fs::read_to_string(&history_path)?;
            for line in content.lines() {
                if let Ok(entry) = serde_json::from_str::<Value>(line) {
                    let display = entry.get("display").and_then(|v| v.as_str()).unwrap_or("");
                    let timestamp = entry
                        .get("timestamp")
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let project = entry.get("project").and_then(|v| v.as_str()).unwrap_or("");

                    self.conn.execute(
                        "INSERT INTO history (display, timestamp, project) VALUES (?1, ?2, ?3)",
                        params![display, timestamp, project],
                    )?;
                }
            }
        }

        Ok(())
    }

    fn load_transcripts(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS transcripts (
                rowid INTEGER PRIMARY KEY,
                type TEXT,
                content TEXT,
                tool_name TEXT,
                session_id TEXT
            )",
            [],
        )?;
        // TODO: Load from transcripts/*.jsonl
        Ok(())
    }

    fn load_jhistory(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS jhistory (
                rowid INTEGER PRIMARY KEY,
                session_id TEXT,
                ts INTEGER,
                text TEXT,
                display TEXT,
                timestamp INTEGER
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE VIEW IF NOT EXISTS codex_history AS SELECT * FROM jhistory",
            [],
        )?;

        let jhistory_path = self.codex_data_dir.join("history.jsonl");
        if !jhistory_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&jhistory_path)?;
        for line in content.lines() {
            if let Ok(entry) = serde_json::from_str::<Value>(line) {
                let text = entry
                    .get("text")
                    .or_else(|| entry.get("display"))
                    .and_then(json_value_as_string)
                    .unwrap_or_default();

                let session_id = entry
                    .get("session_id")
                    .or_else(|| entry.get("sessionId"))
                    .and_then(json_value_as_string)
                    .unwrap_or_default();

                let ts = entry
                    .get("ts")
                    .and_then(json_number_as_i64)
                    .or_else(|| {
                        entry
                            .get("timestamp")
                            .and_then(json_number_as_i64)
                            .map(normalize_ts_seconds)
                    })
                    .unwrap_or(0);

                let timestamp = ts.saturating_mul(1000);

                self.conn.execute(
                    "INSERT INTO jhistory (session_id, ts, text, display, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session_id, ts, text.clone(), text, timestamp],
                )?;
            }
        }

        Ok(())
    }

    fn load_todos(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS todos (
                rowid INTEGER PRIMARY KEY,
                content TEXT,
                status TEXT
            )",
            [],
        )?;
        // TODO: Load from todos/*.json
        Ok(())
    }

    fn load_commits(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS commits (
                id TEXT PRIMARY KEY,
                short_id TEXT,
                author_name TEXT,
                author_email TEXT,
                authored_at TEXT,
                summary TEXT,
                message TEXT,
                is_merge INTEGER
            )",
            [],
        )?;

        // Use git2 to load commits
        if let Ok(repo) = git2::Repository::open(&self.git_repo_path) {
            let mut revwalk = repo.revwalk().map_err(|e| Error::Vcsql(e.to_string()))?;
            revwalk.push_head().ok();

            for oid in revwalk.filter_map(|r| r.ok()) {
                if let Ok(commit) = repo.find_commit(oid) {
                    let id = commit.id().to_string();
                    let short_id = &id[..7.min(id.len())];
                    let author = commit.author();
                    let author_name = author.name().unwrap_or("");
                    let author_email = author.email().unwrap_or("");
                    let time = commit.time();
                    let authored_at = format_git_time(time.seconds());
                    let summary = commit.summary().unwrap_or("");
                    let message = commit.message().unwrap_or("");
                    let is_merge = if commit.parent_count() > 1 { 1 } else { 0 };

                    self.conn.execute(
                        "INSERT OR IGNORE INTO commits VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            id,
                            short_id,
                            author_name,
                            author_email,
                            authored_at,
                            summary,
                            message,
                            is_merge
                        ],
                    )?;
                }
            }
        }

        Ok(())
    }

    fn load_diffs(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS diffs (
                commit_id TEXT PRIMARY KEY,
                files_changed INTEGER,
                insertions INTEGER,
                deletions INTEGER
            )",
            [],
        )?;
        // TODO: Implement diff stats loading
        Ok(())
    }

    fn load_diff_files(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS diff_files (
                commit_id TEXT,
                path TEXT,
                insertions INTEGER,
                deletions INTEGER
            )",
            [],
        )?;
        // TODO: Implement per-file diff loading
        Ok(())
    }

    fn load_branches(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS branches (
                name TEXT PRIMARY KEY,
                target TEXT,
                is_head INTEGER,
                is_remote INTEGER
            )",
            [],
        )?;

        if let Ok(repo) = git2::Repository::open(&self.git_repo_path) {
            if let Ok(branches) = repo.branches(None) {
                for branch in branches.filter_map(|b| b.ok()) {
                    let (branch, branch_type) = branch;
                    let name = branch.name().ok().flatten().unwrap_or("");
                    let target = branch
                        .get()
                        .target()
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    let is_head = if branch.is_head() { 1 } else { 0 };
                    let is_remote = if branch_type == git2::BranchType::Remote {
                        1
                    } else {
                        0
                    };

                    self.conn.execute(
                        "INSERT OR IGNORE INTO branches VALUES (?1, ?2, ?3, ?4)",
                        params![name, target, is_head, is_remote],
                    )?;
                }
            }
        }

        Ok(())
    }
}

/// Normalize dates from various formats to YYYY-MM-DD
fn normalize_date(value: &str) -> String {
    // Epoch milliseconds (13 digits)
    if value.chars().all(|c| c.is_ascii_digit()) && value.len() >= 13 {
        if let Ok(ms) = value.parse::<i64>() {
            let secs = ms / 1000;
            if let Some(dt) = DateTime::from_timestamp(secs, 0) {
                return dt.format("%Y-%m-%d").to_string();
            }
        }
    }

    // Epoch seconds (10 digits)
    if value.chars().all(|c| c.is_ascii_digit()) && value.len() >= 10 {
        if let Ok(secs) = value.parse::<i64>() {
            if let Some(dt) = DateTime::from_timestamp(secs, 0) {
                return dt.format("%Y-%m-%d").to_string();
            }
        }
    }

    // ISO date string - just take first 10 chars
    if value.len() >= 10 {
        return value[..10].to_string();
    }

    value.to_string()
}

/// Format git timestamp to ISO date
fn format_git_time(secs: i64) -> String {
    DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

fn default_codex_data_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn json_number_as_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_u64()
            .and_then(|n| i64::try_from(n).ok())
            .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

fn json_value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn normalize_ts_seconds(raw_ts: i64) -> i64 {
    if raw_ts > 10_000_000_000 {
        raw_ts / 1000
    } else {
        raw_ts
    }
}

fn query_mentions_table(query_upper: &str, table_name: &str) -> bool {
    let table_upper = table_name.to_uppercase();
    query_upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|token| token == table_upper)
}

/// Detect which tables are needed from a SQL query
pub fn detect_tables(query: &str) -> (Vec<String>, Vec<String>) {
    let query_upper = query.to_uppercase();

    let claude_tables = [
        "history",
        "jhistory",
        "codex_history",
        "transcripts",
        "todos",
        "stats",
    ];
    let git_tables = [
        "commits",
        "commit_parents",
        "branches",
        "tags",
        "refs",
        "stashes",
        "reflog",
        "diffs",
        "diff_files",
        "blame",
        "config",
        "remotes",
        "submodules",
        "status",
        "worktrees",
        "hooks",
        "notes",
    ];

    let needed_claude: Vec<String> = claude_tables
        .iter()
        .filter(|t| query_mentions_table(&query_upper, t))
        .map(|s| s.to_string())
        .collect();

    let needed_git: Vec<String> = git_tables
        .iter()
        .filter(|t| query_mentions_table(&query_upper, t))
        .map(|s| s.to_string())
        .collect();

    (needed_claude, needed_git)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_tables_handles_jhistory_without_history_false_positive() {
        let (claude, _) = detect_tables("SELECT session_id, text FROM jhistory LIMIT 5");

        assert!(claude.contains(&"jhistory".to_string()));
        assert!(!claude.contains(&"history".to_string()));
    }

    #[test]
    fn detect_tables_handles_codex_history_without_history_false_positive() {
        let (claude, _) = detect_tables("SELECT session_id, text FROM codex_history LIMIT 5");

        assert!(claude.contains(&"codex_history".to_string()));
        assert!(!claude.contains(&"history".to_string()));
    }

    #[test]
    fn normalize_ts_seconds_converts_millis() {
        assert_eq!(normalize_ts_seconds(1_754_402_102), 1_754_402_102);
        assert_eq!(normalize_ts_seconds(1_754_402_102_000), 1_754_402_102);
    }
}
