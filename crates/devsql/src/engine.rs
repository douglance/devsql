//! Unified query engine that combines ccql and vcsql data

use crate::{Error, Result};
use chrono::DateTime;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

/// Schema version. Bump when transcripts columns / indexes change so the
/// cache gets dropped and rebuilt on the next run.
const SCHEMA_VERSION: i64 = 1;

/// Cache filename inside `claude_data_dir`.
const CACHE_FILE: &str = ".devsql-cache.db";

/// Unified query engine that loads data from both Claude Code and Git
pub struct UnifiedEngine {
    conn: Connection,
    claude_data_dir: PathBuf,
    git_repo_path: PathBuf,
    /// True when backed by a persistent cache file. Drives incremental sync.
    cached: bool,
}

impl UnifiedEngine {
    /// Create a new unified engine.
    ///
    /// When `use_cache` is true (the default), opens a persistent SQLite at
    /// `<claude_data_dir>/.devsql-cache.db`. Subsequent runs only re-parse
    /// JSONL transcript files whose mtime/size changed, dropping query time
    /// from ~20s to ~1s on a real `~/.claude` directory. Falls back to an
    /// in-memory database when the cache can't be opened (e.g. read-only
    /// data dir).
    pub fn new(claude_data_dir: PathBuf, git_repo_path: PathBuf) -> Result<Self> {
        Self::with_options(claude_data_dir, git_repo_path, true, false)
    }

    pub fn with_options(
        claude_data_dir: PathBuf,
        git_repo_path: PathBuf,
        use_cache: bool,
        rebuild_cache: bool,
    ) -> Result<Self> {
        let (conn, cached) = if use_cache {
            let cache_path = claude_data_dir.join(CACHE_FILE);
            if rebuild_cache {
                let _ = std::fs::remove_file(&cache_path);
            }
            match Connection::open(&cache_path) {
                Ok(c) => (c, true),
                Err(_) => (Connection::open_in_memory()?, false),
            }
        } else {
            (Connection::open_in_memory()?, false)
        };

        // Performance pragmas for the persistent cache. A larger page cache
        // cuts disk reads during query and rebuild, and `synchronous = NORMAL`
        // skips fsyncs that aren't needed for our crash semantics — the cache
        // is rebuildable from JSONL anyway, so a partial-tail loss is fine.
        // (We deliberately do NOT enable WAL: with 500k-row single-commit
        // rebuilds, WAL checkpointing made cold runs ~50% slower in testing.)
        if cached {
            conn.execute_batch(
                "PRAGMA synchronous = NORMAL;
                 PRAGMA temp_store = MEMORY;
                 PRAGMA cache_size = -65536;",
            )?;
        }

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

        let engine = Self {
            conn,
            claude_data_dir,
            git_repo_path,
            cached,
        };

        if engine.cached {
            engine.ensure_schema_version()?;
        }

        Ok(engine)
    }

    /// If the cached schema version doesn't match, drop our tables so the
    /// next loader call rebuilds them from scratch.
    fn ensure_schema_version(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _devsql_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )?;

        let current: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM _devsql_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();

