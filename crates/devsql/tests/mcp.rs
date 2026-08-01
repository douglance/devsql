//! End-to-end MCP stdio coverage for DevSQL's primary Code Mode interface.

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
fn exposes_code_mode_and_executes_a_devsql_query() {
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
            "codemode_cancel",
            "codemode_decide",
            "codemode_execute",
            "codemode_execution",
            "codemode_search",
        ]
    );

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "codemode_search", "arguments": {"query": "query"}}
        }),
    );
    let searched = response(&receiver, 3);
    assert!(searched.get("error").is_none(), "{searched}");
    assert!(searched.to_string().contains("devsql.query"), "{searched}");

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "codemode_execute",
                "arguments": {"code": "devsql.query({ query: 'SELECT 1 AS value' })"}
            }
        }),
    );
    let started = response(&receiver, 4);
    assert!(started.get("error").is_none(), "{started}");
    let execution_id = started["result"]["structuredContent"]["id"]
        .as_str()
        .expect("execution id")
        .to_string();
    let mut executed = started;
    for id in 5..105 {
        if executed["result"]["structuredContent"]["status"] == "completed" {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        send(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "codemode_execution",
                    "arguments": {"id": execution_id}
                }
            }),
        );
        executed = response(&receiver, id);
    }
    assert_eq!(
        executed["result"]["structuredContent"]["status"], "completed",
        "{executed}"
    );
    assert!(
        executed.to_string().contains("value") && executed.to_string().contains('1'),
        "{executed}"
    );

    drop(stdin);
    let status = child.wait().expect("wait for MCP server");
    reader.join().expect("join MCP reader");
    assert!(status.success(), "MCP server exited with {status}");
}
