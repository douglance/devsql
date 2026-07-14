use serde_json::Value;

/// One tool call extracted from an assistant message's `content[]` array.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRow {
    pub tool_name: String,
    pub input_json: String,
    pub target: Option<String>,
    pub timestamp: Option<String>,
}

/// Extract tool-call rows from a single transcript JSONL record.
///
/// Only `type == "assistant"` records with `message.content[]` `tool_use`
/// blocks yield rows; every other shape (user turns, non-tool content
/// blocks, malformed records) yields an empty vec rather than an error.
pub fn extract_tool_calls(json: &Value) -> Vec<ToolCallRow> {
    if json.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return Vec::new();
    }
    let Some(content) = json
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return Vec::new();
    };

    let timestamp = json
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(String::from);

    content
        .iter()
        .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter_map(|block| {
            let tool_name = block.get("name").and_then(|n| n.as_str())?.to_string();
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let input_json = serde_json::to_string(&input).unwrap_or_default();
            let target = extract_target(&tool_name, &input);
            Some(ToolCallRow {
                tool_name,
                input_json,
                target,
                timestamp: timestamp.clone(),
            })
        })
        .collect()
}

/// Derive the `target` column for a tool call: the first line of the
/// command for `Bash`, the subagent type for `Agent`, and `file_path` for
/// file-oriented tools (`Read`, `Write`, `Edit`, `NotebookEdit`, ...).
fn extract_target(tool_name: &str, input: &Value) -> Option<String> {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|c| c.as_str())
            .map(|cmd| cmd.lines().next().unwrap_or("").to_string()),
        "Agent" => input
            .get("subagent_type")
            .and_then(|s| s.as_str())
            .map(String::from),
        _ => input
            .get("file_path")
            .and_then(|f| f.as_str())
            .map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bash_read_and_agent_targets() {
        let assistant = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-06-01T10:00:05.000Z",
            "message": {
                "content": [
                    {"type": "text", "text": "let me check that"},
                    {"type": "tool_use", "name": "Bash", "input": {"command": "ls -la\necho done", "description": "list"}},
                    {"type": "tool_use", "name": "Read", "input": {"file_path": "/tmp/foo.rs"}},
                    {"type": "tool_use", "name": "Agent", "input": {"subagent_type": "sdlc:engineer", "prompt": "do it"}}
                ]
            }
        });

        let rows = extract_tool_calls(&assistant);
        assert_eq!(rows.len(), 3, "only tool_use blocks yield rows, not text");

        assert_eq!(rows[0].tool_name, "Bash");
        assert_eq!(rows[0].target.as_deref(), Some("ls -la"));
        assert_eq!(rows[0].timestamp.as_deref(), Some("2026-06-01T10:00:05.000Z"));

        assert_eq!(rows[1].tool_name, "Read");
        assert_eq!(rows[1].target.as_deref(), Some("/tmp/foo.rs"));

        assert_eq!(rows[2].tool_name, "Agent");
        assert_eq!(rows[2].target.as_deref(), Some("sdlc:engineer"));
        assert_eq!(
            rows[2].input_json,
            serde_json::json!({"subagent_type": "sdlc:engineer", "prompt": "do it"}).to_string()
        );
    }

    #[test]
    fn non_assistant_and_non_tool_records_yield_no_rows() {
        let user = serde_json::json!({"type": "user", "message": {"content": "hi"}});
        assert!(extract_tool_calls(&user).is_empty());

        let text_only_assistant = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "no tools here"}]}
        });
        assert!(extract_tool_calls(&text_only_assistant).is_empty());

        let malformed = serde_json::json!({"type": "assistant"});
        assert!(extract_tool_calls(&malformed).is_empty());
    }

    /// Fixture-based end-to-end test: parses a two-line jsonl fixture (one
    /// assistant message with Bash + Read tool_use + a non-tool text block,
    /// one assistant message with an Agent call) exactly the way the
    /// devsql `tool_calls` table loader does, and asserts exact row counts
    /// and column values.
    #[test]
    fn fixture_jsonl_yields_exact_rows_and_columns() {
        let fixture = concat!(
            r#"{"type":"assistant","timestamp":"2026-06-01T10:00:05.000Z","message":{"content":[{"type":"text","text":"checking"},{"type":"tool_use","name":"Bash","input":{"command":"cargo test -p ccql\necho done"}},{"type":"tool_use","name":"Read","input":{"file_path":"/repo/src/lib.rs"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-06-01T10:00:10.000Z","message":{"content":[{"type":"tool_use","name":"Agent","input":{"subagent_type":"sdlc:tester"}}]}}"#,
        );

        let rows: Vec<ToolCallRow> = fixture
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .flat_map(|json| extract_tool_calls(&json))
            .collect();

        assert_eq!(rows.len(), 3, "2 tool_use blocks in msg 1 + 1 in msg 2");

        assert_eq!(rows[0].tool_name, "Bash");
        assert_eq!(rows[0].target.as_deref(), Some("cargo test -p ccql"));

        assert_eq!(rows[1].tool_name, "Read");
        assert_eq!(rows[1].target.as_deref(), Some("/repo/src/lib.rs"));

        assert_eq!(rows[2].tool_name, "Agent");
        assert_eq!(rows[2].target.as_deref(), Some("sdlc:tester"));
        assert_eq!(rows[2].timestamp.as_deref(), Some("2026-06-01T10:00:10.000Z"));
    }
}
