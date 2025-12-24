//! Integration tests for vcsql library API.

use std::process::Command;
use tempfile::TempDir;
use vcsql::{GitRepo, SqlEngine, VcsqlError, TABLES};

/// Creates a temporary Git repository with some commits for testing.
fn create_test_repo() -> TempDir {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let path = temp.path();

    // Initialize repo
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("Failed to init git repo");

    // Configure user
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("Failed to set email");

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .expect("Failed to set name");

    // Create initial commit
    std::fs::write(path.join("README.md"), "# Test Repo\n").expect("Failed to write file");

    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("Failed to add files");

    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(path)
        .output()
        .expect("Failed to commit");

    // Create second commit
    std::fs::write(path.join("src.rs"), "fn main() {}\n").expect("Failed to write file");

    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("Failed to add files");

    Command::new("git")
        .args(["commit", "-m", "Add source file"])
        .current_dir(path)
        .output()
        .expect("Failed to commit");

    temp
}

#[test]
fn test_open_repo() {
    let temp = create_test_repo();
    let repo = GitRepo::open(temp.path());
    assert!(repo.is_ok(), "Should open test repository");
}

#[test]
fn test_open_nonexistent_repo() {
    let result = GitRepo::open("/nonexistent/path");
    assert!(matches!(result, Err(VcsqlError::RepoNotFound(_))));
}

#[test]
fn test_sql_engine_new() {
    let engine = SqlEngine::new();
    assert!(engine.is_ok(), "Should create SQL engine");
}

#[test]
fn test_query_commits() {
    let temp = create_test_repo();
    let mut repo = GitRepo::open(temp.path()).expect("Failed to open repo");
    let mut engine = SqlEngine::new().expect("Failed to create engine");

    engine
        .load_tables_for_query("SELECT * FROM commits", &mut repo)
        .expect("Failed to load tables");

    let result = engine
        .execute("SELECT * FROM commits ORDER BY committed_at DESC")
        .expect("Failed to execute query");

    assert_eq!(result.row_count(), 2, "Should have 2 commits");
    assert!(
        result.columns.contains(&"author_name".to_string()),
        "Should have author_name column"
    );
}

#[test]
fn test_query_branches() {
    let temp = create_test_repo();
    let mut repo = GitRepo::open(temp.path()).expect("Failed to open repo");
    let mut engine = SqlEngine::new().expect("Failed to create engine");

    engine
        .load_tables_for_query("SELECT * FROM branches", &mut repo)
        .expect("Failed to load tables");

    let result = engine
        .execute("SELECT name FROM branches")
        .expect("Failed to execute query");

    assert!(result.row_count() >= 1, "Should have at least one branch");
}

#[test]
fn test_extract_table_names() {
    let tables = SqlEngine::extract_table_names("SELECT * FROM commits JOIN branches ON 1=1");
    assert!(tables.contains("commits"));
    assert!(tables.contains("branches"));
}

#[test]
fn test_table_info() {
    assert_eq!(TABLES.len(), 17, "Should have 17 tables defined");

    let table_names: Vec<&str> = TABLES.iter().map(|t| t.name).collect();
    assert!(table_names.contains(&"commits"));
    assert!(table_names.contains(&"branches"));
    assert!(table_names.contains(&"tags"));
    assert!(table_names.contains(&"diffs"));
    assert!(table_names.contains(&"blame"));
}

#[test]
fn test_query_result_to_json() {
    let temp = create_test_repo();
    let mut repo = GitRepo::open(temp.path()).expect("Failed to open repo");
    let mut engine = SqlEngine::new().expect("Failed to create engine");

    engine
        .load_tables_for_query("SELECT * FROM commits", &mut repo)
        .expect("Failed to load tables");

    let result = engine
        .execute("SELECT short_id, summary FROM commits LIMIT 1")
        .expect("Failed to execute query");

    let json = result.to_json_array();
    assert_eq!(json.len(), 1, "Should have 1 JSON object");
    assert!(json[0].is_object(), "Should be a JSON object");
    assert!(json[0].get("short_id").is_some(), "Should have short_id");
    assert!(json[0].get("summary").is_some(), "Should have summary");
}

#[test]
fn test_table_not_found() {
    let temp = create_test_repo();
    let mut repo = GitRepo::open(temp.path()).expect("Failed to open repo");
    let mut engine = SqlEngine::new().expect("Failed to create engine");

    let result = engine.load_table("nonexistent_table", &mut repo);
    assert!(matches!(result, Err(VcsqlError::TableNotFound(_))));
}
