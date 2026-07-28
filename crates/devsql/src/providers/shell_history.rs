//! Read-only shell-history providers normalized into one SQLite table.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags};
use std::path::{Path, PathBuf};

use crate::Result;

const CREATE_TABLE: &str = "CREATE TABLE shell_history (
    source TEXT NOT NULL,
    source_id TEXT,
    source_order INTEGER NOT NULL,
    timestamp TEXT,
    duration_ms INTEGER,
    exit_code INTEGER,
    command TEXT NOT NULL,
    cwd TEXT,
    session_id TEXT,
    hostname TEXT,
    history_path TEXT NOT NULL
)";

#[derive(Debug)]
struct ShellEntry {
    source: &'static str,
    source_id: Option<String>,
    source_order: i64,
    timestamp: Option<String>,
    duration_ms: Option<i64>,
    exit_code: Option<i64>,
    command: String,
    cwd: Option<String>,
    session_id: Option<String>,
    hostname: Option<String>,
    history_path: String,
}

/// Rebuild `shell_history` from every available read-only source.
///
/// Shell histories are optional developer-local data. A missing, unreadable, or
/// incompatible source contributes no rows instead of failing the full query.
pub fn load(conn: &mut Connection) -> Result<()> {
    conn.execute("DROP TABLE IF EXISTS shell_history", [])?;
    conn.execute(CREATE_TABLE, [])?;

    let mut entries = Vec::new();
    if let Some(path) = atuin_history_path() {
        entries.extend(read_atuin(&path));
    }
    if let Some(path) = zsh_history_path() {
        entries.extend(read_zsh(&path));
    }
    if let Some(path) = bash_history_path() {
        entries.extend(read_bash(&path));
    }

    let tx = conn.transaction()?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO shell_history (
                source, source_id, source_order, timestamp, duration_ms,
                exit_code, command, cwd, session_id, hostname, history_path
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for entry in entries {
            insert.execute(params![
                entry.source,
                entry.source_id,
                entry.source_order,
                entry.timestamp,
                entry.duration_ms,
                entry.exit_code,
                entry.command,
                entry.cwd,
                entry.session_id,
                entry.hostname,
                entry.history_path,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn read_atuin(path: &Path) -> Vec<ShellEntry> {
    if !path.is_file() {
        return Vec::new();
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(source) = Connection::open_with_flags(path, flags) else {
        return Vec::new();
    };
    let Ok(mut statement) = source.prepare(
        "SELECT id, timestamp, duration, exit, command, cwd, session, hostname
         FROM history
         WHERE deleted_at IS NULL
         ORDER BY timestamp, id",
    ) else {
        return Vec::new();
    };

    let history_path = path.to_string_lossy().into_owned();
    let Ok(rows) = statement.query_map([], |row| {
        let timestamp_ns: i64 = row.get(1)?;
        let duration_ns: i64 = row.get(2)?;
        Ok(ShellEntry {
            source: "atuin",
            source_id: row.get(0)?,
            source_order: 0,
            timestamp: timestamp_from_nanos(timestamp_ns),
            duration_ms: Some(duration_ns / 1_000_000),
            exit_code: row.get(3)?,
            command: row.get(4)?,
            cwd: row.get(5)?,
            session_id: row.get(6)?,
            hostname: row.get(7)?,
            history_path: history_path.clone(),
        })
    }) else {
        return Vec::new();
    };

    rows.enumerate()
        .filter_map(|(index, row)| {
            row.ok().map(|mut entry| {
                entry.source_order = index as i64;
                entry
            })
        })
        .collect()
}

fn read_zsh(path: &Path) -> Vec<ShellEntry> {
    let Some(content) = read_lossy(path) else {
        return Vec::new();
    };
    let history_path = path.to_string_lossy().into_owned();
    let mut entries = Vec::new();
    let mut pending: Option<(Option<String>, Option<i64>, String)> = None;

    for line in content.lines() {
        if let Some((_, _, command)) = pending.as_mut() {
            command.push('\n');
            command.push_str(line);
            if !has_line_continuation(command) {
                let (timestamp, duration_ms, command) = pending.take().expect("pending command");
                entries.push(native_entry(
                    "zsh",
                    entries.len() as i64,
                    timestamp,
                    duration_ms,
                    command,
                    &history_path,
                ));
            }
            continue;
        }

        let (timestamp, duration_ms, command) = parse_zsh_line(line);
        if has_line_continuation(&command) {
            pending = Some((timestamp, duration_ms, command));
        } else {
            entries.push(native_entry(
                "zsh",
                entries.len() as i64,
                timestamp,
                duration_ms,
                command,
                &history_path,
            ));
        }
    }

    if let Some((timestamp, duration_ms, command)) = pending {
        entries.push(native_entry(
            "zsh",
            entries.len() as i64,
            timestamp,
            duration_ms,
            command,
            &history_path,
        ));
    }
    entries
}

fn has_line_continuation(command: &str) -> bool {
    command
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count()
        % 2
        == 1
}

fn parse_zsh_line(line: &str) -> (Option<String>, Option<i64>, String) {
    let Some(rest) = line.strip_prefix(": ") else {
        return (None, None, line.to_string());
    };
    let Some((metadata, command)) = rest.split_once(';') else {
        return (None, None, line.to_string());
    };
    let Some((epoch, duration)) = metadata.split_once(':') else {
        return (None, None, line.to_string());
    };
    let (Ok(epoch), Ok(duration)) = (epoch.parse::<i64>(), duration.parse::<i64>()) else {
        return (None, None, line.to_string());
    };
    (
        timestamp_from_seconds(epoch),
        duration.checked_mul(1_000),
        command.to_string(),
    )
}

fn read_bash(path: &Path) -> Vec<ShellEntry> {
    let Some(content) = read_lossy(path) else {
        return Vec::new();
    };
    let history_path = path.to_string_lossy().into_owned();
    let mut entries = Vec::new();
    let mut next_timestamp = None;

    for line in content.lines() {
        if let Some(epoch) = line
            .strip_prefix('#')
            .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
            .and_then(|value| value.parse::<i64>().ok())
        {
            next_timestamp = timestamp_from_seconds(epoch);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        entries.push(native_entry(
            "bash",
            entries.len() as i64,
            next_timestamp.take(),
            None,
            line.to_string(),
            &history_path,
        ));
    }
    entries
}

fn native_entry(
    source: &'static str,
    source_order: i64,
    timestamp: Option<String>,
    duration_ms: Option<i64>,
    command: String,
    history_path: &str,
) -> ShellEntry {
    ShellEntry {
        source,
        source_id: None,
        source_order,
        timestamp,
        duration_ms,
        exit_code: None,
        command,
        cwd: None,
        session_id: None,
        hostname: None,
        history_path: history_path.to_string(),
    }
}

fn read_lossy(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn timestamp_from_nanos(nanos: i64) -> Option<String> {
    let seconds = nanos.div_euclid(1_000_000_000);
    let subsecond_nanos = nanos.rem_euclid(1_000_000_000) as u32;
    DateTime::<Utc>::from_timestamp(seconds, subsecond_nanos)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn timestamp_from_seconds(seconds: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn atuin_history_path() -> Option<PathBuf> {
    if let Some(path) = env_path("DEVSQL_ATUIN_DB") {
        return Some(path);
    }

    if let Some(path) = atuin_config_path().and_then(|config| configured_atuin_db(&config)) {
        return Some(path);
    }

    if let Some(data_home) = env_path("XDG_DATA_HOME") {
        return Some(data_home.join("atuin/history.db"));
    }
    dirs::home_dir().map(|home| home.join(".local/share/atuin/history.db"))
}

fn atuin_config_path() -> Option<PathBuf> {
    if let Some(config_dir) = env_path("ATUIN_CONFIG_DIR") {
        return Some(config_dir.join("config.toml"));
    }
    if let Some(config_home) = env_path("XDG_CONFIG_HOME") {
        return Some(config_home.join("atuin/config.toml"));
    }
    dirs::home_dir().map(|home| home.join(".config/atuin/config.toml"))
}

fn configured_atuin_db(config_path: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    let raw = value.get("db_path")?.as_str()?;
    let expanded = expand_home(raw);
    if expanded.is_absolute() {
        Some(expanded)
    } else {
        config_path.parent().map(|parent| parent.join(expanded))
    }
}

fn zsh_history_path() -> Option<PathBuf> {
    if let Some(path) = env_path("DEVSQL_ZSH_HISTORY") {
        return Some(path);
    }
    if active_shell_is("zsh") {
        if let Some(path) = env_path("HISTFILE") {
            return Some(path);
        }
    }
    let base = env_path("ZDOTDIR").or_else(dirs::home_dir)?;
    Some(base.join(".zsh_history"))
}

fn bash_history_path() -> Option<PathBuf> {
    if let Some(path) = env_path("DEVSQL_BASH_HISTORY") {
        return Some(path);
    }
    if active_shell_is("bash") {
        if let Some(path) = env_path("HISTFILE") {
            return Some(path);
        }
    }
    dirs::home_dir().map(|home| home.join(".bash_history"))
}

fn active_shell_is(name: &str) -> bool {
    std::env::var_os("SHELL")
        .and_then(|shell| PathBuf::from(shell).file_name().map(|part| part.to_owned()))
        .is_some_and(|shell| shell == name)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| expand_home(&value))
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
