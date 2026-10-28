//! Machine-labeled snapshots for shared history.

use crate::{Error, Result, UnifiedEngine};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

const TABLES: &[&str] = &[
    "history",
    "jhistory",
    "codex_history",
    "transcripts",
    "sessions",
    "todos",
    "tool_calls",
    "codex_threads",
    "codex_events",
    "codex_messages",
    "codex_tool_executions",
    "codex_compactions",
    "codex_ingest_errors",
    "codex_tool_calls",
    "shell_history",
    "command_events",
    "grok_bots",
    "grok_entries",
    "grok_messages",
    "grok_ingest_errors",
];

/// Export one machine's observed history to a new SQLite snapshot.
pub fn export(engine: &mut UnifiedEngine, machine: &str, path: &Path) -> Result<()> {
    if machine.is_empty()
        || machine.len() > 128
        || !machine
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err(Error::Query("invalid machine identity".into()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    drop(file);
    let result = (|| {
        engine.load_claude_tables(&[
            "history",
            "jhistory",
            "transcripts",
            "sessions",
            "todos",
            "tool_calls",
            "codex_threads",
        ])?;
        engine.load_command_events()?;
        engine.load_grok_tables(&["grok_bots"])?;
        let conn = engine.conn();
        conn.execute(
            "ATTACH DATABASE ?1 AS shared_export",
            [path.to_string_lossy().as_ref()],
        )?;
        let outcome = (|| {
            conn.execute_batch("BEGIN")?;
            for table in TABLES {
                let sql = format!("CREATE TABLE shared_export.{table} AS SELECT ?1 AS _machine, * FROM main.{table}");
                // Some sources are temporary views rather than main tables.
                let sql = sql.replace(&format!("main.{table}"), table);
                conn.execute(&sql, params![machine])?;
            }
            conn.pragma_update(
                Some(rusqlite::DatabaseName::Attached("shared_export")),
                "user_version",
                1,
            )?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = conn.execute_batch("ROLLBACK");
        }
        let detached = conn.execute_batch("DETACH DATABASE shared_export");
        outcome.and(detached.map_err(Error::from))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Query a set of complete snapshots together, preserving source machine.
pub fn query(paths: &[std::path::PathBuf], sql: &str) -> Result<Vec<Value>> {
    let first = sql.trim_start().split_whitespace().next().unwrap_or("");
    if !first.eq_ignore_ascii_case("select") && !first.eq_ignore_ascii_case("with") {
        return Err(Error::Query(
            "shared queries must start with SELECT or WITH".into(),
        ));
    }
    let conn = Connection::open("")?;
    conn.execute_batch(
        "CREATE TABLE shared_snapshots (_machine TEXT NOT NULL, modified_at INTEGER NOT NULL)",
    )?;
    let words: Vec<String> = sql
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let selected: Vec<&&str> = TABLES
        .iter()
        .filter(|table| words.iter().any(|word| word == **table))
        .collect();
    let mut schemas = std::collections::HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        let machine = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::Query("invalid snapshot filename".into()))?;
        let modified = path
            .metadata()?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Query("invalid snapshot timestamp".into()))?
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO shared_snapshots VALUES (?1, ?2)",
            params![machine, modified],
        )?;
        // Refuse to create missing databases and reject databases without our tables.
        let snapshot = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = snapshot.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != 1 {
            return Err(Error::Query("unsupported shared snapshot version".into()));
        }
        for table in TABLES {
            let exists: bool = snapshot.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(Error::Query(format!("invalid snapshot: missing {table}")));
            }
            let mut info = snapshot.prepare(&format!("PRAGMA table_info({table})"))?;
            let columns: Vec<String> = info
                .query_map([], |row| row.get(1))?
                .collect::<rusqlite::Result<_>>()?;
            if columns.first().map(String::as_str) != Some("_machine") {
                return Err(Error::Query(format!("invalid snapshot schema: {table}")));
            }
            if let Some(expected) = schemas.get(table) {
                if expected != &columns {
                    return Err(Error::Query(format!(
                        "incompatible snapshot schema: {table}"
                    )));
                }
            } else {
                schemas.insert(*table, columns);
            }
        }
        drop(snapshot);
        conn.execute(
            "ATTACH DATABASE ?1 AS incoming",
            [path.to_string_lossy().as_ref()],
        )?;
        for table in &selected {
            let statement = if i == 0 {
                format!("CREATE TABLE {table} AS SELECT * FROM incoming.{table}")
            } else {
                format!("INSERT INTO {table} SELECT * FROM incoming.{table}")
            };
            conn.execute_batch(&statement)?;
        }
        conn.execute_batch("DETACH DATABASE incoming")?;
    }
    if paths.is_empty() {
        return Err(Error::Query("no shared snapshots".into()));
    }
    let mut stmt = conn.prepare(sql)?;
    if !stmt.readonly() {
        return Err(Error::Query("shared queries must be read-only".into()));
    }
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let rows = stmt.query_map([], |row| {
        let mut obj = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let value = match row.get_ref(i)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(v) => Value::from(v),
                rusqlite::types::ValueRef::Real(v) => Value::from(v),
                rusqlite::types::ValueRef::Text(v) => {
                    Value::String(String::from_utf8_lossy(v).into_owned())
                }
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            obj.insert(name.clone(), value);
        }
        Ok(Value::Object(obj))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn snapshots_query_two_machines_and_reject_writes() {
        let dir = TempDir::new().unwrap();
        let mut paths = Vec::new();
        for (machine, message) in [("alpha", "first prompt"), ("beta", "second prompt")] {
            let claude = dir.path().join(machine).join("claude");
            let codex = dir.path().join(machine).join("codex");
            std::fs::create_dir_all(&claude).unwrap();
            std::fs::create_dir_all(&codex).unwrap();
            std::fs::write(
                claude.join("history.jsonl"),
                format!("{{\"display\":\"{message}\",\"timestamp\":1}}\n"),
            )
            .unwrap();
            let mut engine =
                UnifiedEngine::new_with_codex_data_dir(claude, dir.path().into(), codex).unwrap();
            let path = dir.path().join(format!("{machine}.sqlite"));
            export(&mut engine, machine, &path).unwrap();
            paths.push(path);
        }
        let rows = query(
            &paths,
            "SELECT _machine, display FROM history ORDER BY _machine",
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                serde_json::json!({"_machine":"alpha","display":"first prompt"}),
                serde_json::json!({"_machine":"beta","display":"second prompt"})
            ]
        );
        assert!(query(&paths, "DELETE FROM history").is_err());
        assert!(query(&paths, "WITH x AS (SELECT 1) DELETE FROM history").is_err());
        let db = Connection::open(&paths[1]).unwrap();
        db.execute_batch("ALTER TABLE history RENAME COLUMN display TO changed_display")
            .unwrap();
        drop(db);
        assert!(query(&paths, "SELECT * FROM history")
            .unwrap_err()
            .to_string()
            .contains("incompatible snapshot schema"));
    }

    #[test]
    fn resolves_only_tailnet_hosts() {
        let status = serde_json::json!({"BackendState":"Running", "Self":{"ID":"node1", "HostName":"self", "TailscaleIPs":["100.64.0.1"]}, "Peer":{"one":{"HostName":"host", "DNSName":"host.example.ts.net.", "TailscaleIPs":["100.64.0.2"]}}});
        assert_eq!(
            tailscale_host("host", &status).unwrap(),
            ("100.64.0.2".into(), "node1".into())
        );
        assert!(tailscale_host("unknown", &status).is_err());
        assert!(tailscale_host("host;echo", &status).is_err());
        let mut stopped = status.clone();
        stopped["BackendState"] = "Stopped".into();
        assert!(tailscale_host("host", &stopped).is_err());
    }
}

/// Resolve a configured host against the active Tailscale peer list.
pub fn tailscale_host(host: &str, status: &Value) -> Result<(String, String)> {
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(Error::Query("invalid Tailscale host".into()));
    }
    if status.get("BackendState").and_then(Value::as_str) != Some("Running") {
        return Err(Error::Query("Tailscale is not running".into()));
    }
    let peers = status
        .get("Peer")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Query("Tailscale peer list unavailable".into()))?;
    let mut matches = peers.values().chain(status.get("Self")).filter(|p| {
        p.get("HostName")
            .and_then(Value::as_str)
            .is_some_and(|v| v.eq_ignore_ascii_case(host))
            || p.get("DNSName").and_then(Value::as_str).is_some_and(|v| {
                v.trim_end_matches('.')
                    .eq_ignore_ascii_case(host.trim_end_matches('.'))
            })
    });
    let peer = matches
        .next()
        .ok_or_else(|| Error::Query("host is not in this Tailscale network".into()))?;
    if matches.next().is_some() {
        return Err(Error::Query(
            "Tailscale host name is ambiguous; use its full DNS name".into(),
        ));
    }
    let addresses = peer
        .get("TailscaleIPs")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Query("host has no Tailscale address".into()))?;
    let address = addresses
        .iter()
        .filter_map(Value::as_str)
        .find(|s| {
            s.parse::<std::net::Ipv4Addr>().is_ok_and(|a| {
                let b = a.octets();
                b[0] == 100 && (64..=127).contains(&b[1])
            })
        })
        .ok_or_else(|| Error::Query("host has no Tailscale IPv4 address".into()))?;
    let machine = status
        .pointer("/Self/ID")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(|| Error::Query("Tailscale machine identity unavailable".into()))?;
    Ok((address.to_owned(), machine.to_owned()))
}

