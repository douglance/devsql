//! CLI integration tests for `devsql recall`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn write(path: &std::path::Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

/// Build a temporary `~/.claude`-shaped data dir with two sessions and two
/// history prompts. One prompt matches BOTH query terms; the other matches one.
fn create_claude_data_dir() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let root = temp.path();
    let slug = "-Users-test-Developer-app";

    // sessions: title comes from the `ai-title` record.
    write(
        &root.join("projects").join(slug).join("sess-both.jsonl"),
        &[
            r#"{"type":"user","content":"hi","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/test/Developer/app"}"#,
            r#"{"type":"ai-title","aiTitle":"vision simulator session","sessionId":"sess-both"}"#,
        ]
        .join("\n"),
    );
    write(
        &root.join("projects").join(slug).join("sess-one.jsonl"),
        &[
            r#"{"type":"user","content":"hi","timestamp":"2026-05-01T10:00:00.000Z","cwd":"/Users/test/Developer/app"}"#,
            r#"{"type":"ai-title","aiTitle":"vision notes only","sessionId":"sess-one"}"#,
        ]
        .join("\n"),
    );

    // history: display strings; numeric epoch-ms timestamps.
    write(
        &root.join("history.jsonl"),
        &[
            r#"{"display":"apple vision pro simulator boot sequence","timestamp":1754402102000,"project":"/Users/test/Developer/app"}"#,
            r#"{"display":"vision notes only, nothing else","timestamp":1754402100000,"project":"/Users/test/Developer/app"}"#,
        ]
        .join("\n"),
    );

    temp
}

fn create_codex_data_dir() -> TempDir {
    let temp = TempDir::new().expect("temp");
    write(
        &temp
            .path()
            .join("sessions/2026/07/27/rollout-codex-recall.jsonl"),
        concat!(
            "{\"timestamp\":\"2026-07-27T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-recall\",\"cwd\":\"/Users/test/Developer/app\",\"thread_source\":\"user\"}}\n",
            "{\"timestamp\":\"2026-07-27T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"debug auth callback Authorization: Bearer abc.def.ghi\"}]}}\n"
        ),
    );
    temp
}

#[test]
fn recall_surfaces_matching_fixture() {
    let data = create_claude_data_dir();
    let codex_home = TempDir::new().expect("codex");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "recall",
            "vision",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("vision simulator session"))
        .stdout(predicate::str::contains("apple vision pro simulator boot"));
}

#[test]
fn recall_ranks_more_matches_first() {
    let data = create_claude_data_dir();
    let codex_home = TempDir::new().expect("codex");

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "recall",
            "vision simulator",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid json");

    // The prompt hitting BOTH terms must sort first with score 2.
    let prompts = parsed["prompts"].as_array().expect("prompts array");
    assert_eq!(prompts.len(), 2, "both prompts match at least one term");
    assert_eq!(prompts[0]["score"], serde_json::json!(2));
    assert!(prompts[0]["prompt"]
        .as_str()
        .unwrap()
        .contains("simulator boot"));
    assert_eq!(prompts[1]["score"], serde_json::json!(1));

    // Sessions likewise: the two-term title outranks the one-term title.
    let sessions = parsed["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions[0]["score"], serde_json::json!(2));
    assert_eq!(
        sessions[0]["title"],
        serde_json::json!("vision simulator session")
    );
}

#[test]
fn recall_empty_terms_exit_zero() {
    let data = create_claude_data_dir();
    let codex_home = TempDir::new().expect("codex");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "recall",
            "a",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"total\": 0"));
}

#[test]
fn recall_returns_ranked_redacted_codex_threads() {
    let claude = TempDir::new().expect("claude");
    let codex_home = create_codex_data_dir();

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "recall",
            "auth callback",
            "--data-dir",
            claude.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid json");
    let threads = parsed["codex_threads"]
        .as_array()
        .expect("codex_threads array");
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["thread_id"], serde_json::json!("codex-recall"));
    assert_eq!(threads[0]["score"], serde_json::json!(2));
    let excerpt = threads[0]["excerpt"].as_str().expect("excerpt");
    assert!(excerpt.contains("debug auth callback"));
    assert!(excerpt.contains("Bearer <redacted>"));
    assert!(!excerpt.contains("abc.def.ghi"));
}

#[test]
fn root_and_named_query_are_compatible() {
    for args in [
        vec!["SELECT 1 AS value", "--format", "json"],
        vec!["query", "SELECT 1 AS value", "--format", "json"],
    ] {
        Command::new(env!("CARGO_BIN_EXE_devsql"))
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains("\"value\": 1"));
    }
}

#[test]
fn root_help_leads_agents_to_code_mode() {
    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("PRIMARY AGENT INTERFACE"))
        .stdout(predicate::str::contains("devsql --mcp"))
        .stdout(predicate::str::contains(
            "The direct CLI below is the human and scripting fallback.",
        ));
}

#[test]
fn table_and_csv_formats_remain_available() {
    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .args(["SELECT 1 AS value", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("value"));

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .args(["SELECT 1 AS value", "--format", "csv"])
        .assert()
        .success()
        .stdout(predicate::str::contains("value\n1"));
}