        let matches = current.as_deref() == Some(&SCHEMA_VERSION.to_string());
        if !matches {
            // Old version (or no version yet). Drop our owned tables so the
            // loaders rebuild them. Don't touch tables we don't own.
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS transcripts;
                 DROP TABLE IF EXISTS _transcript_files;
                 DROP TABLE IF EXISTS history;
                 DROP TABLE IF EXISTS todos;
                 DROP TABLE IF EXISTS commits;
                 DROP TABLE IF EXISTS diffs;
                 DROP TABLE IF EXISTS diff_files;
                 DROP TABLE IF EXISTS branches;",
            )?;
            self.conn.execute(
                "INSERT OR REPLACE INTO _devsql_meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(())
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

        // history.jsonl is small (~thousands of rows) and append-only. With a
        // persistent cache we'd accumulate duplicates, so truncate and reload.
        self.conn.execute("DELETE FROM history", [])?;

        // Load from ccql's history.jsonl
        let history_path = self.claude_data_dir.join("history.jsonl");
        if history_path.exists() {
            let content = std::fs::read_to_string(&history_path)?;
            let tx = self.conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO history (display, timestamp, project) VALUES (?1, ?2, ?3)",
                )?;
                for line in content.lines() {
                    if let Ok(entry) = serde_json::from_str::<Value>(line) {
                        let display = entry.get("display").and_then(|v| v.as_str()).unwrap_or("");
                        let timestamp = entry
                            .get("timestamp")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        let project = entry.get("project").and_then(|v| v.as_str()).unwrap_or("");

                        stmt.execute(params![display, timestamp, project])?;
                    }
                }
            }
            tx.commit()?;
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
                source_file TEXT,
                source_path TEXT
            )",
            [],
        )?;

        // Tracking table for incremental sync. Keyed by the absolute source
        // path so we can detect mtime/size changes between runs and reload
        // only what actually changed.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS _transcript_files (
                path TEXT PRIMARY KEY,
                mtime_ns INTEGER NOT NULL,
                size INTEGER NOT NULL
            )",
            [],
        )?;

        // Indexes — created once, reused across runs. These speed up the
        // common filter patterns (timestamp ranges, project filtering,
        // single-session drill-down).
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_transcripts_project
                ON transcripts (project);
             CREATE INDEX IF NOT EXISTS idx_transcripts_timestamp
                ON transcripts (timestamp);
             CREATE INDEX IF NOT EXISTS idx_transcripts_session
                ON transcripts (session_id);
             CREATE INDEX IF NOT EXISTS idx_transcripts_source_path
                ON transcripts (source_path);
             CREATE INDEX IF NOT EXISTS idx_transcripts_type
                ON transcripts (type);",
        )?;

        // Build the on-disk set of files and their (mtime, size).
        let mut disk: Vec<(PathBuf, Option<String>, i64, i64)> = Vec::new();

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
                if let Some((mtime, size)) = file_stat(path) {
                    disk.push((path.to_path_buf(), project_slug, mtime, size));
                }
            }
        }

        let transcripts_dir = self.claude_data_dir.join("transcripts");
        if transcripts_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&transcripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "jsonl") {
                        if let Some((mtime, size)) = file_stat(&path) {
                            disk.push((path, None, mtime, size));
                        }
                    }
                }
            }
        }

        // Pull the cache snapshot.
        let mut cache: HashMap<String, (i64, i64)> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT path, mtime_ns, size FROM _transcript_files")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
            })?;
            for row in rows.flatten() {
                cache.insert(row.0, (row.1, row.2));
            }
        }

        // Compute the diff: what needs re-parsing, what's stale.
        let mut to_reload: Vec<(PathBuf, Option<String>, i64, i64)> = Vec::new();
        let mut seen_paths: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(disk.len());

        for (path, slug, mtime, size) in disk.into_iter() {
            let key = path.to_string_lossy().into_owned();
            seen_paths.insert(key.clone());
            match cache.get(&key) {
                Some(&(cached_mtime, cached_size))
                    if cached_mtime == mtime && cached_size == size => {}
                _ => to_reload.push((path, slug, mtime, size)),
            }
        }

        let stale: Vec<String> = cache
            .keys()
            .filter(|p| !seen_paths.contains(*p))
            .cloned()
            .collect();

        if to_reload.is_empty() && stale.is_empty() {
            return Ok(());
        }

        // Apply diff inside a single transaction.
        let tx = self.conn.unchecked_transaction()?;
        {
            // Remove rows for files that vanished from disk.
            if !stale.is_empty() {
                let mut del_rows = tx.prepare(
                    "DELETE FROM transcripts WHERE source_path = ?1",
                )?;
                let mut del_meta = tx.prepare(
                    "DELETE FROM _transcript_files WHERE path = ?1",
                )?;
                for p in &stale {
                    del_rows.execute(params![p])?;
                    del_meta.execute(params![p])?;
                }
            }

            // Re-load files whose mtime or size changed.
            if !to_reload.is_empty() {
                let mut del_rows = tx.prepare(
                    "DELETE FROM transcripts WHERE source_path = ?1",
                )?;
                let mut upsert_meta = tx.prepare(
                    "INSERT OR REPLACE INTO _transcript_files (path, mtime_ns, size)
                     VALUES (?1, ?2, ?3)",
                )?;
                let mut ins = tx.prepare(
                    "INSERT INTO transcripts (
                        type, role, content, tool_name, session_id, project,
                        timestamp, cwd, git_branch, user_type, uuid, parent_uuid,
                        source_file, source_path
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                )?;

                for (path, project_slug, mtime, size) in to_reload {
                    let path_str = path.to_string_lossy().into_owned();
                    del_rows.execute(params![path_str])?;

                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown");
                    let session_id = stem.strip_prefix("ses_").unwrap_or(stem).to_string();
                    let source_file = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let content = match std::fs::read_to_string(&path) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    for line in content.lines() {
                        if line.is_empty() {
                            continue;
                        }
                        let Ok(entry) = serde_json::from_str::<Value>(line) else {
                            continue;
                        };
                        let row_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let timestamp =
                            entry.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                        let cwd = entry.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
                        let git_branch =
                            entry.get("gitBranch").and_then(|v| v.as_str()).unwrap_or("");
                        let user_type =
                            entry.get("userType").and_then(|v| v.as_str()).unwrap_or("");
                        let uuid = entry.get("uuid").and_then(|v| v.as_str()).unwrap_or("");
                        let parent_uuid = entry
                            .get("parentUuid")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        let (role, text, tool_name) = extract_message_fields(&entry);

                        ins.execute(params![
                            row_type,
                            role,
                            text,
                            tool_name,
                            session_id,
                            project_slug.as_deref().unwrap_or(""),
                            timestamp,
                            cwd,
                            git_branch,
                            user_type,
                            uuid,
                            parent_uuid,
                            source_file,
                            path_str,
                        ])?;
                    }

                    upsert_meta.execute(params![path_str, mtime, size])?;
                }
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

        // Truncate before reload: branch HEAD moves and branches get deleted.
        self.conn.execute("DELETE FROM branches", [])?;

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

/// Returns `(mtime_ns, size)` for a path, or `None` if it can't be stat'd.
/// mtime is normalized to nanoseconds since UNIX_EPOCH for stable storage.
fn file_stat(path: &std::path::Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len() as i64;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(UNIX_EPOCH).ok()?;
    let mtime_ns = (dur.as_secs() as i64) * 1_000_000_000 + (dur.subsec_nanos() as i64);
    Some((mtime_ns, size))
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
