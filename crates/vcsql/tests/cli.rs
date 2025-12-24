//! CLI integration tests using assert_cmd.

use assert_cmd::Command;
use predicates::prelude::*;
use std::process;
use tempfile::TempDir;

/// Creates a temporary Git repository for CLI testing.
fn create_test_repo() -> TempDir {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let path = temp.path();

    process::Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("Failed to init git repo");

    process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("Failed to set email");

    process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .expect("Failed to set name");

    std::fs::write(path.join("README.md"), "# Test\n").expect("Failed to write file");

    process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("Failed to add files");

    process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(path)
        .output()
        .expect("Failed to commit");

    temp
}

#[test]
fn test_help() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("SQL query engine for Git"));
}

#[test]
fn test_version() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("vcsql"));
}

#[test]
fn test_tables_command() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.arg("tables")
        .assert()
        .success()
        .stdout(predicate::str::contains("commits"))
        .stdout(predicate::str::contains("branches"));
}

#[test]
fn test_schema_command() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.args(["schema", "commits"])
        .assert()
        .success()
        .stdout(predicate::str::contains("author_name"))
        .stdout(predicate::str::contains("committed_at"));
}

#[test]
fn test_examples_command() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.arg("examples")
        .assert()
        .success()
        .stdout(predicate::str::contains("SELECT"));
}

#[test]
fn test_query_execution() {
    let temp = create_test_repo();

    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.args(["--repo", temp.path().to_str().unwrap()])
        .arg("SELECT short_id, summary FROM commits")
        .assert()
        .success()
        .stdout(predicate::str::contains("Initial commit"));
}

#[test]
fn test_json_output() {
    let temp = create_test_repo();

    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.args(["--repo", temp.path().to_str().unwrap(), "--format", "json"])
        .arg("SELECT short_id, summary FROM commits")
        .assert()
        .success()
        .stdout(predicate::str::contains("["))
        .stdout(predicate::str::contains("\"summary\""));
}

#[test]
fn test_csv_output() {
    let temp = create_test_repo();

    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.args(["--repo", temp.path().to_str().unwrap(), "--format", "csv"])
        .arg("SELECT short_id, summary FROM commits")
        .assert()
        .success()
        .stdout(predicate::str::contains("short_id,summary"));
}

#[test]
fn test_nonexistent_repo() {
    let mut cmd = Command::cargo_bin("vcsql").unwrap();
    cmd.args(["--repo", "/nonexistent/path"])
        .arg("SELECT * FROM commits")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Repository not found"));
}
