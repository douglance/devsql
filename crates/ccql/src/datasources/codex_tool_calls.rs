use serde_json::Value;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Discover Codex rollout session files under `<codex_sessions_dir>/**/*.jsonl`.
///
/// Mirrors `discover_transcript_files`'s walk-and-extension-filter pattern.
/// Compressed archives (`*.jsonl.zst`) are skipped naturally: their
/// extension resolves to `zst`, not `jsonl`.
pub fn discover_codex_session_files(sessions_dir: &Path) -> Vec<PathBuf> {
    if !sessions_dir.exists() {
        return Vec::new();
    }

    WalkDir::new(sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

/// Session-level metadata parsed from a `type == "session_meta"` record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexSessionMeta {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub parent_session_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_role: Option<String>,
    pub originator: Option<String>,
}

/// Extract session metadata from one JSONL record, if it is a
/// `session_meta` line. Returns `None` for any other shape.
pub fn extract_session_meta(json: &Value) -> Option<CodexSessionMeta> {
    if json.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = json.get("payload")?;
    Some(CodexSessionMeta {
        session_id: payload
            .get("session_id")
            .or_else(|| payload.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        cwd: payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from),
        parent_session_id: payload
            .get("parent_thread_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        agent_id: payload
            .get("agent_path")
            .and_then(|v| v.as_str())
            .map(String::from),
        agent_role: payload
            .get("agent_role")
            .and_then(|v| v.as_str())
            .map(String::from),
        originator: payload
            .get("originator")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// One Codex tool call extracted from a `payload.type == "function_call"` record.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexToolCallRow {
    pub tool_name: String,
    pub arguments_json: String,
    pub cmd: Option<String>,
    pub source_id: Option<String>,
    pub cwd: Option<String>,
    pub timestamp: Option<String>,
}

/// Extract a Codex tool call from one JSONL record. Returns `None` for
/// non-function-call records or malformed shapes (never errors).
pub fn extract_codex_tool_call(json: &Value) -> Option<CodexToolCallRow> {
    let payload = json.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("function_call") {
        return None;
    }
    let tool_name = payload.get("name").and_then(|n| n.as_str())?.to_string();

    // Codex rollout files encode `arguments` as a JSON-text string.
    // Fall back to treating it as an inline JSON value for robustness.
    let arguments_raw = payload.get("arguments");
    let arguments_str = arguments_raw.and_then(|a| a.as_str());
    let arguments_value: Value = arguments_str
        .and_then(|s| serde_json::from_str(s).ok())
        .or_else(|| arguments_raw.cloned())
        .unwrap_or(Value::Null);
    let arguments_json = match arguments_str {
        Some(s) => s.to_string(),
        None => serde_json::to_string(&arguments_value).unwrap_or_default(),
    };

    let cmd = extract_cmd(&tool_name, &arguments_value);
    let source_id = payload
        .get("call_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let cwd = arguments_value
        .get("workdir")
        .and_then(|v| v.as_str())
        .map(String::from);
    let timestamp = json
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(CodexToolCallRow {
        tool_name,
        arguments_json,
        cmd,
        source_id,
        cwd,
        timestamp,
    })
}

/// Extract the shell command string for `exec_command`/`shell` function
/// calls; other tool names have no `cmd` value.
fn extract_cmd(tool_name: &str, arguments: &Value) -> Option<String> {
    match tool_name {
        "exec_command" => arguments
            .get("cmd")
            .and_then(|c| c.as_str())
            .map(String::from),
        "shell" => match arguments.get("command") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(items)) => {
                let parts: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(" "))
                }
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    #[test]
    fn discover_finds_nested_jsonl_and_skips_zst_archives() {
        let temp = tempfile::tempdir().expect("temp");
        let sessions_dir = temp.path().join("sessions");

        write(
            &sessions_dir
                .join("2026")
                .join("07")
                .join("11")
                .join("rollout-a.jsonl"),
            "{}",
        );
        write(&sessions_dir.join("rollout-legacy.jsonl"), "{}");
        write(
            &sessions_dir
                .join("2025")
                .join("11")
                .join("07")
                .join("rollout-compressed.jsonl.zst"),
            "binary",
        );

        let files = discover_codex_session_files(&sessions_dir);
        assert_eq!(files.len(), 2, "the .jsonl.zst archive is excluded");
        assert!(files.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[test]
    fn discover_returns_empty_for_missing_dir() {
        let temp = tempfile::tempdir().expect("temp");
        let missing = temp.path().join("no-sessions-here");
        assert!(discover_codex_session_files(&missing).is_empty());
    }

    #[test]
    fn extracts_session_meta() {
        let line = serde_json::json!({
            "timestamp": "2026-07-11T04:00:37.144Z",
            "type": "session_meta",
            "payload": {
                "session_id": "019f4f55-8ff1-7bb2-9870-a9321fa7ff32",
                "cwd": "/Users/doug/Developer/app",
                "parent_thread_id": "parent-session",
                "agent_path": "/root/worker",
                "agent_role": "engineer",
                "originator": "codex-tui"
            }
        });

        let meta = extract_session_meta(&line).expect("session_meta");
        assert_eq!(
            meta.session_id.as_deref(),
            Some("019f4f55-8ff1-7bb2-9870-a9321fa7ff32")
        );
        assert_eq!(meta.cwd.as_deref(), Some("/Users/doug/Developer/app"));
        assert_eq!(meta.parent_session_id.as_deref(), Some("parent-session"));
        assert_eq!(meta.agent_id.as_deref(), Some("/root/worker"));
        assert_eq!(meta.agent_role.as_deref(), Some("engineer"));
        assert_eq!(meta.originator.as_deref(), Some("codex-tui"));

        let non_meta = serde_json::json!({"type": "response_item"});
        assert!(extract_session_meta(&non_meta).is_none());
    }

    #[test]
    fn extracts_exec_command_call_with_string_encoded_arguments() {
        let line = serde_json::json!({
            "timestamp": "2026-07-11T04:19:22.459Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "exec_command",
                "arguments": "{\"cmd\":\"echo hi\",\"workdir\":\"/tmp\"}",
                "call_id": "call_abc"
            }
        });

        let row = extract_codex_tool_call(&line).expect("function_call");
        assert_eq!(row.tool_name, "exec_command");
        assert_eq!(row.cmd.as_deref(), Some("echo hi"));
        assert_eq!(row.source_id.as_deref(), Some("call_abc"));
        assert_eq!(row.cwd.as_deref(), Some("/tmp"));
        assert_eq!(row.timestamp.as_deref(), Some("2026-07-11T04:19:22.459Z"));
        assert_eq!(
            row.arguments_json,
            "{\"cmd\":\"echo hi\",\"workdir\":\"/tmp\"}"
        );
    }

    #[test]
    fn extracts_shell_call_with_array_command() {
        let line = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell",
                "arguments": "{\"command\":[\"bash\",\"-lc\",\"echo hi\"]}"
            }
        });

        let row = extract_codex_tool_call(&line).expect("function_call");
        assert_eq!(row.tool_name, "shell");
        assert_eq!(row.cmd.as_deref(), Some("bash -lc echo hi"));
    }

    #[test]
    fn non_function_call_and_malformed_records_yield_none() {
        let session_meta = serde_json::json!({"type": "session_meta", "payload": {}});
        assert!(extract_codex_tool_call(&session_meta).is_none());

        let other_payload = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "content": "hi"}
        });
        assert!(extract_codex_tool_call(&other_payload).is_none());

        let malformed = serde_json::json!({"type": "response_item"});
        assert!(extract_codex_tool_call(&malformed).is_none());
    }

    /// Fixture-based end-to-end test mirroring the devsql `codex_tool_calls`
    /// table loader: walk discovered session files, track session_meta as
    /// encountered, and emit one row per function_call line.
    #[test]
    fn fixture_session_file_yields_exact_rows_and_columns() {
        let temp = tempfile::tempdir().expect("temp");
        let sessions_dir = temp.path().join("sessions");
        let file_path = sessions_dir.join("2026").join("07").join("rollout-x.jsonl");

        let fixture = concat!(
            r#"{"timestamp":"2026-07-11T04:00:37.144Z","type":"session_meta","payload":{"session_id":"sess-abc","cwd":"/repo"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-11T04:00:58.866Z","type":"response_item","payload":{"type":"function_call","name":"wait","arguments":"{\"cell_id\":\"1\"}"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-11T04:19:22.459Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo build\",\"workdir\":\"/repo\"}"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-11T04:20:00.000Z","type":"response_item","payload":{"type":"message","content":"not a tool call"}}"#,
        );
        write(&file_path, fixture);

        let files = discover_codex_session_files(&sessions_dir);
        assert_eq!(files.len(), 1);

        let content = std::fs::read_to_string(&files[0]).expect("read fixture");
        let mut session_id = None;
        let mut cwd = None;
        let mut rows = Vec::new();
        for line in content.lines() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(meta) = extract_session_meta(&entry) {
                session_id = meta.session_id.or(session_id);
                cwd = meta.cwd.or(cwd);
                continue;
            }
            if let Some(call) = extract_codex_tool_call(&entry) {
                rows.push((call, session_id.clone(), cwd.clone()));
            }
        }

        assert_eq!(
            rows.len(),
            2,
            "2 function_call lines; the message line yields none"
        );

        let (wait_call, wait_session, wait_cwd) = &rows[0];
        assert_eq!(wait_call.tool_name, "wait");
        assert_eq!(wait_call.cmd, None);
        assert_eq!(wait_session.as_deref(), Some("sess-abc"));
        assert_eq!(wait_cwd.as_deref(), Some("/repo"));

        let (exec_call, exec_session, exec_cwd) = &rows[1];
        assert_eq!(exec_call.tool_name, "exec_command");
        assert_eq!(exec_call.cmd.as_deref(), Some("cargo build"));
        assert_eq!(exec_session.as_deref(), Some("sess-abc"));
        assert_eq!(exec_cwd.as_deref(), Some("/repo"));
    }
}
