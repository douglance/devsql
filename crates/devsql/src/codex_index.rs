use crate::Result;
use ccql::datasources::codex_journal::{
    discover_codex_journals, read_first_journal_record, visit_journal_records, CodexJournalFile,
    CodexJournalRecord, JournalState,
};
use chrono::Utc;
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyncStats {
    pub parsed_journals: usize,
    pub parsed_records: usize,
    pub unchanged_journals: usize,
    pub pruned_threads: usize,
}

pub(crate) struct CodexIndex {
    conn: Connection,
    codex_home: PathBuf,
    cache_path: PathBuf,
}

impl CodexIndex {
    pub(crate) fn open(codex_home: &Path) -> Result<Self> {
        let cache_root = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("devsql-cache"))
            .join("devsql")
            .join("codex-index");
        let canonical_home = codex_home
            .canonicalize()
            .unwrap_or_else(|_| codex_home.to_path_buf());
        let digest = Sha256::digest(canonical_home.to_string_lossy().as_bytes());
        let cache_path = cache_root.join(format!("{digest:x}.sqlite"));
        Self::open_at(codex_home, &cache_path)
    }

    pub(crate) fn open_at(codex_home: &Path, cache_path: &Path) -> Result<Self> {
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
            set_private_directory_permissions(parent)?;
        }
        let mut conn = match open_cache_connection(cache_path) {
            Ok(conn) => conn,
            Err(error) if is_corrupt_cache_error(&error) => {
                remove_cache_files(cache_path)?;
                open_cache_connection(cache_path)?
            }
            Err(error) => return Err(error),
        };
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            drop(conn);
            remove_cache_files(cache_path)?;
            conn = open_cache_connection(cache_path)?;
        }
        create_schema(&conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            conn,
            codex_home: codex_home.to_path_buf(),
            cache_path: cache_path.to_path_buf(),
        })
    }

    pub(crate) fn sync(&mut self) -> Result<SyncStats> {
        if !self.codex_home.exists() {
            return Ok(SyncStats::default());
        }
        let journals = discover_codex_journals(&self.codex_home)?;
        let mut stats = SyncStats::default();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut seen_threads = HashSet::new();
        let mut seen_paths = HashSet::new();
        let mut all_journals_readable = true;

        for journal in journals {
            seen_paths.insert(journal.path.to_string_lossy().into_owned());
            tx.execute_batch("SAVEPOINT codex_journal_sync")?;
            match sync_journal(&tx, &journal, &mut stats) {
                Ok(Some(thread_id)) => {
                    tx.execute_batch("RELEASE SAVEPOINT codex_journal_sync")?;
                    seen_threads.insert(thread_id);
                }
                Ok(None) => {
                    tx.execute_batch("RELEASE SAVEPOINT codex_journal_sync")?;
                }
                Err(error) => {
                    tx.execute_batch(
                        "ROLLBACK TO SAVEPOINT codex_journal_sync;
                         RELEASE SAVEPOINT codex_journal_sync;",
                    )?;
                    all_journals_readable = false;
                    tx.execute(
                        "DELETE FROM codex_ingest_errors
                         WHERE journal_path = ?1 AND error_kind = 'journal'",
                        [journal.path.to_string_lossy().as_ref()],
                    )?;
                    record_ingest_error(
                        &tx,
                        &journal.path,
                        None,
                        None,
                        "journal",
                        &error.to_string(),
                    )?;
                }
            }
        }

        if all_journals_readable {
            let cached_threads = {
                let mut statement = tx.prepare("SELECT thread_id FROM source_files")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for thread_id in cached_threads {
                if !seen_threads.contains(&thread_id) {
                    stats.pruned_threads += tx.execute(
                        "DELETE FROM codex_threads WHERE thread_id = ?1",
                        [thread_id],
                    )?;
                }
            }
            let error_paths = {
                let mut statement =
                    tx.prepare("SELECT DISTINCT journal_path FROM codex_ingest_errors")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            for path in error_paths {
                if !seen_paths.contains(&path) {
                    tx.execute(
                        "DELETE FROM codex_ingest_errors WHERE journal_path = ?1",
                        [path],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn cache_path(&self) -> &Path {
        &self.cache_path
    }
}

fn open_cache_connection(cache_path: &Path) -> Result<Connection> {
    let conn = Connection::open(cache_path)?;
    set_private_file_permissions(cache_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(30))?;
    ensure_wal_mode(&conn)?;
    set_private_file_permissions(cache_path)?;
    set_cache_sidecar_permissions(cache_path)?;
    Ok(conn)
}

fn ensure_wal_mode(conn: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        let result = (|| -> rusqlite::Result<()> {
            let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
            if !mode.eq_ignore_ascii_case("wal") {
                conn.pragma_update(None, "journal_mode", "WAL")?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if is_busy_sql_error(&error) && started.elapsed() < Duration::from_secs(30) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn is_busy_sql_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn is_corrupt_cache_error(error: &crate::Error) -> bool {
    matches!(
        error,
        crate::Error::Sql(rusqlite::Error::SqliteFailure(code, _))
            if matches!(code.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

fn remove_cache_files(cache_path: &Path) -> Result<()> {
    for path in [
        cache_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", cache_path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", cache_path.to_string_lossy())),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn set_cache_sidecar_permissions(cache_path: &Path) -> Result<()> {
    for path in [
        PathBuf::from(format!("{}-wal", cache_path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", cache_path.to_string_lossy())),
    ] {
        if path.exists() {
            set_private_file_permissions(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Debug)]
struct SourceState {
    thread_id: String,
    size: u64,
    modified_ns: i64,
    fingerprint: String,
    last_complete_offset: u64,
    last_record_index: i64,
    compressed: bool,
}

fn sync_journal(
    tx: &Transaction<'_>,
    journal: &CodexJournalFile,
    stats: &mut SyncStats,
) -> Result<Option<String>> {
    let metadata = fs::metadata(&journal.path)?;
    let size = metadata.len();
    let modified_ns = modified_ns(&metadata);
    let path_text = journal.path.to_string_lossy().into_owned();
    let existing = tx
        .query_row(
            "SELECT thread_id, size, modified_ns, leading_fingerprint,
                    last_complete_offset, last_record_index, compressed
             FROM source_files WHERE journal_path = ?1",
            [&path_text],
            |row| {
                Ok(SourceState {
                    thread_id: row.get(0)?,
                    size: row.get::<_, i64>(1)? as u64,
                    modified_ns: row.get(2)?,
                    fingerprint: row.get(3)?,
                    last_complete_offset: row.get::<_, i64>(4)? as u64,
                    last_record_index: row.get(5)?,
                    compressed: row.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()?;

    if let Some(existing) = &existing {
        if existing.size == size && existing.modified_ns == modified_ns {
            stats.unchanged_journals += 1;
            return Ok(Some(existing.thread_id.clone()));
        }
    }

    let fingerprint = leading_fingerprint(journal)?;
    let append = existing.as_ref().is_some_and(|old| {
        !journal.compressed && !old.compressed && old.fingerprint == fingerprint && size >= old.size
    });
    let thread_id = if let Some(existing) = &existing {
        existing.thread_id.clone()
    } else {
        journal_thread_id(journal)?
    };

    if append {
        let old = existing.as_ref().expect("append requires existing source");
        let progress = parse_journal(
            tx,
            journal,
            &thread_id,
            old.last_complete_offset,
            old.last_record_index + 1,
            stats,
        )?;
        update_source_file(
            tx,
            journal,
            &thread_id,
            size,
            modified_ns,
            &fingerprint,
            progress.last_complete_offset,
            progress.last_record_index,
        )?;
    } else {
        tx.execute(
            "DELETE FROM codex_ingest_errors
             WHERE journal_path = ?1 OR thread_id = ?2",
            params![path_text, thread_id],
        )?;
        tx.execute(
            "DELETE FROM codex_threads WHERE thread_id = ?1",
            [&thread_id],
        )?;
        insert_thread_shell(tx, journal, &thread_id)?;
        let progress = parse_journal(tx, journal, &thread_id, 0, 0, stats)?;
        update_source_file(
            tx,
            journal,
            &thread_id,
            size,
            modified_ns,
            &fingerprint,
            progress.last_complete_offset,
            progress.last_record_index,
        )?;
    }
    recompute_thread(tx, &thread_id)?;
    stats.parsed_journals += 1;
    Ok(Some(thread_id))
}

fn parse_journal(
    tx: &Transaction<'_>,
    journal: &CodexJournalFile,
    thread_id: &str,
    start_offset: u64,
    start_record_index: i64,
    stats: &mut SyncStats,
) -> Result<ccql::datasources::codex_journal::JournalProgress> {
    let source_path = journal.path.to_string_lossy().into_owned();
    let progress = visit_journal_records(journal, start_offset, start_record_index, |record| {
        stats.parsed_records += 1;
        if let Some(error) = &record.parse_error {
            tx.execute(
                "INSERT OR REPLACE INTO codex_events
                     (thread_id, record_index, source_path)
                     VALUES (?1, ?2, ?3)",
                params![thread_id, record.record_index, source_path],
            )
            .map_err(|error| sql_to_io(error.into()))?;
            record_ingest_error(
                tx,
                &journal.path,
                Some(thread_id),
                Some(record.record_index),
                "json",
                error,
            )
            .map_err(sql_to_io)?;
            return Ok(());
        }
        normalize_record(tx, thread_id, &source_path, &record).map_err(sql_to_io)
    })?;
    Ok(progress)
}

fn normalize_record(
    tx: &Transaction<'_>,
    thread_id: &str,
    source_path: &str,
    record: &CodexJournalRecord,
) -> Result<()> {
    let Some(value) = record.value.as_ref() else {
        return Ok(());
    };
    let timestamp = value.get("timestamp").and_then(Value::as_str);
    let record_type = value.get("type").and_then(Value::as_str);
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let payload_type = payload.get("type").and_then(Value::as_str);
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| payload.get("message")?.get("role")?.as_str());
    let call_id = payload.get("call_id").and_then(Value::as_str);

    tx.execute(
        "INSERT OR REPLACE INTO codex_events
         (thread_id, record_index, timestamp, record_type, payload_type, role, call_id, source_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            thread_id,
            record.record_index,
            timestamp,
            record_type,
            payload_type,
            role,
            call_id,
            source_path
        ],
    )?;

    if record_type == Some("session_meta") {
        update_thread_metadata(tx, thread_id, payload, timestamp)?;
    }
    if let Some(message) = normalize_message(record_type, payload_type, payload) {
        tx.execute(
            "INSERT OR REPLACE INTO codex_messages
             (thread_id, record_index, timestamp, role, text, content_json, is_canonical, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                thread_id,
                record.record_index,
                timestamp,
                message.role,
                message.text,
                message.content_json,
                i64::from(message.canonical),
                source_path
            ],
        )?;
    }
    if is_tool_call(payload_type) {
        normalize_tool_call(
            tx,
            thread_id,
            source_path,
            record.record_index,
            timestamp,
            payload,
        )?;
    } else if is_tool_output(payload_type) {
        normalize_tool_output(
            tx,
            thread_id,
            source_path,
            record.record_index,
            timestamp,
            payload,
        )?;
    }
    if record_type == Some("compacted") {
        tx.execute(
            "INSERT OR REPLACE INTO codex_compactions
             (thread_id, record_index, timestamp, window_id, previous_window_id,
              first_window_id, window_number, summary_text, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                thread_id,
                record.record_index,
                timestamp,
                string_field(payload, "window_id"),
                string_field(payload, "previous_window_id"),
                string_field(payload, "first_window_id"),
                payload.get("window_number").and_then(Value::as_i64),
                string_field(payload, "message").or_else(|| string_field(payload, "summary")),
                source_path
            ],
        )?;
    }
    Ok(())
}

struct NormalizedMessage {
    role: String,
    text: String,
    content_json: String,
    canonical: bool,
}

fn normalize_message(
    record_type: Option<&str>,
    payload_type: Option<&str>,
    payload: &Value,
) -> Option<NormalizedMessage> {
    let canonical = record_type == Some("response_item")
        && matches!(payload_type, Some("message" | "agent_message"));
    let event_message = record_type == Some("event_msg")
        && matches!(payload_type, Some("user_message" | "agent_message"));
    let legacy = record_type == Some("message");
    if !canonical && !event_message && !legacy {
        return None;
    }
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| match payload_type {
            Some("user_message") => Some("user".to_string()),
            Some("agent_message") => Some("assistant".to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "unknown".to_string());
    let content = payload
        .get("content")
        .or_else(|| payload.get("message"))
        .unwrap_or(payload);
    let text = extract_text(content);
    if text.is_empty() {
        return None;
    }
    Some(NormalizedMessage {
        role,
        text,
        content_json: serde_json::to_string(content).unwrap_or_default(),
        canonical: canonical || legacy,
    })
}

fn extract_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .map(extract_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => {
            for key in ["text", "content", "message"] {
                if let Some(value) = object.get(key) {
                    let text = extract_text(value);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

fn is_tool_call(payload_type: Option<&str>) -> bool {
    matches!(
        payload_type,
        Some("function_call" | "custom_tool_call" | "tool_search_call")
    )
}

fn is_tool_output(payload_type: Option<&str>) -> bool {
    matches!(
        payload_type,
        Some("function_call_output" | "custom_tool_call_output" | "tool_search_output")
    )
}

fn normalize_tool_call(
    tx: &Transaction<'_>,
    thread_id: &str,
    source_path: &str,
    record_index: i64,
    timestamp: Option<&str>,
    payload: &Value,
) -> Result<()> {
    let call_id = payload
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("record:{record_index}"));
    let tool_name = payload
        .get("name")
        .or_else(|| payload.get("tool_name"))
        .and_then(Value::as_str);
    let arguments = payload
        .get("arguments")
        .or_else(|| payload.get("input"))
        .unwrap_or(&Value::Null);
    let arguments_json = arguments
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(arguments).unwrap_or_default());
    let parsed_arguments = arguments
        .as_str()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .unwrap_or_else(|| arguments.clone());
    let cmd = tool_name.and_then(|name| extract_command(name, &parsed_arguments));
    tx.execute(
        "INSERT INTO codex_tool_executions
         (thread_id, call_id, call_record_index, tool_name, arguments_json, cmd,
          called_at, source_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(thread_id, call_id) DO UPDATE SET
           call_record_index = excluded.call_record_index,
           tool_name = excluded.tool_name,
           arguments_json = excluded.arguments_json,
           cmd = excluded.cmd,
           called_at = excluded.called_at,
           source_path = excluded.source_path",
        params![
            thread_id,
            call_id,
            record_index,
            tool_name,
            arguments_json,
            cmd,
            timestamp,
            source_path
        ],
    )?;
    Ok(())
}

fn normalize_tool_output(
    tx: &Transaction<'_>,
    thread_id: &str,
    source_path: &str,
    record_index: i64,
    timestamp: Option<&str>,
    payload: &Value,
) -> Result<()> {
    let call_id = payload
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("output-record:{record_index}"));
    let output = payload
        .get("output")
        .or_else(|| payload.get("result"))
        .unwrap_or(&Value::Null);
    let output_text = output
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(output).unwrap_or_default());
    tx.execute(
        "INSERT INTO codex_tool_executions
         (thread_id, call_id, output_record_index, output_text, completed_at, source_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(thread_id, call_id) DO UPDATE SET
           output_record_index = excluded.output_record_index,
           output_text = excluded.output_text,
           completed_at = excluded.completed_at,
           source_path = excluded.source_path",
        params![
            thread_id,
            call_id,
            record_index,
            output_text,
            timestamp,
            source_path
        ],
    )?;
    Ok(())
}

fn extract_command(tool_name: &str, arguments: &Value) -> Option<String> {
    match tool_name {
        "exec_command" | "shell" => arguments
            .get("cmd")
            .or_else(|| arguments.get("command"))
            .and_then(|command| match command {
                Value::String(command) => Some(command.clone()),
                Value::Array(parts) => Some(
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            }),
        _ => None,
    }
}

fn insert_thread_shell(
    tx: &Transaction<'_>,
    journal: &CodexJournalFile,
    thread_id: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO codex_threads
         (thread_id, state, compressed, journal_path)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            thread_id,
            state_text(journal.state),
            i64::from(journal.compressed),
            journal.path.to_string_lossy()
        ],
    )?;
    Ok(())
}

fn update_thread_metadata(
    tx: &Transaction<'_>,
    thread_id: &str,
    payload: &Value,
    timestamp: Option<&str>,
) -> Result<()> {
    let source = payload.get("source").unwrap_or(&Value::Null);
    let source_kind = payload
        .get("thread_source")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| source.as_str().map(str::to_owned))
        .or_else(|| source.as_object()?.keys().next().cloned());
    let parent_thread_id = find_string_key(source, "parent_thread_id");
    let parent_record_index =
        find_i64_key(source, "parent_record_index").or_else(|| find_i64_key(source, "turn_index"));
    let agent_path = find_string_key(source, "agent_path");
    let git_branch = payload
        .get("git")
        .and_then(|git| git.get("branch"))
        .and_then(Value::as_str);
    tx.execute(
        "UPDATE codex_threads SET
           parent_thread_id = COALESCE(?2, parent_thread_id),
           parent_record_index = COALESCE(?3, parent_record_index),
           source_kind = COALESCE(?4, source_kind),
           source_json = ?5,
           agent_path = COALESCE(?6, agent_path),
           cwd = COALESCE(?7, cwd),
           git_branch = COALESCE(?8, git_branch),
           cli_version = COALESCE(?9, cli_version),
           started_at = COALESCE(?10, started_at)
         WHERE thread_id = ?1",
        params![
            thread_id,
            parent_thread_id,
            parent_record_index,
            source_kind,
            serde_json::to_string(source).ok(),
            agent_path,
            string_field(payload, "cwd"),
            git_branch,
            string_field(payload, "cli_version"),
            string_field(payload, "timestamp").or_else(|| timestamp.map(str::to_owned))
        ],
    )?;
    Ok(())
}

fn find_string_key(value: &Value, target: &str) -> Option<String> {
    match value {
        Value::Object(object) => {
            if let Some(value) = object.get(target).and_then(Value::as_str) {
                return Some(value.to_string());
            }
            object
                .values()
                .find_map(|value| find_string_key(value, target))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_string_key(value, target)),
        _ => None,
    }
}

fn find_i64_key(value: &Value, target: &str) -> Option<i64> {
    match value {
        Value::Object(object) => {
            if let Some(value) = object.get(target).and_then(Value::as_i64) {
                return Some(value);
            }
            object
                .values()
                .find_map(|value| find_i64_key(value, target))
        }
        Value::Array(values) => values.iter().find_map(|value| find_i64_key(value, target)),
        _ => None,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    })
}

fn recompute_thread(tx: &Transaction<'_>, thread_id: &str) -> Result<()> {
    tx.execute(
        "UPDATE codex_threads SET
           started_at = COALESCE(started_at, (
             SELECT MIN(timestamp) FROM codex_events WHERE thread_id = ?1
           )),
           last_event_at = (
             SELECT MAX(timestamp) FROM codex_events WHERE thread_id = ?1
           ),
           first_user_text = (
             SELECT text FROM codex_messages
             WHERE thread_id = ?1 AND role = 'user' AND is_canonical = 1
             ORDER BY record_index LIMIT 1
           ),
           event_count = (
             SELECT COUNT(*) FROM codex_events WHERE thread_id = ?1
           ),
           user_message_count = (
             SELECT COUNT(*) FROM codex_messages
             WHERE thread_id = ?1 AND role = 'user' AND is_canonical = 1
           ),
           assistant_message_count = (
             SELECT COUNT(*) FROM codex_messages
             WHERE thread_id = ?1 AND role = 'assistant' AND is_canonical = 1
           ),
           tool_call_count = (
             SELECT COUNT(*) FROM codex_tool_executions
             WHERE thread_id = ?1 AND call_record_index IS NOT NULL
           ),
           compaction_count = (
             SELECT COUNT(*) FROM codex_compactions WHERE thread_id = ?1
           )
         WHERE thread_id = ?1",
        [thread_id],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_source_file(
    tx: &Transaction<'_>,
    journal: &CodexJournalFile,
    thread_id: &str,
    size: u64,
    modified_ns: i64,
    fingerprint: &str,
    last_complete_offset: u64,
    last_record_index: i64,
) -> Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO source_files
         (thread_id, journal_path, state, compressed, size, modified_ns,
          leading_fingerprint, last_complete_offset, last_record_index)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            thread_id,
            journal.path.to_string_lossy(),
            state_text(journal.state),
            i64::from(journal.compressed),
            size as i64,
            modified_ns,
            fingerprint,
            last_complete_offset as i64,
            last_record_index
        ],
    )?;
    update_thread_location(tx, thread_id, journal)
}

fn update_thread_location(
    tx: &Transaction<'_>,
    thread_id: &str,
    journal: &CodexJournalFile,
) -> Result<()> {
    let path = journal.path.to_string_lossy();
    tx.execute(
        "UPDATE codex_threads SET state = ?2, compressed = ?3, journal_path = ?4
         WHERE thread_id = ?1",
        params![
            thread_id,
            state_text(journal.state),
            i64::from(journal.compressed),
            path
        ],
    )?;
    for table in [
        "codex_events",
        "codex_messages",
        "codex_tool_executions",
        "codex_compactions",
    ] {
        tx.execute(
            &format!("UPDATE {table} SET source_path = ?2 WHERE thread_id = ?1"),
            params![thread_id, path],
        )?;
    }
    Ok(())
}

fn journal_thread_id(journal: &CodexJournalFile) -> Result<String> {
    if let Some(record) = read_first_journal_record(journal)? {
        if let Some(payload) = record.value.as_ref().and_then(|value| value.get("payload")) {
            if let Some(thread_id) = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(Value::as_str)
            {
                return Ok(thread_id.to_string());
            }
        }
    }
    let name = journal
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let stem = name
        .strip_suffix(".jsonl.zst")
        .or_else(|| name.strip_suffix(".jsonl"))
        .unwrap_or(name);
    if stem.len() >= 36 {
        let candidate = &stem[stem.len() - 36..];
        if candidate
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
        {
            return Ok(candidate.to_string());
        }
    }
    Ok(stem
        .rsplit_once('-')
        .map(|(_, suffix)| suffix)
        .unwrap_or(stem)
        .to_string())
}

fn leading_fingerprint(journal: &CodexJournalFile) -> Result<String> {
    let bytes = read_first_journal_record(journal)?
        .and_then(|record| record.value)
        .and_then(|value| serde_json::to_vec(&value).ok())
        .unwrap_or_default();
    let digest = Sha256::digest(bytes);
    Ok(format!("{digest:x}"))
}

fn modified_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn state_text(state: JournalState) -> &'static str {
    match state {
        JournalState::Active => "active",
        JournalState::Archived => "archived",
    }
}

fn record_ingest_error(
    tx: &Transaction<'_>,
    path: &Path,
    thread_id: Option<&str>,
    record_index: Option<i64>,
    error_kind: &str,
    message: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO codex_ingest_errors
         (journal_path, thread_id, record_index, error_kind, message, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            path.to_string_lossy(),
            thread_id,
            record_index,
            error_kind,
            message,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn sql_to_io(error: crate::Error) -> std::io::Error {
    std::io::Error::other(error)
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR REPLACE INTO index_meta (key, value)
        VALUES ('schema_version', '2');

        CREATE TABLE IF NOT EXISTS codex_threads (
            thread_id TEXT PRIMARY KEY,
            parent_thread_id TEXT,
            parent_record_index INTEGER,
            source_kind TEXT,
            source_json TEXT,
            agent_path TEXT,
            cwd TEXT,
            git_branch TEXT,
            cli_version TEXT,
            state TEXT NOT NULL,
            compressed INTEGER NOT NULL,
            journal_path TEXT NOT NULL,
            started_at TEXT,
            last_event_at TEXT,
            first_user_text TEXT,
            event_count INTEGER NOT NULL DEFAULT 0,
            user_message_count INTEGER NOT NULL DEFAULT 0,
            assistant_message_count INTEGER NOT NULL DEFAULT 0,
            tool_call_count INTEGER NOT NULL DEFAULT 0,
            compaction_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS source_files (
            thread_id TEXT PRIMARY KEY REFERENCES codex_threads(thread_id) ON DELETE CASCADE,
            journal_path TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL,
            compressed INTEGER NOT NULL,
            size INTEGER NOT NULL,
            modified_ns INTEGER NOT NULL,
            leading_fingerprint TEXT NOT NULL,
            last_complete_offset INTEGER NOT NULL,
            last_record_index INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS codex_events (
            thread_id TEXT NOT NULL REFERENCES codex_threads(thread_id) ON DELETE CASCADE,
            record_index INTEGER NOT NULL,
            timestamp TEXT,
            record_type TEXT,
            payload_type TEXT,
            role TEXT,
            call_id TEXT,
            source_path TEXT NOT NULL,
            PRIMARY KEY (thread_id, record_index)
        );

        CREATE TABLE IF NOT EXISTS codex_messages (
            thread_id TEXT NOT NULL REFERENCES codex_threads(thread_id) ON DELETE CASCADE,
            record_index INTEGER NOT NULL,
            timestamp TEXT,
            role TEXT,
            text TEXT,
            content_json TEXT,
            is_canonical INTEGER NOT NULL,
            source_path TEXT NOT NULL,
            PRIMARY KEY (thread_id, record_index)
        );

        CREATE TABLE IF NOT EXISTS codex_tool_executions (
            thread_id TEXT NOT NULL REFERENCES codex_threads(thread_id) ON DELETE CASCADE,
            call_id TEXT NOT NULL,
            call_record_index INTEGER,
            output_record_index INTEGER,
            tool_name TEXT,
            arguments_json TEXT,
            cmd TEXT,
            output_text TEXT,
            called_at TEXT,
            completed_at TEXT,
            cwd TEXT,
            source_path TEXT NOT NULL,
            PRIMARY KEY (thread_id, call_id)
        );

        CREATE TABLE IF NOT EXISTS codex_compactions (
            thread_id TEXT NOT NULL REFERENCES codex_threads(thread_id) ON DELETE CASCADE,
            record_index INTEGER NOT NULL,
            timestamp TEXT,
            window_id TEXT,
            previous_window_id TEXT,
            first_window_id TEXT,
            window_number INTEGER,
            summary_text TEXT,
            source_path TEXT NOT NULL,
            PRIMARY KEY (thread_id, record_index)
        );

        CREATE TABLE IF NOT EXISTS codex_ingest_errors (
            journal_path TEXT NOT NULL,
            thread_id TEXT,
            record_index INTEGER,
            error_kind TEXT NOT NULL,
            message TEXT NOT NULL,
            observed_at TEXT NOT NULL
        );
        ",
    )?;
    let _ = SCHEMA_VERSION;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, contents).expect("write");
    }

    #[test]
    fn sync_normalizes_a_thread_messages_tools_and_compaction() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home.join("sessions/2026/07/27/rollout-thread-1.jsonl");
        write(
            &journal,
            concat!(
                "{\"timestamp\":\"2026-07-27T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\",\"cwd\":\"/repo\",\"cli_version\":\"0.145.0\",\"thread_source\":\"user\",\"git\":{\"branch\":\"main\"}}}\n",
                "{\"timestamp\":\"2026-07-27T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"debug auth callback\"}]}}\n",
                "{\"timestamp\":\"2026-07-27T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"call_id\":\"call-1\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\"}}\n",
                "{\"timestamp\":\"2026-07-27T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-1\",\"output\":\"tests passed\"}}\n",
                "{\"timestamp\":\"2026-07-27T10:00:04Z\",\"type\":\"compacted\",\"payload\":{\"window_id\":\"w2\",\"previous_window_id\":\"w1\",\"window_number\":2,\"message\":\"auth summary\"}}\n"
            ),
        );
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");

        let stats = index.sync().expect("sync");

        assert_eq!(stats.parsed_journals, 1);
        assert_eq!(stats.parsed_records, 5);
        let thread: (String, String, i64, i64, i64) = index
            .connection()
            .query_row(
                "SELECT cwd, first_user_text, event_count, tool_call_count, compaction_count
                 FROM codex_threads WHERE thread_id = 'thread-1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("thread");
        assert_eq!(
            thread,
            ("/repo".into(), "debug auth callback".into(), 5, 1, 1)
        );
        let tool: (String, String, String) = index
            .connection()
            .query_row(
                "SELECT tool_name, cmd, output_text FROM codex_tool_executions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("tool");
        assert_eq!(
            tool,
            (
                "exec_command".into(),
                "cargo test".into(),
                "tests passed".into()
            )
        );
    }

    #[test]
    fn second_sync_skips_unchanged_journals_and_append_reads_only_new_records() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home.join("sessions/rollout-thread-2.jsonl");
        write(
            &journal,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-2\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"first\"}]}}\n"
            ),
        );
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index.sync().expect("first sync");

        let unchanged = index.sync().expect("unchanged sync");
        assert_eq!(unchanged.unchanged_journals, 1);
        assert_eq!(unchanged.parsed_records, 0);

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .expect("append");
        file.write_all(
            b"{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"second\"}]}}\n",
        )
        .expect("append line");

        let appended = index.sync().expect("append sync");
        assert_eq!(appended.parsed_records, 1);
        let count: i64 = index
            .connection()
            .query_row(
                "SELECT event_count FROM codex_threads WHERE thread_id = 'thread-2'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 3);
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_compressed_journal_is_not_reopened() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home
            .join("archived_sessions")
            .join("rollout-unchanged.jsonl.zst");
        fs::create_dir_all(journal.parent().expect("parent")).expect("archive dir");
        let output = fs::File::create(&journal).expect("create");
        let mut encoder = zstd::stream::write::Encoder::new(output, 0).expect("encoder");
        encoder
            .write_all(b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"unchanged-thread\"}}\n")
            .expect("compress");
        encoder.finish().expect("finish");

        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index.sync().expect("initial sync");

        fs::set_permissions(&journal, fs::Permissions::from_mode(0o000)).expect("make unreadable");
        let changes_before = index.connection().total_changes();
        let unchanged = index
            .sync()
            .expect("metadata-identical journal should not be reopened");

        assert_eq!(unchanged.unchanged_journals, 1);
        assert_eq!(unchanged.parsed_records, 0);
        assert_eq!(index.connection().total_changes(), changes_before);
    }

    #[test]
    fn schema_version_change_rebuilds_the_disposable_cache() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        fs::create_dir_all(&codex_home).expect("codex home");
        let cache = temp.path().join("cache/index.sqlite");
        let index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index
            .connection()
            .pragma_update(None, "user_version", 99)
            .expect("version");
        index
            .connection()
            .execute("CREATE TABLE stale_cache_data (value TEXT)", [])
            .expect("stale table");
        drop(index);

        let rebuilt = CodexIndex::open_at(&codex_home, &cache).expect("reopen");

        let stale_count: i64 = rebuilt
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'stale_cache_data'",
                [],
                |row| row.get(0),
            )
            .expect("schema");
        assert_eq!(stale_count, 0);
        let version: i64 = rebuilt
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn corrupt_disposable_cache_is_recreated() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        fs::create_dir_all(&codex_home).expect("codex home");
        let cache = temp.path().join("cache/index.sqlite");
        fs::create_dir_all(cache.parent().expect("parent")).expect("cache dir");
        fs::write(&cache, b"not a sqlite database").expect("corrupt cache");

        let rebuilt = CodexIndex::open_at(&codex_home, &cache).expect("rebuild corrupt cache");

        let table_count: i64 = rebuilt
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'codex_threads'",
                [],
                |row| row.get(0),
            )
            .expect("schema");
        assert_eq!(table_count, 1);
    }

    #[test]
    fn concurrent_fresh_cache_initialization_waits_for_wal_mode() {
        use std::sync::{Arc, Barrier};

        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        fs::create_dir_all(&codex_home).expect("codex home");
        let cache = temp.path().join("cache/index.sqlite");
        let barrier = Arc::new(Barrier::new(8));

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let barrier = Arc::clone(&barrier);
                let codex_home = &codex_home;
                let cache = &cache;
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    let mut index =
                        CodexIndex::open_at(codex_home, cache).expect("concurrent open");
                    index.sync().expect("concurrent sync");
                }));
            }
            for handle in handles {
                handle.join().expect("thread");
            }
        });
    }

    #[test]
    fn archive_and_compression_transition_preserves_thread_identity() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let active = codex_home.join("sessions/rollout-archive.jsonl");
        let contents = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"archive-thread\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"keep me\"}]}}\n"
        );
        write(&active, contents);
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index.sync().expect("active sync");

        let archived = codex_home
            .join("archived_sessions")
            .join("rollout-archive.jsonl.zst");
        fs::create_dir_all(archived.parent().expect("parent")).expect("archive dir");
        let output = fs::File::create(&archived).expect("create archive");
        let mut encoder = zstd::stream::write::Encoder::new(output, 0).expect("encoder");
        encoder.write_all(contents.as_bytes()).expect("compress");
        encoder.finish().expect("finish");
        fs::remove_file(&active).expect("remove active");

        index.sync().expect("archive sync");

        let thread: (String, i64, String, i64) = index
            .connection()
            .query_row(
                "SELECT state, compressed, journal_path, user_message_count
                 FROM codex_threads WHERE thread_id = 'archive-thread'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("thread");
        assert_eq!(thread.0, "archived");
        assert_eq!(thread.1, 1);
        assert_eq!(thread.2, archived.to_string_lossy());
        assert_eq!(thread.3, 1);
    }

    #[test]
    fn deleted_journal_prunes_derived_thread_rows() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home.join("sessions/rollout-delete.jsonl");
        write(
            &journal,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"delete-thread\"}}\n",
        );
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index.sync().expect("initial sync");
        fs::remove_file(&journal).expect("delete journal");

        let stats = index.sync().expect("delete sync");

        assert_eq!(stats.pruned_threads, 1);
        let count: i64 = index
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM codex_threads WHERE thread_id = 'delete-thread'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn unreadable_changed_journal_keeps_last_good_rows_and_records_error() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home
            .join("archived_sessions")
            .join("rollout-broken.jsonl.zst");
        fs::create_dir_all(journal.parent().expect("parent")).expect("archive dir");
        let output = fs::File::create(&journal).expect("create");
        let mut encoder = zstd::stream::write::Encoder::new(output, 0).expect("encoder");
        encoder
            .write_all(
                concat!(
                    "{\"type\":\"session_meta\",\"payload\":{\"id\":\"broken-thread\"}}\n",
                    "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"last good\"}]}}\n"
                )
                .as_bytes(),
            )
            .expect("compress");
        encoder.finish().expect("finish");
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");
        index.sync().expect("initial sync");
        fs::write(&journal, b"invalid zstd data").expect("corrupt journal");

        index.sync().expect("failed journal is nonfatal");

        let message_count: i64 = index
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM codex_messages WHERE thread_id = 'broken-thread'",
                [],
                |row| row.get(0),
            )
            .expect("messages");
        assert_eq!(message_count, 1);
        let error_count: i64 = index
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM codex_ingest_errors
                 WHERE journal_path = ?1 AND error_kind = 'journal'",
                [journal.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .expect("errors");
        assert_eq!(error_count, 1);
    }

    #[test]
    fn extracts_subagent_lineage_without_duplicating_event_messages() {
        let temp = tempfile::tempdir().expect("temp");
        let codex_home = temp.path().join("codex");
        let journal = codex_home.join("sessions/rollout-lineage.jsonl");
        write(
            &journal,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child-thread\",\"session_id\":\"parent-thread\",\"thread_source\":\"subagent\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent-thread\",\"agent_path\":\"/root/research\"}}}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"same message\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"same message\"}]}}\n",
                "{\"type\":\"world_state\",\"payload\":{\"future_field\":true}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"reasoning\",\"encrypted_content\":\"opaque\"}}\n"
            ),
        );
        let cache = temp.path().join("cache/index.sqlite");
        let mut index = CodexIndex::open_at(&codex_home, &cache).expect("open");

        index.sync().expect("sync");

        let thread: (String, String, String, i64, i64) = index
            .connection()
            .query_row(
                "SELECT parent_thread_id, source_kind, agent_path,
                        event_count, user_message_count
                 FROM codex_threads WHERE thread_id = 'child-thread'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("thread");
        assert_eq!(
            thread,
            (
                "parent-thread".into(),
                "subagent".into(),
                "/root/research".into(),
                5,
                1
            )
        );
        let message_rows: i64 = index
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM codex_messages WHERE thread_id = 'child-thread'",
                [],
                |row| row.get(0),
            )
            .expect("messages");
        assert_eq!(message_rows, 2);
    }

    #[test]
    fn filename_fallback_keeps_the_complete_rollout_uuid() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join(
            "sessions/rollout-2026-07-27T09-00-00-019fa06b-6982-78c0-9d88-4810fc6cfdd4.jsonl",
        );
        write(&path, "not-json\n");
        let journal = CodexJournalFile {
            path,
            state: JournalState::Active,
            compressed: false,
        };

        let thread_id = journal_thread_id(&journal).expect("thread id");

        assert_eq!(thread_id, "019fa06b-6982-78c0-9d88-4810fc6cfdd4");
    }
}
