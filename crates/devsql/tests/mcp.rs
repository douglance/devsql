//! End-to-end MCP stdio coverage for direct DevSQL tool discovery.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

fn send(stdin: &mut impl Write, message: Value) {
    writeln!(stdin, "{message}").expect("write MCP message");
    stdin.flush().expect("flush MCP message");
}

fn response(receiver: &Receiver<Value>, id: i64) -> Value {
    loop {
        let message = receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("MCP response before timeout");
        if message.get("id").and_then(Value::as_i64) == Some(id) {
            return message;
        }
    }
}

#[test]
fn discovers_all_direct_tools_and_calls_query() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start DevSQL MCP server");
    let mut stdin = child.stdin.take().expect("MCP stdin");
    let stdout = child.stdout.take().expect("MCP stdout");
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.expect("read MCP line");
            if let Ok(value) = serde_json::from_str(&line) {
                sender.send(value).expect("forward MCP message");
            }
        }
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "devsql-test", "version": "1.0.0"}
            }
        }),
    );
    let initialized = response(&receiver, 1);
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    let listed = response(&receiver, 2);
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let mut names = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "context",
            "day",
            "days",
            "diff",
            "gather",
            "history",
            "impact",
            "query",
            "recall",
            "search",
            "today",
            "work_done",
            "work_list",
            "work_note",
            "work_start",
            "work_update",
        ]
    );
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        let writes_worklog = name.starts_with("work_") && name != "work_list";
        assert_eq!(
            tool["annotations"]["readOnlyHint"], !writes_worklog,
            "{tool}"
        );
        assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");
        assert_eq!(
            tool["annotations"]["idempotentHint"], !writes_worklog,
            "{tool}"
        );
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{tool}");
        assert!(tool.get("outputSchema").is_some(), "{tool}");
    }

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "query", "arguments": {"query": "SELECT 1 AS value"}}
        }),
    );
    let called = response(&receiver, 3);
    assert!(called.get("error").is_none(), "{called}");
    assert!(
        called.to_string().contains("value") && called.to_string().contains('1'),
        "{called}"
    );

    drop(stdin);
    let status = child.wait().expect("wait for MCP server");
    reader.join().expect("join MCP reader");
    assert!(status.success(), "MCP server exited with {status}");
}
