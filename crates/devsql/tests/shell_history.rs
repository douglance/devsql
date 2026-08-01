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
            .env("DEVSQL_BASH_HISTORY", &self.bash_history)
            .env("CODEX_HOME", self.root.path().join("codex"));
        command
    }

    fn empty_claude_dir(&self) -> PathBuf {
        let path = self.root.path().join("claude");
        std::fs::create_dir_all(&path).expect("claude dir");
        path
    }

    fn agent_history(&self) -> (PathBuf, PathBuf) {
        let claude = self.root.path().join("claude");
        let claude_session = claude
            .join("projects")
            .join("-work-claude")
            .join("claude-session.jsonl");
        std::fs::create_dir_all(claude_session.parent().unwrap()).expect("claude projects");
        std::fs::write(
            &claude_session,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-07-12T10:00:00.000Z","sessionId":"claude-session","cwd":"/work/claude","entrypoint":"cli","message":{"content":[{"type":"tool_use","id":"toolu_bash_1","name":"Bash","input":{"command":"printf 'provenance-agent-term\nsecond line'"}}]}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-07-12T10:00:01.000Z","sessionId":"claude-session","cwd":"/work/claude","entrypoint":"cli","message":{"content":[{"type":"tool_use","id":"toolu_read_1","name":"Read","input":{"file_path":"/work/claude/src/lib.rs"}}]}}"#,
                "\n",
            ),
        )
        .expect("claude fixture");
        let claude_subagent = claude
            .join("projects")
            .join("-work-claude")
            .join("claude-session")
            .join("subagents")
            .join("agent-research.jsonl");
        std::fs::create_dir_all(claude_subagent.parent().unwrap()).expect("claude subagents");
        std::fs::write(
            &claude_subagent,
            concat!(
                "malformed json is ignored\n",
                r#"{"type":"assistant","timestamp":"2026-07-12T10:01:00.000Z","sessionId":"claude-session","cwd":"/work/claude","entrypoint":"cli","agentName":"researcher","message":{"content":[{"type":"tool_use","id":"toolu_subagent_1","name":"Bash","input":{"command":"echo claude-subagent-provenance"}}]}}"#,
                "\n",
            ),
        )
        .expect("claude subagent fixture");

        let codex = self.root.path().join("codex");
        let codex_session = codex
            .join("sessions")
            .join("2026")
            .join("07")
            .join("rollout-provenance.jsonl");
        std::fs::create_dir_all(codex_session.parent().unwrap()).expect("codex sessions");
        std::fs::write(
            &codex_session,
            concat!(
                r#"{"timestamp":"2026-07-12T11:00:00.000Z","type":"session_meta","payload":{"session_id":"codex-session","cwd":"/work/codex-default","parent_thread_id":"codex-parent","agent_path":"/root/provenance","agent_role":"engineer","originator":"codex-tui"}}"#,
                "\n",
                r#"{"timestamp":"2026-07-12T11:00:01.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"echo provenance-agent-term\",\"workdir\":\"/work/codex-call\"}","call_id":"call_exec_1"}}"#,
                "\n",
                r#"{"timestamp":"2026-07-12T11:00:02.000Z","type":"response_item","payload":{"type":"function_call","name":"wait","arguments":"{\"cell_id\":\"1\"}","call_id":"call_wait_1"}}"#,
                "\n",
            ),
        )
        .expect("codex fixture");

        (claude, codex)
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
fn command_events_preserve_exact_source_native_provenance() {
    let fixtures = ShellFixtures::new();
    let (claude, codex) = fixtures.agent_history();
    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "SELECT source, channel, actor, provenance_quality, provenance_reason, \
                    source_id, session_id, parent_session_id, agent_id, agent_role, \
                    originator, tool_name, command, cwd, source_path \
             FROM command_events \
             WHERE command LIKE '%provenance-agent-term%' \
             ORDER BY source",
            "--data-dir",
            claude.to_str().unwrap(),
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
    assert_eq!(
        rows.len(),
        2,
        "only command-bearing agent tool calls qualify"
    );

    let claude_row = rows.iter().find(|row| row["source"] == "claude").unwrap();
    assert_eq!(claude_row["channel"], "agent_tool");
    assert_eq!(claude_row["actor"], "agent");
    assert_eq!(claude_row["provenance_quality"], "exact");
    assert!(claude_row["provenance_reason"].is_null());
    assert_eq!(claude_row["source_id"], "toolu_bash_1");
    assert_eq!(claude_row["session_id"], "claude-session");
    assert!(claude_row["parent_session_id"].is_null());
    assert!(claude_row["agent_id"].is_null());
    assert!(claude_row["agent_role"].is_null());
    assert_eq!(claude_row["originator"], "cli");
    assert_eq!(claude_row["tool_name"], "Bash");
    assert_eq!(
        claude_row["command"],
        "printf 'provenance-agent-term\nsecond line'"
    );
    assert_eq!(claude_row["cwd"], "/work/claude");
    assert!(claude_row["source_path"]
        .as_str()
        .unwrap()
        .ends_with("claude-session.jsonl"));

    let codex_row = rows.iter().find(|row| row["source"] == "codex").unwrap();
    assert_eq!(codex_row["channel"], "agent_tool");
    assert_eq!(codex_row["actor"], "agent");
    assert_eq!(codex_row["provenance_quality"], "exact");
    assert!(codex_row["provenance_reason"].is_null());
    assert_eq!(codex_row["source_id"], "call_exec_1");
    assert_eq!(codex_row["session_id"], "codex-session");
    assert_eq!(codex_row["parent_session_id"], "codex-parent");
    assert_eq!(codex_row["agent_id"], "/root/provenance");
    assert_eq!(codex_row["agent_role"], "engineer");
    assert_eq!(codex_row["originator"], "codex-tui");
    assert_eq!(codex_row["tool_name"], "exec_command");
    assert_eq!(codex_row["command"], "echo provenance-agent-term");
    assert_eq!(codex_row["cwd"], "/work/codex-call");
    assert!(codex_row["source_path"]
        .as_str()
        .unwrap()
        .ends_with("rollout-provenance.jsonl"));
}

