//! Unified query engine that combines ccql and vcsql data

use crate::{Error, Result};
use ccql::datasources::transcript::{
    discover_transcript_files, flattened_usage_fields, SessionAggregate,
};
use chrono::DateTime;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

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
                "sessions" => self.load_sessions()?,
                "todos" => self.load_todos()?,
                "tool_calls" => self.load_tool_calls()?,
                "codex_tool_calls" => self.load_codex_tool_calls()?,
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

    /// Load code analysis tables needed for the query
    pub fn load_code_tables(&mut self, tables: &[&str]) -> Result<()> {
        crate::providers::load_all_code_tables(&self.conn, &self.git_repo_path, tables)
    }

    /// Load the normalized Atuin, zsh, and bash history table.
    pub fn load_shell_history(&mut self) -> Result<()> {
        crate::providers::shell_history::load(&mut self.conn)
    }

    /// Load normalized shell and agent-issued command events with source-native provenance.
    pub fn load_command_events(&mut self) -> Result<()> {
        self.load_shell_history()?;
        if !table_exists(&self.conn, "tool_calls")? {
            self.load_tool_calls()?;
        }
        if !table_exists(&self.conn, "codex_tool_calls")? {
            self.load_codex_tool_calls()?;
        }

        self.conn.execute_batch(
            "DROP TABLE IF EXISTS command_events;
             CREATE TABLE command_events (
                source TEXT,
                channel TEXT,
                actor TEXT,
                provenance_quality TEXT,
                provenance_reason TEXT,
                source_id TEXT,
                source_order INTEGER,
                session_id TEXT,
                parent_session_id TEXT,
                agent_id TEXT,
                agent_role TEXT,
                originator TEXT,
                tool_name TEXT,
                timestamp TEXT,
                duration_ms INTEGER,
                exit_code INTEGER,
                command TEXT,
                cwd TEXT,
                hostname TEXT,
                source_path TEXT
             );
             INSERT INTO command_events
             SELECT source, 'shell', 'unknown', 'unattributed',
                    'unattributed_shell_history', source_id, source_order,
                    session_id, NULL, NULL, NULL, NULL, NULL, timestamp,
                    duration_ms, exit_code, command, cwd, hostname, history_path
             FROM shell_history;
             INSERT INTO command_events
             SELECT 'claude', 'agent_tool', 'agent', 'exact', NULL,
                    source_id, rowid, session_id, parent_session_id, agent_id,
                    agent_role, originator, tool_name, timestamp, NULL, NULL,
                    command, cwd, NULL, source_path
             FROM tool_calls
             WHERE tool_name = 'Bash' AND command IS NOT NULL;
             INSERT INTO command_events
             SELECT 'codex', 'agent_tool', 'agent', 'exact', NULL,
                    source_id, rowid, session_id, parent_session_id, agent_id,
                    agent_role, originator, tool_name, timestamp, NULL, NULL,
                    cmd, cwd, NULL, source_path
             FROM codex_tool_calls
             WHERE tool_name IN ('exec_command', 'shell') AND cmd IS NOT NULL;",
        )?;

        Ok(())
    }

    /// Access the underlying SQLite connection. Used by `gather` to
    /// materialize section results as `gather_<section>` tables in the
    /// connection used for rendering.
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
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

    /// Build a ccql Config for transcript discovery (None when the Claude
    /// data directory does not exist).
    fn ccql_config(&self) -> Option<ccql::Config> {
        ccql::Config::new_with_codex_data_dir(
            self.claude_data_dir.clone(),
            self.codex_data_dir.clone(),
        )
        .ok()
    }

    fn load_transcripts(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS transcripts (
                rowid INTEGER PRIMARY KEY,
                type TEXT,
                content TEXT,
                tool_name TEXT,
                session_id TEXT,
                _source_file TEXT,
                _session_id TEXT,
                _project TEXT,
                _agent_id TEXT,
                timestamp TEXT,
                model TEXT,
                usage_input_tokens INTEGER,
                usage_output_tokens INTEGER,
                usage_cache_read_input_tokens INTEGER,
                usage_cache_creation_input_tokens INTEGER,
                usage_ephemeral_5m_input_tokens INTEGER,
                usage_ephemeral_1h_input_tokens INTEGER,
                usage_service_tier TEXT
            )",
            [],
        )?;

        let Some(config) = self.ccql_config() else {
            return Ok(());
        };

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO transcripts (type, content, tool_name, session_id,
                    _source_file, _session_id, _project, _agent_id, timestamp,
                    model, usage_input_tokens, usage_output_tokens,
                    usage_cache_read_input_tokens, usage_cache_creation_input_tokens,
                    usage_ephemeral_5m_input_tokens, usage_ephemeral_1h_input_tokens,
                    usage_service_tier)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;

            for file in discover_transcript_files(&config) {
                let content = match std::fs::read_to_string(&file.path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for line in content.lines() {
                    let Ok(entry) = serde_json::from_str::<Value>(line) else {
                        continue;
                    };

                    let msg_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let msg_content = entry
                        .get("content")
                        .and_then(|v| v.as_str())
                        .or_else(|| entry.get("message").and_then(|v| v.as_str()))
                        .unwrap_or("");
                    let tool_name = entry
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let timestamp = entry.get("timestamp").and_then(|v| v.as_str());

                    let usage: HashMap<&str, &Value> =
                        flattened_usage_fields(&entry).into_iter().collect();
                    let usage_int = |key: &str| usage.get(key).and_then(|v| v.as_i64());

                    stmt.execute(params![
                        msg_type,
                        msg_content,
                        tool_name,
                        file.session_id,
                        file.source_file,
                        file.session_id,
                        file.project,
                        file.agent_id,
                        timestamp,
                        usage.get("model").and_then(|v| v.as_str()),
                        usage_int("usage_input_tokens"),
                        usage_int("usage_output_tokens"),
                        usage_int("usage_cache_read_input_tokens"),
                        usage_int("usage_cache_creation_input_tokens"),
                        usage_int("usage_ephemeral_5m_input_tokens"),
                        usage_int("usage_ephemeral_1h_input_tokens"),
                        usage.get("usage_service_tier").and_then(|v| v.as_str()),
                    ])?;
                }
            }
        }
        tx.commit()?;

        Ok(())
    }

    fn load_tool_calls(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS tool_calls (
                rowid INTEGER PRIMARY KEY,
                tool_name TEXT,
                input_json TEXT,
                target TEXT,
                source_id TEXT,
                command TEXT,
                session_id TEXT,
                parent_session_id TEXT,
                agent_id TEXT,
                agent_role TEXT,
                originator TEXT,
                cwd TEXT,
                source_path TEXT,
                _project TEXT,
                timestamp TEXT
            )",
            [],
        )?;

        let Some(config) = self.ccql_config() else {
            return Ok(());
        };

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO tool_calls (
                    tool_name, input_json, target, source_id, command, session_id,
                    parent_session_id, agent_id, agent_role, originator, cwd,
                    source_path, _project, timestamp
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 )",
            )?;

            for file in discover_transcript_files(&config) {
                let source_path = file.path.to_string_lossy().into_owned();
                visit_jsonl_candidates(&file.path, &[b"\"tool_use\""], |entry| {
                    let record_session_id = string_field(&entry, "sessionId")
                        .or_else(|| string_field(&entry, "session_id"))
                        .unwrap_or_else(|| file.session_id.clone());
                    let parent_session_id = file.agent_id.as_ref().map(|_| file.session_id.clone());
                    let agent_role = string_field(&entry, "agentName");
                    let originator = string_field(&entry, "originator")
                        .or_else(|| nested_string_field(&entry, "origin", "kind"))
                        .or_else(|| string_field(&entry, "entrypoint"));
                    let cwd = string_field(&entry, "cwd");
                    for call in ccql::datasources::tool_calls::extract_tool_calls(&entry) {
                        stmt.execute(params![
                            call.tool_name,
                            call.input_json,
                            call.target,
                            call.source_id,
                            call.command,
                            record_session_id,
                            parent_session_id,
                            file.agent_id,
                            agent_role,
                            originator,
                            cwd,
                            source_path,
                            file.project,
                            call.timestamp,
                        ])?;
                    }
                    Ok(())
                })?;
            }
        }
        tx.commit()?;

        Ok(())
    }

    fn load_codex_tool_calls(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS codex_tool_calls (
                rowid INTEGER PRIMARY KEY,
                tool_name TEXT,
                arguments_json TEXT,
                cmd TEXT,
                source_id TEXT,
                session_id TEXT,
                parent_session_id TEXT,
                agent_id TEXT,
                agent_role TEXT,
                originator TEXT,
                cwd TEXT,
                source_path TEXT,
                timestamp TEXT
            )",
            [],
        )?;

        let sessions_dir = self.codex_data_dir.join("sessions");

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO codex_tool_calls (
                    tool_name, arguments_json, cmd, source_id, session_id,
                    parent_session_id, agent_id, agent_role, originator, cwd,
                    source_path, timestamp
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
                 )",
            )?;

            for path in
                ccql::datasources::codex_tool_calls::discover_codex_session_files(&sessions_dir)
            {
                let source_path = path.to_string_lossy().into_owned();
                let mut meta = ccql::datasources::codex_tool_calls::CodexSessionMeta::default();
                visit_jsonl_candidates(
                    &path,
                    &[b"\"session_meta\"", b"\"function_call\""],
                    |entry| {
                        if let Some(new_meta) =
                            ccql::datasources::codex_tool_calls::extract_session_meta(&entry)
                        {
                            merge_codex_meta(&mut meta, new_meta);
                            return Ok(());
                        }

                        if let Some(call) =
                            ccql::datasources::codex_tool_calls::extract_codex_tool_call(&entry)
                        {
                            let cwd = call.cwd.clone().or_else(|| meta.cwd.clone());
                            stmt.execute(params![
                                call.tool_name,
                                call.arguments_json,
                                call.cmd,
                                call.source_id,
                                meta.session_id,
                                meta.parent_session_id,
                                meta.agent_id,
                                meta.agent_role,
                                meta.originator,
                                cwd,
                                source_path,
                                call.timestamp,
                            ])?;
                        }
                        Ok(())
                    },
                )?;
            }
        }
        tx.commit()?;

        Ok(())
    }

    fn load_sessions(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT,
                project TEXT,
                cwd TEXT,
                git_branch TEXT,
                version TEXT,
                title TEXT,
                first_timestamp TEXT,
                last_timestamp TEXT,
                user_message_count INTEGER,
                assistant_message_count INTEGER,
                subagent_count INTEGER,
                total_input_tokens INTEGER,
                total_output_tokens INTEGER,
                total_cache_read_input_tokens INTEGER,
                total_cache_creation_input_tokens INTEGER,
                pr_url TEXT,
                pr_number INTEGER
            )",
            [],
        )?;

        let Some(config) = self.ccql_config() else {
            return Ok(());
        };

        let files = discover_transcript_files(&config);

        // Subagent file counts keyed by (project, parent session id)
        let mut subagent_counts: HashMap<(Option<String>, String), i64> = HashMap::new();
        for file in &files {
            if file.agent_id.is_some() {
                *subagent_counts
                    .entry((file.project.clone(), file.session_id.clone()))
                    .or_insert(0) += 1;
            }
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO sessions VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;

            for file in files.iter().filter(|f| f.agent_id.is_none()) {
                let content = match std::fs::read_to_string(&file.path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let mut agg = SessionAggregate::default();
                for line in content.lines() {
                    if let Ok(json) = serde_json::from_str::<Value>(line) {
                        agg.observe(&json);
                    }
                }

                let subagent_count = subagent_counts
                    .get(&(file.project.clone(), file.session_id.clone()))
                    .copied()
                    .unwrap_or(0);

                stmt.execute(params![
                    file.session_id,
                    file.project,
                    agg.cwd,
                    agg.git_branch,
                    agg.version,
                    agg.title,
                    agg.first_timestamp,
                    agg.last_timestamp,
                    agg.user_message_count,
                    agg.assistant_message_count,
                    subagent_count,
                    agg.total_input_tokens,
                    agg.total_output_tokens,
                    agg.total_cache_read_input_tokens,
                    agg.total_cache_creation_input_tokens,
                    agg.pr_url,
                    agg.pr_number,
                ])?;
            }
        }
        tx.commit()?;

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

        let todos_dir = self.claude_data_dir.join("todos");
        if !todos_dir.is_dir() {
            return Ok(());
        }

        let entries = match std::fs::read_dir(&todos_dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Try parsing as a JSON array of todo items
            if let Ok(items) = serde_json::from_str::<Vec<Value>>(&content) {
                for item in &items {
                    let todo_content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");

                    self.conn.execute(
                        "INSERT INTO todos (content, status) VALUES (?1, ?2)",
                        params![todo_content, status],
                    )?;
                }
            }
        }

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

        if let Ok(repo) = git2::Repository::open(&self.git_repo_path) {
            let mut revwalk = repo.revwalk().map_err(|e| Error::Vcsql(e.to_string()))?;
            revwalk.push_head().ok();

            for oid in revwalk.filter_map(|r| r.ok()) {
                let Ok(commit) = repo.find_commit(oid) else {
                    continue;
                };

                let commit_tree = match commit.tree() {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                let parent_tree = if commit.parent_count() > 0 {
                    commit.parent(0).ok().and_then(|p| p.tree().ok())
                } else {
                    None
                };

                let diff =
                    match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };

                let stats = match diff.stats() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let commit_id = commit.id().to_string();
                self.conn.execute(
                    "INSERT OR IGNORE INTO diffs VALUES (?1, ?2, ?3, ?4)",
                    params![
                        commit_id,
                        stats.files_changed() as i64,
                        stats.insertions() as i64,
                        stats.deletions() as i64,
                    ],
                )?;
            }
        }

        Ok(())
    }

    fn load_diff_files(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS diff_files (
                commit_id TEXT,
                path TEXT,
                status TEXT,
                insertions INTEGER,
                deletions INTEGER
            )",
            [],
        )?;

        if let Ok(repo) = git2::Repository::open(&self.git_repo_path) {
            let mut revwalk = repo.revwalk().map_err(|e| Error::Vcsql(e.to_string()))?;
            revwalk.push_head().ok();

            for oid in revwalk.filter_map(|r| r.ok()) {
                let Ok(commit) = repo.find_commit(oid) else {
                    continue;
                };

                let commit_tree = match commit.tree() {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                let parent_tree = if commit.parent_count() > 0 {
                    commit.parent(0).ok().and_then(|p| p.tree().ok())
                } else {
                    None
                };

                let diff =
                    match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };

                let commit_id = commit.id().to_string();

                for delta_idx in 0..diff.deltas().len() {
                    let delta = diff.deltas().nth(delta_idx).unwrap();

                    let path = delta
                        .new_file()
                        .path()
                        .or_else(|| delta.old_file().path())
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let status = match delta.status() {
                        git2::Delta::Added => "A",
                        git2::Delta::Deleted => "D",
                        git2::Delta::Modified => "M",
                        git2::Delta::Renamed => "R",
                        git2::Delta::Copied => "C",
                        _ => "?",
                    };

                    // Get per-file line stats from Patch
                    let (insertions, deletions) =
                        if let Ok(Some(ref p)) = git2::Patch::from_diff(&diff, delta_idx) {
                            let (_, adds, dels) = p.line_stats().unwrap_or((0, 0, 0));
                            (adds as i64, dels as i64)
                        } else {
                            (0i64, 0i64)
                        };

                    self.conn.execute(
                        "INSERT INTO diff_files VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![commit_id, path, status, insertions, deletions],
                    )?;
                }
            }
        }

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

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table_name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn visit_jsonl_candidates<F>(path: &Path, markers: &[&[u8]], mut visit: F) -> Result<()>
where
    F: FnMut(Value) -> Result<()>,
{
    let Ok(file) = File::open(path) else {
        return Ok(());
    };
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();

    loop {
        line.clear();
        let bytes_read = match reader.read_until(b'\n', &mut line) {
            Ok(bytes_read) => bytes_read,
            Err(_) => return Ok(()),
        };
        if bytes_read == 0 {
            return Ok(());
        }
        if !markers
            .iter()
            .any(|marker| memchr::memmem::find(&line, marker).is_some())
        {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        visit(entry)?;
    }
}

fn string_field(json: &Value, field: &str) -> Option<String> {
    json.get(field).and_then(Value::as_str).map(String::from)
}

fn nested_string_field(json: &Value, parent: &str, field: &str) -> Option<String> {
    json.get(parent)
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(String::from)
}

fn merge_codex_meta(
    current: &mut ccql::datasources::codex_tool_calls::CodexSessionMeta,
    new: ccql::datasources::codex_tool_calls::CodexSessionMeta,
) {
    if new.session_id.is_some() {
        current.session_id = new.session_id;
    }
    if new.cwd.is_some() {
        current.cwd = new.cwd;
    }
    if new.parent_session_id.is_some() {
        current.parent_session_id = new.parent_session_id;
    }
    if new.agent_id.is_some() {
        current.agent_id = new.agent_id;
    }
    if new.agent_role.is_some() {
        current.agent_role = new.agent_role;
    }
    if new.originator.is_some() {
        current.originator = new.originator;
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

/// Detect which tables are needed from a SQL query.
///
/// Returns a 4-tuple: (claude_tables, git_tables, code_tables, shell_tables).
pub fn detect_tables(query: &str) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let query_upper = query.to_uppercase();

    let claude_tables = [
        "history",
        "jhistory",
        "codex_history",
        "transcripts",
        "sessions",
        "todos",
        "stats",
        "tool_calls",
        "codex_tool_calls",
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
    let code_tables = [
        "source_files",
        "source_lines",
        "symbols",
        "imports",
        "ast_nodes",
    ];
    let shell_tables = ["shell_history", "command_events"];

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

    let needed_code: Vec<String> = code_tables
        .iter()
        .filter(|t| query_mentions_table(&query_upper, t))
        .map(|s| s.to_string())
        .collect();

    let needed_shell: Vec<String> = shell_tables
        .iter()
        .filter(|t| query_mentions_table(&query_upper, t))
        .map(|s| s.to_string())
        .collect();

    (needed_claude, needed_git, needed_code, needed_shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_tables_handles_jhistory_without_history_false_positive() {
        let (claude, _, _, _) = detect_tables("SELECT session_id, text FROM jhistory LIMIT 5");

        assert!(claude.contains(&"jhistory".to_string()));
        assert!(!claude.contains(&"history".to_string()));
    }

    #[test]
    fn detect_tables_handles_codex_history_without_history_false_positive() {
        let (claude, _, _, _) = detect_tables("SELECT session_id, text FROM codex_history LIMIT 5");

        assert!(claude.contains(&"codex_history".to_string()));
        assert!(!claude.contains(&"history".to_string()));
    }

    #[test]
    fn detect_tables_finds_code_tables() {
        let (_, _, code, _) = detect_tables(
            "SELECT * FROM source_files JOIN symbols ON source_files.path = symbols.file_path",
        );

        assert!(code.contains(&"source_files".to_string()));
        assert!(code.contains(&"symbols".to_string()));
        assert!(!code.contains(&"source_lines".to_string()));
    }

    #[test]
    fn detect_tables_finds_shell_history_without_history_false_positive() {
        let (claude, _, _, shell) = detect_tables("SELECT command FROM shell_history LIMIT 5");

        assert_eq!(shell, vec!["shell_history".to_string()]);
        assert!(!claude.contains(&"history".to_string()));
    }

    #[test]
    fn detect_tables_finds_command_events() {
        let (_, _, _, shell) = detect_tables("SELECT actor, command FROM command_events");

        assert_eq!(shell, vec!["command_events".to_string()]);
    }

    #[test]
    fn normalize_ts_seconds_converts_millis() {
        assert_eq!(normalize_ts_seconds(1_754_402_102), 1_754_402_102);
        assert_eq!(normalize_ts_seconds(1_754_402_102_000), 1_754_402_102);
    }

    fn write(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn loads_modern_projects_layout_with_usage_and_sessions() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        let slug = "-Users-doug-Developer-app";

        let records = [
            r#"{"type":"user","content":"hi","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/doug/Developer/app","gitBranch":"main","version":"2.1.100"}"#,
            r#"{"type":"assistant","timestamp":"2026-06-01T10:00:05.000Z","message":{"model":"claude-opus-4-7","usage":{"input_tokens":6,"output_tokens":127,"cache_read_input_tokens":100,"cache_creation_input_tokens":200}}}"#,
            r#"{"type":"ai-title","aiTitle":"Fix the widget","sessionId":"sess-rich"}"#,
            r#"{"type":"pr-link","sessionId":"sess-rich","prNumber":42,"prUrl":"https://github.com/org/repo/pull/42"}"#,
        ]
        .join("\n");
        write(
            &root.join("projects").join(slug).join("sess-rich.jsonl"),
            &records,
        );
        write(
            &root
                .join("projects")
                .join(slug)
                .join("sess-rich")
                .join("subagents")
                .join("agent-one.jsonl"),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":999,"output_tokens":999}}}"#,
        );
        write(
            &root.join("transcripts").join("ses_old.jsonl"),
            r#"{"type":"user","content":"legacy"}"#,
        );

        let mut engine =
            UnifiedEngine::new(root.to_path_buf(), root.to_path_buf()).expect("engine");
        engine
            .load_claude_tables(&["transcripts", "sessions"])
            .expect("load");

        // All files across both layouts are ingested (4 + 1 + 1 records)
        let count = engine
            .query("SELECT COUNT(*) AS n FROM transcripts")
            .expect("count");
        assert_eq!(count[0]["n"], serde_json::json!(6));

        // _project / _agent_id metadata columns
        let sub = engine
            .query("SELECT _project, _agent_id, _session_id FROM transcripts WHERE _agent_id IS NOT NULL")
            .expect("subagent row");
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0]["_project"], serde_json::json!(slug));
        assert_eq!(sub[0]["_agent_id"], serde_json::json!("agent-one"));
        assert_eq!(sub[0]["_session_id"], serde_json::json!("sess-rich"));

        // Flattened usage columns
        let usage = engine
            .query(
                "SELECT model, usage_input_tokens, usage_output_tokens, \
                 usage_cache_read_input_tokens, usage_cache_creation_input_tokens \
                 FROM transcripts WHERE _agent_id IS NULL AND type = 'assistant'",
            )
            .expect("usage row");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0]["model"], serde_json::json!("claude-opus-4-7"));
        assert_eq!(usage[0]["usage_input_tokens"], serde_json::json!(6));
        assert_eq!(usage[0]["usage_output_tokens"], serde_json::json!(127));
        assert_eq!(
            usage[0]["usage_cache_read_input_tokens"],
            serde_json::json!(100)
        );
        assert_eq!(
            usage[0]["usage_cache_creation_input_tokens"],
            serde_json::json!(200)
        );

        // sessions table: one row per top-level session file
        let sessions = engine
            .query(
                "SELECT session_id, project, title, pr_number, pr_url, subagent_count, \
                 user_message_count, assistant_message_count, total_input_tokens, \
                 total_output_tokens FROM sessions ORDER BY session_id",
            )
            .expect("sessions");
        assert_eq!(sessions.len(), 2);

        let legacy = &sessions[0];
        assert_eq!(legacy["session_id"], serde_json::json!("old"));
        assert_eq!(legacy["project"], serde_json::Value::Null);
        assert_eq!(legacy["subagent_count"], serde_json::json!(0));

        let rich = &sessions[1];
        assert_eq!(rich["session_id"], serde_json::json!("sess-rich"));
        assert_eq!(rich["project"], serde_json::json!(slug));
        assert_eq!(rich["title"], serde_json::json!("Fix the widget"));
        assert_eq!(rich["pr_number"], serde_json::json!(42));
        assert_eq!(
            rich["pr_url"],
            serde_json::json!("https://github.com/org/repo/pull/42")
        );
        assert_eq!(rich["subagent_count"], serde_json::json!(1));
        assert_eq!(rich["user_message_count"], serde_json::json!(1));
        assert_eq!(rich["assistant_message_count"], serde_json::json!(1));
        assert_eq!(rich["total_input_tokens"], serde_json::json!(6));
        assert_eq!(rich["total_output_tokens"], serde_json::json!(127));
    }
}
