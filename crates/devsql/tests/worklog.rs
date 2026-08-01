//! CLI integration tests for worklog (work / today / day / days).

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn devsql() -> Command {
    Command::new(env!("CARGO_BIN_EXE_devsql"))
}

fn with_home() -> TempDir {
    TempDir::new().expect("temp home")
}

#[test]
fn work_start_today_round_trip() {
    let home = with_home();

    let start = devsql()
        .env("DEVSQL_HOME", home.path())
        .args([
            "work",
            "start",
            "Fix the dashboard",
            "--project",
            "factorylog",
            "--agent",
            "codex",
            "--body",
            "Started layout work",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&start).expect("json");
    let task_id = parsed["task"]["id"].as_str().expect("task id");
    assert_eq!(parsed["task"]["status"], "doing");
    assert_eq!(parsed["event"]["kind"], "start");

    devsql()
        .env("DEVSQL_HOME", home.path())
        .args([
            "work",
            "done",
            task_id,
            "--body",
            "Shipped layout",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"done\""));

    let today = devsql()
        .env("DEVSQL_HOME", home.path())
        .args(["today", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let day: Value = serde_json::from_slice(&today).expect("json");
    assert_eq!(day["stats"]["updates"], 2);
    assert_eq!(day["stats"]["tasks"], 1);
    assert_eq!(day["stats"]["done"], 1);
    assert!(day["markdown"]
        .as_str()
        .unwrap_or("")
        .contains("Fix the dashboard"));
}

#[test]
fn cross_project_filter() {
    let home = with_home();

    for (title, project) in [("Alpha", "proj-a"), ("Beta", "proj-b")] {
        devsql()
            .env("DEVSQL_HOME", home.path())
            .args([
                "work",
                "start",
                title,
                "--project",
                project,
                "--agent",
                "claude",
                "--format",
                "json",
            ])
            .assert()
            .success();
    }

    let filtered = devsql()
        .env("DEVSQL_HOME", home.path())
        .args(["today", "--project", "proj-a", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let day: Value = serde_json::from_slice(&filtered).expect("json");
    assert_eq!(day["stats"]["updates"], 1);
    let events = day["events"].as_array().unwrap();
    assert_eq!(events[0]["title"], "Alpha");
}

#[test]
fn work_list_and_sql_tables() {
    let home = with_home();

    devsql()
        .env("DEVSQL_HOME", home.path())
        .args([
            "work",
            "start",
            "SQL check",
            "--project",
            "devsql",
            "--format",
            "json",
        ])
        .assert()
        .success();

    let list = devsql()
        .env("DEVSQL_HOME", home.path())
        .args(["work", "list", "--status", "doing", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&list).expect("json");
    assert_eq!(parsed["total"], 1);

    // Query via SQL tables loaded from durable worklog
    devsql()
        .env("DEVSQL_HOME", home.path())
        .args([
            "query",
            "SELECT title FROM work_events ORDER BY ts DESC LIMIT 1",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("SQL check"));
}

#[test]
fn days_index() {
    let home = with_home();

    devsql()
        .env("DEVSQL_HOME", home.path())
        .args(["work", "note", "Quick note", "--project", "x", "--format", "json"])
        .assert()
        .success();

    let days = devsql()
        .env("DEVSQL_HOME", home.path())
        .args(["days", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&days).expect("json");
    assert!(parsed["total"].as_u64().unwrap() >= 1);
}
