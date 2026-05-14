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
    git_repo_path: PathBuf,
}

impl UnifiedEngine {
    /// Create a new unified engine
    pub fn new(claude_data_dir: PathBuf, git_repo_path: PathBuf) -> Result<Self> {
        let conn = Connection::open_in_memory()?;

        // Register custom DATE function that handles both epoch ms and ISO dates
        conn.create_scalar_function("DATE", 1, rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
            let value: String = ctx.get(0)?;
            Ok(normalize_date(&value))
        })?;

        Ok(Self {
            conn,
            claude_data_dir,
            git_repo_path,
        })
    }

    /// Load Claude Code tables needed for the query
    pub fn load_claude_tables(&mut self, tables: &[&str]) -> Result<()> {
        for table in tables {
            match *table {
                "history" => self.load_history()?,
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
                    let timestamp = entry.get("timestamp").map(|v| v.to_string()).unwrap_or_default();
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
                role TEXT,
                content TEXT,
                tool_name TEXT,
                session_id TEXT,
                project TEXT,
                timestamp TEXT,
                cwd TEXT,
                git_branch TEXT,
                user_type TEXT,
                uuid TEXT,
                parent_uuid TEXT,
                source_file TEXT
            )",
            [],
        )?;

        // Current Claude Code layout: ~/.claude/projects/<slug>/<session>.jsonl
        // Also nested layouts: <slug>/<session-uuid>/subagents/<agent>.jsonl
        // (spawned subagent transcripts) and <slug>/prune-backup/*.jsonl
        // (archived pre-compaction transcripts).
        let projects_dir = self.claude_data_dir.join("projects");
        if projects_dir.exists() {
            for entry in walkdir::WalkDir::new(&projects_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if !path.extension().is_some_and(|ext| ext == "jsonl") {
                    continue;
                }
                let project_slug = path
                    .strip_prefix(&projects_dir)
                    .ok()
                    .and_then(|r| r.components().next())
                    .and_then(|c| c.as_os_str().to_str())
                    .map(|s| s.to_string());
                self.load_transcript_file(path, project_slug.as_deref())?;
            }
        }

        // Legacy layout: ~/.claude/transcripts/<session>.jsonl
        let transcripts_dir = self.claude_data_dir.join("transcripts");
        if transcripts_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&transcripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "jsonl") {
                        self.load_transcript_file(&path, None)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn load_transcript_file(
        &mut self,
        path: &std::path::Path,
        project: Option<&str>,
    ) -> Result<()> {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        // Legacy filenames sometimes carried a `ses_` prefix.
        let session_id = stem.strip_prefix("ses_").unwrap_or(stem).to_string();
        let source_file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };

        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO transcripts (
                    type, role, content, tool_name, session_id, project,
                    timestamp, cwd, git_branch, user_type, uuid, parent_uuid, source_file
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for line in content.lines() {
                if line.is_empty() {
                    continue;
                }
                let Ok(entry) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let row_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let timestamp = entry.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                let cwd = entry.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
                let git_branch = entry.get("gitBranch").and_then(|v| v.as_str()).unwrap_or("");
                let user_type = entry.get("userType").and_then(|v| v.as_str()).unwrap_or("");
                let uuid = entry.get("uuid").and_then(|v| v.as_str()).unwrap_or("");
                let parent_uuid = entry
                    .get("parentUuid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let (role, text, tool_name) = extract_message_fields(&entry);

                stmt.execute(params![
                    row_type,
                    role,
                    text,
                    tool_name,
                    session_id,
                    project.unwrap_or(""),
                    timestamp,
                    cwd,
                    git_branch,
                    user_type,
                    uuid,
                    parent_uuid,
                    source_file,
                ])?;
            }
        }
        tx.commit()?;
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
                        params![id, short_id, author_name, author_email, authored_at, summary, message, is_merge],
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
                    let target = branch.get().target().map(|t| t.to_string()).unwrap_or_default();
                    let is_head = if branch.is_head() { 1 } else { 0 };
                    let is_remote = if branch_type == git2::BranchType::Remote { 1 } else { 0 };

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

/// Extract `(role, content_text, tool_name)` from a transcript row's
/// `message` object. The Claude Code JSONL format stores user prompts as
/// either a string or a list of content blocks, and assistant responses
/// as a list of `text` / `thinking` / `tool_use` blocks. We collapse all
/// text-bearing blocks into one string and surface the first tool name.
fn extract_message_fields(entry: &Value) -> (String, String, String) {
    let Some(msg) = entry.get("message") else {
        return (String::new(), String::new(), String::new());
    };

    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let content = msg.get("content");
    let (text, tool_name) = match content {
        Some(Value::String(s)) => (s.clone(), String::new()),
        Some(Value::Array(blocks)) => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut first_tool: Option<String> = None;
            for block in blocks {
                let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match btype {
                    "text" | "thinking" => {
                        if let Some(s) = block.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(s.to_string());
                        } else if let Some(s) = block.get("thinking").and_then(|v| v.as_str()) {
                            text_parts.push(s.to_string());
                        }
                    }
                    "tool_use" => {
                        if first_tool.is_none() {
                            if let Some(n) = block.get("name").and_then(|v| v.as_str()) {
                                first_tool = Some(n.to_string());
                            }
                        }
                    }
                    "tool_result" => {
                        if let Some(s) = block.get("content").and_then(|v| v.as_str()) {
                            text_parts.push(s.to_string());
                        }
                    }
                    _ => {}
                }
            }
            (text_parts.join("\n"), first_tool.unwrap_or_default())
        }
        _ => (String::new(), String::new()),
    };

    (role, text, tool_name)
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

/// Detect which tables are needed from a SQL query
pub fn detect_tables(query: &str) -> (Vec<String>, Vec<String>) {
    let query_upper = query.to_uppercase();

    let claude_tables = ["history", "transcripts", "todos", "stats"];
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
        .filter(|t| query_upper.contains(&t.to_uppercase()))
        .map(|s| s.to_string())
        .collect();

    let needed_git: Vec<String> = git_tables
        .iter()
        .filter(|t| query_upper.contains(&t.to_uppercase()))
        .map(|s| s.to_string())
        .collect();

    (needed_claude, needed_git)
}
