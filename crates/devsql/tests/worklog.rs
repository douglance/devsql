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

fn work(home: &TempDir, dir: &std::path::Path, args: &[&str]) -> assert_cmd::assert::Assert {
    devsql()
        .env("DEVSQL_HOME", home.path())
        .current_dir(dir)
        .args(args)
        .args(["--format", "json"])
        .assert()
}

fn json(assert: assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).expect("json")
}

#[test]
fn start_takes_title_flag_and_note_means_body() {
    let home = with_home();
    let dir = TempDir::new().expect("dir");
    let started = json(
        work(&home, dir.path(), &["work", "start", "--title", "Wire the gate", "--note", "first pass"])
            .success(),
    );
    assert_eq!(started["task"]["title"], "Wire the gate");
    assert_eq!(started["event"]["body"], "first pass");
}

#[test]
fn update_and_done_find_a_task_by_title_or_id_prefix() {
    let home = with_home();
    let dir = TempDir::new().expect("dir");
    let started = json(work(&home, dir.path(), &["work", "start", "Wire the gate"]).success());
    let id = started["task"]["id"].as_str().unwrap().to_string();

    let updated = json(
        work(&home, dir.path(), &["work", "update", "wire the GATE", "--note", "halfway"]).success(),
    );
    assert_eq!(updated["task"]["id"], id.as_str());
    assert_eq!(updated["event"]["body"], "halfway");

    let done = json(work(&home, dir.path(), &["work", "done", &id[..12]]).success());
    assert_eq!(done["task"]["id"], id.as_str());
    assert_eq!(done["task"]["status"], "done");
}

#[test]
fn a_progress_sentence_lands_on_the_only_open_task_here() {
    let home = with_home();
    let here = TempDir::new().expect("here");
    let elsewhere = TempDir::new().expect("elsewhere");
    let started = json(work(&home, here.path(), &["work", "start", "Port the parser"]).success());
    work(&home, elsewhere.path(), &["work", "start", "Unrelated work"]).success();

    let done = json(
        work(&home, here.path(), &["work", "done", "Parser ported; tests green", "--body", "PR open"])
            .success(),
    );
    assert_eq!(done["task"]["id"], started["task"]["id"]);
    assert_eq!(done["event"]["body"], "Parser ported; tests green\n\nPR open");
}

#[test]
fn an_ambiguous_reference_lists_the_open_tasks() {
    let home = with_home();
    let dir = TempDir::new().expect("dir");
    let first = json(work(&home, dir.path(), &["work", "start", "First"]).success());
    let second = json(work(&home, dir.path(), &["work", "start", "Second"]).success());

    let output = work(&home, dir.path(), &["work", "update", "some progress"])
        .failure()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains(first["task"]["id"].as_str().unwrap()), "{text}");
    assert!(text.contains(second["task"]["id"].as_str().unwrap()), "{text}");
}

#[test]
fn a_wrong_id_is_never_read_as_progress() {
    let home = with_home();
    let dir = TempDir::new().expect("dir");
    work(&home, dir.path(), &["work", "start", "Only task"]).success();
    work(&home, dir.path(), &["work", "done", "00065c16489a29090000"])
        .failure()
        .stdout(predicate::str::contains("Unknown task: 00065c16489a29090000"));
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