#[test]
fn command_events_mark_shell_history_as_unattributed() {
    let fixtures = ShellFixtures::new();
    let claude = fixtures.empty_claude_dir();
    let codex = fixtures.root.path().join("codex");
    std::fs::create_dir_all(&codex).expect("codex dir");
    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "SELECT source, channel, actor, provenance_quality, provenance_reason, \
                    tool_name, command \
             FROM command_events \
             WHERE command LIKE '%shared-shell-term%' \
             ORDER BY source",
            "--data-dir",
            claude.to_str().unwrap(),
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
    assert_eq!(rows.len(), 2, "source-native duplicates remain visible");
    for row in rows {
        assert_eq!(row["channel"], "shell");
        assert_eq!(row["actor"], "unknown");
        assert_eq!(row["provenance_quality"], "unattributed");
        assert_eq!(row["provenance_reason"], "unattributed_shell_history");
        assert!(row["tool_name"].is_null());
    }
}

#[test]
fn command_events_preserve_claude_subagent_identity() {
    let fixtures = ShellFixtures::new();
    let (claude, codex) = fixtures.agent_history();
    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "SELECT session_id, parent_session_id, agent_id, agent_role, command \
             FROM command_events \
             WHERE command = 'echo claude-subagent-provenance'",
            "--data-dir",
            claude.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows = parse_json(&output);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["session_id"], "claude-session");
    assert_eq!(rows[0]["parent_session_id"], "claude-session");
    assert_eq!(rows[0]["agent_id"], "agent-research");
    assert_eq!(rows[0]["agent_role"], "researcher");
}

#[test]
fn command_events_expose_the_stable_public_schema() {
    let fixtures = ShellFixtures::new();
    let claude = fixtures.empty_claude_dir();
    let output = fixtures
        .command()
        .args([
            "SELECT name FROM pragma_table_info('command_events') ORDER BY cid",
            "--data-dir",
            claude.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows = parse_json(&output);
    let names: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "source",
            "channel",
            "actor",
            "provenance_quality",
            "provenance_reason",
            "source_id",
            "source_order",
            "session_id",
            "parent_session_id",
            "agent_id",
            "agent_role",
            "originator",
            "tool_name",
            "timestamp",
            "duration_ms",
            "exit_code",
            "command",
            "cwd",
            "hostname",
            "source_path",
        ]
    );
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
    assert!(commands.iter().all(|row| row["actor"] == "unknown"));
    assert_eq!(result["total"], 2);
}

#[test]
fn recall_returns_exact_agent_command_provenance() {
    let fixtures = ShellFixtures::new();
    let (claude, codex) = fixtures.agent_history();
    let repo = fixtures.git_repo();
    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "recall",
            "provenance-agent-term",
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
    assert!(commands.iter().all(|row| {
        row["actor"] == "agent"
            && row["provenance_quality"] == "exact"
            && row["channel"] == "agent_tool"
    }));
    assert!(commands
        .iter()
        .any(|row| row["agent_role"] == "engineer" && row["session_id"] == "codex-session"));
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
        9,
        "typed gather must retain its six sections, terms, budget, and CTA"
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

#[test]
fn gather_labels_agent_commands_without_double_counting_command_tools() {
    let fixtures = ShellFixtures::new();
    let (claude, codex) = fixtures.agent_history();
    let repo = fixtures.git_repo();

    let output = fixtures
        .command()
        .env("CODEX_HOME", codex)
        .args([
            "gather",
            "provenance-agent-term",
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
    let activity = result["activity"]["rows"]
        .as_array()
        .expect("activity rows");
    let agent_commands: Vec<&Value> = activity
        .iter()
        .filter(|row| row["kind"] == "agent_command")
        .collect();
    assert_eq!(agent_commands.len(), 2);
    assert!(!activity.iter().any(|row| {
        row["kind"] == "tool"
            && (row["text"] == "Bash" || row["text"] == "exec_command" || row["text"] == "shell")
    }));
}