pub fn tailscale_status() -> Result<Value> {
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()?;
    if !output.status.success() {
        return Err(Error::Query("tailscale status failed".into()));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn ssh(host: &str, address: &str) -> std::process::Command {
    let mut command = std::process::Command::new("ssh");
    command.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
        "-o",
        "ProxyCommand=none",
        "-o",
        "ProxyJump=none",
        "-o",
        &format!("HostKeyAlias={host}"),
        address,
    ]);
    command
}

/// Publish one complete snapshot by rsync over SSH, then atomically rename it.
pub fn publish(host: &str, snapshot: &Path) -> Result<String> {
    let status = tailscale_status()?;
    let (address, machine) = tailscale_host(host, &status)?;
    let staging = format!("{}.sqlite.tmp-{}", machine, std::process::id());
    let target = format!("{machine}.sqlite");
    let mkdir = "umask 077; mkdir -p \"$HOME/.devsql/shared/machines\" && chmod 700 \"$HOME/.devsql/shared/machines\"";
    if !ssh(host, &address).arg(mkdir).status()?.success() {
        return Err(Error::Query(
            "SSH could not prepare shared host directory".into(),
        ));
    }
    let transport = format!("ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=2 -o ProxyCommand=none -o ProxyJump=none -o HostKeyAlias={host}");
    let remote = format!("{address}:.devsql/shared/machines/{staging}");
    let transfer = std::process::Command::new("rsync")
        .args(["-a", "-e", &transport])
        .arg(snapshot)
        .arg(remote)
        .status()?;
    if !transfer.success() {
        return Err(Error::Query(
            "snapshot transfer failed; previous snapshot remains available".into(),
        ));
    }
    let rename = format!("umask 077; mv -- \"$HOME/.devsql/shared/machines/{staging}\" \"$HOME/.devsql/shared/machines/{target}\"");
    if !ssh(host, &address).arg(rename).status()?.success() {
        return Err(Error::Query(
            "snapshot publication failed; previous snapshot remains available".into(),
        ));
    }
    Ok(machine)
}

/// Ask the shared host to execute SQL using SSH and send SQL through stdin.
pub fn remote_query(host: &str, sql: &str) -> Result<Value> {
    use std::io::Write;
    use std::process::Stdio;
    let (address, _) = tailscale_host(host, &tailscale_status()?)?;
    let mut child = ssh(host, &address)
        .arg("PATH=\"$HOME/.cargo/bin:$HOME/.local/bin:$PATH\"; export PATH; devsql shared query \"$(cat)\" --dir \"$HOME/.devsql/shared/machines\" --format json")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::Query("SSH stdin unavailable".into()))?
        .write_all(sql.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(Error::Query(format!(
            "shared host query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
