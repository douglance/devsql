use assert_cmd::Command;
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

struct ShellFixtures {
    root: TempDir,
    atuin_db: PathBuf,
    zsh_history: PathBuf,
    bash_history: PathBuf,
}

impl ShellFixtures {
    fn new() -> Self {
        let root = TempDir::new().expect("temp");
        let atuin_db = root.path().join("atuin.db");
        let zsh_history = root.path().join("zsh_history");
        let bash_history = root.path().join("bash_history");

        let conn = Connection::open(&atuin_db).expect("atuin db");
        conn.execute_batch(
            "CREATE TABLE history (
                id TEXT PRIMARY KEY,
                timestamp INTEGER NOT NULL,
                duration INTEGER NOT NULL,
                exit INTEGER NOT NULL,
                command TEXT NOT NULL,
                cwd TEXT NOT NULL,
                session TEXT NOT NULL,
                hostname TEXT NOT NULL,
                deleted_at INTEGER
            );
            INSERT INTO history VALUES (
                'atuin-1', 1754402102000000000, 3500000000, 0,
                'echo shared-shell-term TOKEN=super-secret',
                '/work/app', 'session-1', 'host-a', NULL
            );
            INSERT INTO history VALUES (
                'atuin-deleted', 1754402103000000000, 1, 1,
                'deleted-shell-term', '/work/app', 'session-1', 'host-a', 1754402200
            );",
        )
        .expect("atuin fixture");

        std::fs::write(
            &zsh_history,
            concat!(
                ": 1754402200:3;echo shared-shell-term\n",
                ": 1754402201:1;printf 'multi \\\n",
                "line'\n",
                "plain-zsh-shell-term\n",
            ),
        )
        .expect("zsh fixture");

        std::fs::write(
            &bash_history,
            concat!(
                "#1754402300\n",
                "echo bash-shell-term\n",
                "plain-bash-shell-term\n",
            ),
        )
        .expect("bash fixture");

        Self {
            root,
            atuin_db,
            zsh_history,
            bash_history,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_devsql"));
        command
            .env("DEVSQL_ATUIN_DB", &self.atuin_db)
            .env("DEVSQL_ZSH_HISTORY", &self.zsh_history)
            .env("DEVSQL_BASH_HISTORY", &self.bash_history);
        command
    }

    fn empty_claude_dir(&self) -> PathBuf {
        let path = self.root.path().join("claude");
        std::fs::create_dir_all(&path).expect("claude dir");
        path
    }

    fn git_repo(&self) -> PathBuf {
        let path = self.root.path().join("repo");
        let repo = git2::Repository::init(&path).expect("git init");
        let signature = git2::Signature::now("Test", "test@example.com").expect("signature");
        let mut index = repo.index().expect("index");
        let tree_id = index.write_tree().expect("tree id");
        let tree = repo.find_tree(tree_id).expect("tree");
        repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .expect("commit");
        path
    }
}

fn parse_json(output: &[u8]) -> Value {
    serde_json::from_slice(output).expect("valid json")
}

#[test]
fn query_unifies_atuin_zsh_and_bash_without_deduplicating() {
    let fixtures = ShellFixtures::new();
    let output = fixtures
        .command()
        .args([
            "SELECT source, source_id, source_order, timestamp, duration_ms, \
             exit_code, command, cwd, session_id, hostname, history_path \
             FROM shell_history ORDER BY source, source_order",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows = parse_json(&output);
    let rows = rows.as_array().expect("rows");
    assert_eq!(rows.len(), 6, "deleted Atuin rows must be excluded");

    let atuin = rows
        .iter()
        .find(|row| row["source"] == "atuin")
        .expect("atuin row");
    assert_eq!(atuin["source_id"], "atuin-1");
    assert_eq!(atuin["duration_ms"], 3500);
    assert_eq!(atuin["exit_code"], 0);
    assert_eq!(atuin["cwd"], "/work/app");
    assert_eq!(atuin["session_id"], "session-1");
    assert_eq!(atuin["hostname"], "host-a");
    assert!(atuin["timestamp"].as_str().unwrap().ends_with('Z'));

    let zsh_rows: Vec<&Value> = rows.iter().filter(|row| row["source"] == "zsh").collect();
    assert_eq!(zsh_rows.len(), 3);
    assert_eq!(zsh_rows[0]["duration_ms"], 3000);
    assert_eq!(zsh_rows[1]["command"], "printf 'multi \\\nline'");
    assert!(zsh_rows[2]["timestamp"].is_null());

    let bash_rows: Vec<&Value> = rows.iter().filter(|row| row["source"] == "bash").collect();
    assert_eq!(bash_rows.len(), 2);
    assert!(bash_rows[0]["timestamp"].as_str().unwrap().ends_with('Z'));
    assert!(bash_rows[1]["timestamp"].is_null());

    let duplicate_count = rows
        .iter()
        .filter(|row| {
            row["command"]
                .as_str()
                .unwrap_or("")
                .contains("shared-shell-term")
        })
        .count();
    assert_eq!(duplicate_count, 2);
}

#[test]
fn missing_optional_sources_yield_an_empty_table() {
    let root = TempDir::new().expect("temp");
    let missing = |name: &str| root.path().join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_ATUIN_DB", missing("atuin.db"))
        .env("DEVSQL_ZSH_HISTORY", missing("zsh_history"))
        .env("DEVSQL_BASH_HISTORY", missing("bash_history"))
        .args([
            "SELECT COUNT(*) AS count FROM shell_history",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows = parse_json(&output);
    assert_eq!(rows[0]["count"], 0);
}

#[test]
fn recall_always_returns_matching_raw_shell_commands() {
    let fixtures = ShellFixtures::new();
    let claude = fixtures.empty_claude_dir();
    let repo = fixtures.git_repo();
    let output = fixtures
        .command()
        .args([
            "recall",
            "shared-shell-term",
            "--data-dir",
            claude.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result = parse_json(&output);
    let commands = result["commands"].as_array().expect("commands");
    assert_eq!(commands.len(), 2);
    assert!(commands.iter().any(|row| row["command"]
        .as_str()
        .unwrap()
        .contains("TOKEN=super-secret")));
    assert_eq!(result["total"], 2);
}

#[test]
fn gather_always_adds_matching_shell_commands_to_activity() {
    let fixtures = ShellFixtures::new();
    let claude = fixtures.empty_claude_dir();
    let codex = fixtures.root.path().join("codex");
    std::fs::create_dir_all(&codex).expect("codex dir");
    let repo = fixtures.git_repo();

    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "gather",
            "bash-shell-term",
            "--data-dir",
            claude.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result = parse_json(&output);
    assert_eq!(
        result.as_object().unwrap().len(),
        8,
        "gather must retain its six sections plus terms and budget"
    );
    let activity = result["activity"]["rows"]
        .as_array()
        .expect("activity rows");
    assert!(activity.iter().any(|row| {
        row["kind"] == "shell_command"
            && row["command"]
                .as_str()
                .unwrap_or("")
                .contains("bash-shell-term")
    }));
}
