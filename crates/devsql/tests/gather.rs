//! CLI integration tests for `devsql gather`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

const TERM: &str = "gatherterm";

/// `activity` loads `codex_tool_calls`, which (like every other devsql
/// table loader) falls back to the real `~/.codex` when `CODEX_HOME` is
/// unset. Point it at an empty directory so tests are hermetic and fast
/// instead of scanning the host machine's real Codex session history.
fn isolated_codex_home() -> TempDir {
    TempDir::new().expect("temp")
}

fn populated_codex_home() -> TempDir {
    let temp = TempDir::new().expect("temp");
    write(
        &temp
            .path()
            .join("sessions/2026/07/27/rollout-gather-codex.jsonl"),
        &format!(
            concat!(
                "{{\"timestamp\":\"2026-07-27T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"gather-codex\",\"cwd\":\"/repo\",\"thread_source\":\"user\"}}}}\n",
                "{{\"timestamp\":\"2026-07-27T10:00:01Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"investigate {term} password=hunter2\"}}]}}}}\n",
                "{{\"timestamp\":\"2026-07-27T10:00:02Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call\",\"name\":\"exec_command\",\"call_id\":\"call-gather\",\"arguments\":\"{{\\\"cmd\\\":\\\"echo {term} Authorization: Bearer abc.def.ghi\\\"}}\"}}}}\n"
            ),
            term = TERM
        ),
    );
    temp
}

fn write(path: &std::path::Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

/// Build a `~/.claude`-shaped data dir with a session, a history prompt, a
/// todo, and a transcript tool call -- all mentioning `TERM` -- so
/// prior_work and activity have real matches.
fn create_claude_data_dir() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let root = temp.path();
    let slug = "-Users-test-Developer-app";

    write(
        &root.join("projects").join(slug).join("sess1.jsonl"),
        &[
            r#"{"type":"user","content":"hi","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/test/Developer/app"}"#.to_string(),
            format!(
                r#"{{"type":"assistant","timestamp":"2026-06-01T10:00:05.000Z","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"echo {TERM} test"}}}}]}}}}"#
            ),
            format!(r#"{{"type":"ai-title","aiTitle":"{TERM} session title","sessionId":"sess1"}}"#),
        ]
        .join("\n"),
    );

    write(
        &root.join("history.jsonl"),
        &format!(
            r#"{{"display":"working on {TERM} feature","timestamp":1754402102000,"project":"/Users/test/Developer/app"}}"#
        ),
    );

    write(
        &root.join("todos").join("todo1.json"),
        &format!(r#"[{{"content":"finish {TERM} docs","status":"pending"}}]"#),
    );

    temp
}

/// Build a git repo with one commit and one source file mentioning `TERM`,
/// so repo_state, code_search, symbols, and excerpts have real matches.
fn create_git_repo() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let repo = git2::Repository::init(temp.path()).expect("git init");

    write(
        &temp.path().join("lib.rs"),
        &format!(
            "// fixture for {TERM}\nfn find_{TERM}() {{\n    println!(\"{TERM} handler\");\n}}\n"
        ),
    );

    let mut index = repo.index().expect("index");
    index.add_path(std::path::Path::new("lib.rs")).expect("add");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    let sig = git2::Signature::now("Test", "test@example.com").expect("sig");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &format!("feat: add {TERM} handler"),
        &tree,
        &[],
    )
    .expect("commit");

    temp
}

/// A valid, empty git repo with no matching content (used to isolate a
/// single failing section without also affecting the others).
fn create_empty_git_repo() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let repo = git2::Repository::init(temp.path()).expect("git init");
    let sig = git2::Signature::now("Test", "test@example.com").expect("sig");
    let mut index = repo.index().expect("index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .expect("commit");
    temp
}

/// (a) All 6 sections are present in the JSON bundle.
#[test]
fn gather_returns_all_six_sections() {
    let claude = create_claude_data_dir();
    let repo = create_git_repo();
    let codex_home = isolated_codex_home();

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            TERM,
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            repo.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid json");
    for section in [
        "prior_work",
        "repo_state",
        "code_search",
        "symbols",
        "excerpts",
        "activity",
    ] {
        assert!(
            parsed.get(section).is_some(),
            "missing section `{section}` in gather output: {parsed}"
        );
        assert!(
            parsed[section].get("rows").is_some(),
            "section `{section}` missing `rows`"
        );
    }

    // Real matches should have surfaced -- proves the sections aren't just
    // present but actually populated from the fixtures.
    assert!(parsed["prior_work"]["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .any(|r| r["text"].as_str().unwrap_or("").contains(TERM)));
    assert!(parsed["code_search"]["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .any(|r| r["path"].as_str().unwrap_or("").contains("lib.rs")));
}

/// (b) A single failing section (repo_state, via a repo path with no `.git`)
/// does not take down the other 5 sections.
#[test]
fn gather_survives_one_failing_section() {
    let claude = create_claude_data_dir();
    let not_a_repo = TempDir::new().expect("temp");
    let codex_home = isolated_codex_home();

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            TERM,
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            not_a_repo.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid json");

    // repo_state failed and carries a note, with empty rows.
    assert!(parsed["repo_state"]["note"].is_string(), "{parsed}");
    assert_eq!(parsed["repo_state"]["rows"], serde_json::json!([]));

    // The other sections still ran and returned real data.
    assert!(parsed["prior_work"]["note"].is_null(), "{parsed}");
    assert!(parsed["prior_work"]["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .any(|r| r["text"].as_str().unwrap_or("").contains(TERM)));
    assert!(parsed["activity"]["note"].is_null(), "{parsed}");
    assert!(parsed["code_search"].get("rows").is_some());
    assert!(parsed["symbols"].get("rows").is_some());
    assert!(parsed["excerpts"].get("rows").is_some());
}

/// (c) A tight budget keeps the rendered bundle's real token count (as
/// measured by incurs' own `--token-count` flag) at or under the budget.
#[test]
fn gather_enforces_token_budget() {
    let claude = create_claude_data_dir();
    let repo = create_git_repo();
    let codex_home = isolated_codex_home();
    let budget = 90;

    // Sanity check: without a budget, this fixture produces a bundle well
    // over the tight budget below, so the assertion actually exercises
    // trimming rather than passing by construction.
    let unbounded = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            TERM,
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            repo.path().to_str().unwrap(),
            "--format",
            "json",
            "--budget",
            "1000000",
            "--token-count",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let unbounded_tokens: usize = String::from_utf8(unbounded)
        .expect("utf8")
        .trim()
        .parse()
        .expect("token count");
    assert!(
        unbounded_tokens > budget,
        "fixture must exceed the test budget unbounded to prove trimming happened, got {unbounded_tokens}"
    );

    let bounded = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            TERM,
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            repo.path().to_str().unwrap(),
            "--format",
            "json",
            "--budget",
            &budget.to_string(),
            "--token-count",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let bounded_tokens: usize = String::from_utf8(bounded)
        .expect("utf8")
        .trim()
        .parse()
        .expect("token count");

    assert!(
        bounded_tokens <= budget,
        "expected <= {budget} tokens, got {bounded_tokens}"
    );
}

/// `--format json` still round-trips terms even with no matches.
#[test]
fn gather_empty_terms_still_returns_sections() {
    let claude = create_claude_data_dir();
    let repo = create_empty_git_repo();
    let codex_home = isolated_codex_home();

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            "a",
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            repo.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"prior_work\""))
        .stdout(predicate::str::contains("\"repo_state\""));
}

#[test]
fn gather_includes_redacted_codex_threads_and_activity() {
    let claude = TempDir::new().expect("claude");
    let repo = create_empty_git_repo();
    let codex_home = populated_codex_home();

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("CODEX_HOME", codex_home.path())
        .args([
            "gather",
            TERM,
            "--data-dir",
            claude.path().to_str().unwrap(),
            "-r",
            repo.path().to_str().unwrap(),
            "--format",
            "json",
            "--budget",
            "100000",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).expect("valid json");
    let prior = parsed["prior_work"]["rows"].as_array().expect("prior rows");
    let thread = prior
        .iter()
        .find(|row| row["kind"] == "codex_thread")
        .expect("codex thread");
    assert!(thread["text"].as_str().unwrap().contains(TERM));
    assert!(thread["text"].as_str().unwrap().contains("<redacted>"));
    assert!(!thread["text"].as_str().unwrap().contains("hunter2"));

    let activity = parsed["activity"]["rows"]
        .as_array()
        .expect("activity rows");
    let call = activity
        .iter()
        .find(|row| row["kind"] == "codex_tool")
        .expect("codex tool");
    assert!(call["cmd"].as_str().unwrap().contains("Bearer <redacted>"));
    assert!(!call["cmd"].as_str().unwrap().contains("abc.def.ghi"));
}
