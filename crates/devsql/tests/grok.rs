use assert_cmd::Command;
use data_encoding::BASE32_NOPAD;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const ACCOUNT: &str = "github%7C4741454";
const BOT_A: &str = "69884c90-a572-4638-9b6d-5753ea878c88";
const BOT_B: &str = "a63c857d-3252-47f5-8087-f75ea908a6d6";

struct GrokFixtures {
    root: TempDir,
    grok_dir: PathBuf,
}

impl GrokFixtures {
    fn new() -> Self {
        let root = TempDir::new().expect("temp");
        let grok_dir = root.path().join("Grok Bot");
        std::fs::create_dir_all(grok_dir.join("sand-client-persistence")).expect("persistence dir");
        Self { root, grok_dir }
    }

    fn persistence(&self) -> PathBuf {
        self.grok_dir.join("sand-client-persistence")
    }

    /// Encode a logical key exactly as the desktop app names its blob files.
    fn blob_path(&self, logical_key: &str) -> PathBuf {
        let encoded = BASE32_NOPAD.encode(logical_key.as_bytes()).to_lowercase();
        self.persistence().join(format!("{encoded}.blob"))
    }

    fn write_blob(&self, logical_key: &str, schema_version: u64, value: Value) {
        let body = json!({"schemaVersion": schema_version, "value": value});
        std::fs::write(
            self.blob_path(logical_key),
            serde_json::to_string(&body).expect("blob json"),
        )
        .expect("write blob");
    }

    fn write_roster(&self, rows: Value) {
        self.write_blob(
            &format!("sand.client.slice.account.{ACCOUNT}.roster.last-roster"),
            4,
            json!({"rows": rows}),
        );
    }

    fn write_transcript(&self, bot_id: &str, entries: Value, persisted_at: i64) {
        self.write_blob(
            &format!("sand.client.slice.account.{ACCOUNT}.transcript.replicas.{bot_id}"),
            1,
            json!({
                "entries": entries,
                "epochHint": Value::Null,
                "acceptedSequenceHint": Value::Null,
                "persistedAt": persisted_at,
            }),
        );
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_devsql"));
        command
            .env("DEVSQL_GROK_DIR", &self.grok_dir)
            // Keep the real caches out of the test entirely.
            .env("CODEX_HOME", self.root.path().join("codex"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("HOME", self.root.path());
        command
    }

    fn query(&self, sql: &str) -> Value {
        let output = self
            .command()
            .args([sql, "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("json output")
    }

    fn status(&self) -> Value {
        let output = self
            .command()
            .args(["grok", "status", "--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("status json")
    }

    fn search(&self, args: &[&str]) -> Value {
        let mut command = self.command();
        command.args(["grok", "search"]);
        command.args(args);
        let output = command
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("search json")
    }

    fn scalar(&self, sql: &str) -> String {
        let value = self.query(sql);
        let rows = value.as_array().cloned().unwrap_or_default();
        let first = rows.first().cloned().unwrap_or(Value::Null);
        let obj = first.as_object().cloned().unwrap_or_default();
        obj.values()
            .next()
            .map(render_scalar)
            .unwrap_or_else(|| "".to_string())
    }
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn user_entry(turn: u32, text: &str, ts: i64) -> Value {
    json!({
        "kind": "message",
        "id": format!("t{turn}u"),
        "role": "user",
        "content": text,
        "isStreaming": false,
        "timestampMs": ts,
    })
}

fn send_entry(turn: u32, index: u32, text: &str, ts: i64) -> Value {
    json!({
        "kind": "send-message",
        "id": format!("t{turn}s{index}"),
        "message": {"type": "text", "content": text},
        "timestampMs": ts,
    })
}

fn roster_row(bot_id: &str, name: &str, last_activity: i64) -> Value {
    json!({
        "id": bot_id,
        "name": name,
        "title": name,
        "description": format!("{name} bot"),
        "isGroup": false,
        "origin": "user",
        "path": format!("/home/box/sand-data/agents/{bot_id}/store.db"),
        "createdAt": 1787347952382i64,
        "updatedAt": last_activity,
        "lastActivityAt": last_activity,
        "newestEntryId": "t1u",
    })
}

fn seeded() -> GrokFixtures {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([
        roster_row(BOT_A, "Grokctl", 1_788_704_108_440i64),
        roster_row(BOT_B, "Terri", 1_788_786_153_841i64),
    ]));
    fx.write_transcript(
        BOT_A,
        json!([
            user_entry(0, "can you use grokctl?", 1_787_875_800_675i64),
            send_entry(
                0,
                0,
                "Yes. Here is the command graph.",
                1_787_875_801_000i64
            ),
        ]),
        1_788_704_108_440i64,
    );
    fx.write_transcript(
        BOT_B,
        json!([user_entry(1, "standup card please", 1_788_786_153_837i64)]),
        1_788_786_153_841i64,
    );
    fx
}

#[test]
fn ingests_bots_and_entries_from_local_replicas() {
    let fx = seeded();

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_bots"), "2");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
    assert_eq!(
        fx.scalar(
            "SELECT name FROM grok_bots WHERE bot_id = '69884c90-a572-4638-9b6d-5753ea878c88'"
        ),
        "Grokctl"
    );
    assert_eq!(
        fx.scalar(
            "SELECT remote_store_path FROM grok_bots WHERE bot_id = 'a63c857d-3252-47f5-8087-f75ea908a6d6'"
        ),
        format!("/home/box/sand-data/agents/{BOT_B}/store.db")
    );

    // Epoch millis are projected to RFC3339 so substr(x,1,10) works like the
    // codex_* tables.
    assert_eq!(
        fx.scalar("SELECT substr(timestamp,1,10) FROM grok_entries WHERE entry_id = 't0u'"),
        "2026-08-28"
    );
    assert_eq!(
        fx.scalar("SELECT direction FROM grok_entries WHERE entry_id = 't0s0'"),
        "outbound"
    );
    assert_eq!(
        fx.scalar("SELECT provenance FROM grok_entries WHERE entry_id = 't0u'"),
        "local_replica"
    );
}

#[test]
fn grok_messages_view_exposes_only_text_entries() {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 1i64)]));
    fx.write_transcript(
        BOT_A,
        json!([
            user_entry(0, "hello", 10i64),
            json!({
                "kind": "send-message",
                "id": "t0s0",
                "message": {"type": "widget", "content": {"nested": true}},
                "timestampMs": 11i64,
            }),
            json!({
                "kind": "event",
                "id": "event-15e92eb2-0000-0000-0000-000000000000",
                "event": {"type": "automation-changed"},
                "timestampMs": 12i64,
            }),
        ]),
        100i64,
    );

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
    // A widget must never be JSON-stringified into `text`.
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_messages"), "1");
    assert_eq!(fx.scalar("SELECT text FROM grok_messages"), "hello");
    assert_eq!(fx.scalar("SELECT bot_name FROM grok_messages"), "Grokctl");
    assert_eq!(
        fx.scalar("SELECT event_type FROM grok_entries WHERE kind = 'event'"),
        "automation-changed"
    );
}

#[test]
fn missing_grok_directory_yields_an_empty_table() {
    let root = TempDir::new().expect("temp");
    let mut command = Command::new(env!("CARGO_BIN_EXE_devsql"));
    command
        .env("DEVSQL_GROK_DIR", root.path().join("nonexistent"))
        .env("CODEX_HOME", root.path().join("codex"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("HOME", root.path());
    let output = command
        .args(["SELECT COUNT(*) AS n FROM grok_bots", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).expect("json");
    let rows = value.as_array().expect("rows");
    assert_eq!(rows[0]["n"], json!(0));
}

#[test]
fn malformed_and_undecodable_blobs_do_not_block_healthy_ones() {
    let fx = seeded();

    // Not base32 at all: skipped silently, it is not one of ours.
    std::fs::write(fx.persistence().join("not-base32!!.blob"), "{}").expect("junk blob");
    // Decodable name, invalid JSON body: recorded, other blobs still ingest.
    let broken_key =
        format!("sand.client.slice.account.{ACCOUNT}.transcript.replicas.11111111-1111-1111-1111-111111111111");
    std::fs::write(fx.blob_path(&broken_key), "{not json").expect("broken blob");

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_bots"), "2");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
    // Errors are keyed on (source, error_kind), so a blob that stays broken
    // across runs stays one row.
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM grok_ingest_errors WHERE error_kind = 'blob_parse'"),
        "1"
    );
}

#[test]
fn unknown_entry_kinds_are_stored_rather_than_dropped() {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 1i64)]));
    fx.write_transcript(
        BOT_A,
        json!([json!({
            "kind": "brand-new-kind",
            "id": "t9z1",
            "somethingNovel": {"a": 1},
            "timestampMs": 42i64,
        })]),
        100i64,
    );

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "1");
    assert_eq!(
        fx.scalar("SELECT kind FROM grok_entries WHERE entry_id = 't9z1'"),
        "brand-new-kind"
    );
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_messages"), "0");
    // raw_json is what makes an unknown kind recoverable later.
    let raw = fx.scalar("SELECT raw_json FROM grok_entries WHERE entry_id = 't9z1'");
    assert!(raw.contains("somethingNovel"), "raw_json preserved: {raw}");
}

/// The headline invariant: local replicas are truncated client-side windows, so
/// entries the desktop app evicts must survive in devsql.
#[test]
fn entries_survive_local_replica_truncation() {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 1i64)]));

    let full: Vec<Value> = (1..=10)
        .map(|turn| user_entry(turn, &format!("message {turn}"), 1_000 + i64::from(turn)))
        .collect();
    fx.write_transcript(BOT_A, json!(full), 1_000i64);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "10");

    // The app evicts the first five and rewrites the blob with a newer stamp.
    let truncated: Vec<Value> = (6..=10)
        .map(|turn| user_entry(turn, &format!("message {turn}"), 1_000 + i64::from(turn)))
        .collect();
    fx.write_transcript(BOT_A, json!(truncated), 2_000i64);

    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM grok_entries"),
        "10",
        "evicted entries must not disappear from the index"
    );
    assert_eq!(
        fx.scalar("SELECT text FROM grok_entries WHERE entry_id = 't1u'"),
        "message 1"
    );
    assert_eq!(fx.scalar("SELECT entry_count FROM grok_bots"), "10");
}

#[test]
fn a_bot_removed_from_the_roster_keeps_its_entries() {
    let fx = seeded();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_bots"), "2");

    // Bot B is deleted in the Grok UI: it leaves the roster, but its replica
    // and history stay.
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 2i64)]));

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_bots"), "2");
    assert_eq!(
        fx.scalar(&format!(
            "SELECT roster_present FROM grok_bots WHERE bot_id = '{BOT_B}'"
        )),
        "0"
    );
    assert_eq!(
        fx.scalar(&format!(
            "SELECT COUNT(*) FROM grok_entries WHERE bot_id = '{BOT_B}'"
        )),
        "1"
    );
}

#[test]
fn a_regressing_persisted_at_is_skipped_and_recorded() {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 1i64)]));
    fx.write_transcript(BOT_A, json!([user_entry(1, "first", 10i64)]), 5_000i64);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "1");

    // Same bot, older stamp: a stale rewrite, not new truth.
    fx.write_transcript(
        BOT_A,
        json!([user_entry(2, "stale rewrite", 20i64)]),
        1_000i64,
    );

    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "1");
    assert_eq!(
        fx.scalar(
            "SELECT COUNT(*) FROM grok_ingest_errors WHERE error_kind = 'persisted_at_regression'"
        ),
        "1"
    );
}

#[test]
fn unchanged_blobs_are_not_reparsed_on_a_second_run() {
    let fx = seeded();

    let first = fx.status();
    assert_eq!(
        first["parsed_blobs"],
        json!(3),
        "roster plus two transcripts"
    );
    assert_eq!(first["unchanged_blobs"], json!(0));

    // Nothing on disk moved, so the size/mtime watermark must short-circuit.
    let second = fx.status();
    assert_eq!(
        second["parsed_blobs"],
        json!(0),
        "no blob should be reparsed"
    );
    assert_eq!(second["unchanged_blobs"], json!(3));
    assert_eq!(second["entries"], first["entries"]);
}

#[test]
fn status_reports_coverage_and_roster_state() {
    let fx = seeded();
    let status = fx.status();

    assert_eq!(status["bots"], json!(2));
    assert_eq!(status["bots_in_roster"], json!(2));
    assert_eq!(status["entries"], json!(3));
    assert_eq!(status["messages"], json!(3));
    assert_eq!(status["persistence_dir_present"], json!(true));
    assert_eq!(status["ingest_errors"].as_array().map(Vec::len), Some(0));

    let names: Vec<&str> = status["bot_rows"]
        .as_array()
        .expect("bot rows")
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect();
    assert!(names.contains(&"Terri"), "bot rows: {names:?}");
}

#[test]
fn search_is_global_and_can_filter_by_bot() {
    let fx = seeded();

    let all = fx.search(&["grokctl"]);
    assert_eq!(all["scope"], json!("global"));
    assert_eq!(all["total"], json!(1));
    assert_eq!(all["matches"][0]["bot_name"], json!("Grokctl"));

    let scoped = fx.search(&["standup", "--bot", "Terri"]);
    assert_eq!(scoped["total"], json!(1));
    assert_eq!(scoped["matches"][0]["bot_name"], json!("Terri"));

    let empty = fx.search(&["nothing-matches-this"]);
    assert_eq!(empty["total"], json!(0));
}

#[test]
fn search_rejects_an_unknown_bot() {
    let fx = seeded();
    fx.command()
        .args(["grok", "search", "anything", "--bot", "NoSuchBot"])
        .assert()
        .failure();
}

#[test]
fn entry_id_shapes_are_parsed_or_left_null() {
    let fx = GrokFixtures::new();
    fx.write_roster(json!([roster_row(BOT_A, "Grokctl", 1i64)]));
    fx.write_transcript(
        BOT_A,
        json!([
            send_entry(121, 3, "ordinal", 10i64),
            json!({
                "kind": "send-message",
                "id": "tbs0",
                "message": {"type": "text", "content": "non-numeric turn"},
                "timestampMs": 11i64,
            }),
            json!({
                "kind": "event",
                "id": "event-15e92eb2-0000-0000-0000-000000000000",
                "event": {"type": "automation-changed"},
                "timestampMs": 12i64,
            }),
        ]),
        100i64,
    );

    assert_eq!(
        fx.scalar("SELECT turn_ordinal FROM grok_entries WHERE entry_id = 't121s3'"),
        "121"
    );
    assert_eq!(
        fx.scalar("SELECT entry_suffix FROM grok_entries WHERE entry_id = 't121s3'"),
        "s"
    );
    assert_eq!(
        fx.scalar("SELECT suffix_index FROM grok_entries WHERE entry_id = 't121s3'"),
        "3"
    );

    // A non-numeric turn token must not fabricate an ordinal.
    assert_eq!(
        fx.scalar(
            "SELECT COUNT(*) FROM grok_entries WHERE entry_id = 'tbs0' AND turn_ordinal IS NULL"
        ),
        "1"
    );
    assert_eq!(
        fx.scalar("SELECT turn_token FROM grok_entries WHERE entry_id = 'tbs0'"),
        "b"
    );

    // UUID-shaped ids parse to nothing; ordering falls back to source_order.
    assert_eq!(
        fx.scalar(
            "SELECT COUNT(*) FROM grok_entries
              WHERE entry_id LIKE 'event-%' AND turn_ordinal IS NULL AND turn_token IS NULL"
        ),
        "1"
    );
    assert_eq!(
        fx.scalar("SELECT entry_id FROM grok_entries ORDER BY source_order DESC LIMIT 1"),
        "event-15e92eb2-0000-0000-0000-000000000000"
    );
}

/// Grok is a chat corpus with no command, cwd, or exit code. It must stay out
/// of the cross-source command_events view, whose whole contract is those
/// columns.
#[test]
fn grok_rows_never_leak_into_command_events() {
    let fx = seeded();
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM command_events WHERE source = 'grok'"),
        "0"
    );

    let columns = fx.query("SELECT * FROM command_events LIMIT 1");
    if let Some(first) = columns.as_array().and_then(|rows| rows.first()) {
        let obj = first.as_object().expect("row object");
        assert_eq!(
            obj.len(),
            20,
            "command_events must keep its 20-column contract"
        );
    }
}

/// Grok is an explicit search target, never part of default recall/gather
/// output.
#[test]
fn grok_stays_out_of_recall_and_gather_defaults() {
    let fx = seeded();

    let recall = fx
        .command()
        .args(["recall", "grokctl", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recall: Value = serde_json::from_slice(&recall).expect("recall json");
    let recall_obj = recall.as_object().expect("recall object");
    assert!(
        !recall_obj.contains_key("grok"),
        "recall must not gain a grok section by default"
    );

    let gather = fx
        .command()
        .args(["gather", "grokctl", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let gather: Value = serde_json::from_slice(&gather).expect("gather json");
    let gather_obj = gather.as_object().expect("gather object");
    assert!(
        !gather_obj.contains_key("grok"),
        "gather must not gain a grok section: {:?}",
        gather_obj.keys().collect::<Vec<_>>()
    );

    // Check the rows themselves, not the serialized text: a substring search
    // would match any transcript that merely discusses grok.
    let kinds: Vec<&str> = gather_obj
        .get("prior_work")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("kind").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !kinds.iter().any(|kind| kind.contains("grok")),
        "gather prior_work must not contain grok rows: {kinds:?}"
    );
}

// ---------------------------------------------------------------------------
// Gateway backfill, driven by a fake grokctl. No real network.
// ---------------------------------------------------------------------------

impl GrokFixtures {
    /// Install a stub `grokctl` that replays canned responses.
    ///
    /// `script_body` is shell, receives the real argv, and must print the
    /// envelope devsql expects on stdout.
    fn fake_grokctl(&self, script_body: &str) -> PathBuf {
        let path = self.root.path().join("fake-grokctl");
        std::fs::write(&path, format!("#!/bin/sh\n{script_body}\n")).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        path
    }

    fn sync(&self, grokctl: &Path, extra: &[&str]) -> Value {
        let mut command = self.command();
        command.args(["grok", "sync", "--grokctl"]);
        command.arg(grokctl);
        command.args(extra);
        let output = command
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("sync json")
    }
}

/// Two pages, then a response without a cursor, which ends the walk.
const TWO_PAGE_STUB: &str = r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
case "$*" in
  *beforeSeq*)
    cat <<'JSON'
{"result":{"entries":[
  {"kind":"message","id":"t5u","role":"user","content":"older question","timestampMs":500}
]}}
JSON
    ;;
  *)
    cat <<'JSON'
{"result":{"entries":[
  {"kind":"message","id":"t9u","role":"user","content":"newest question","timestampMs":900},
  {"kind":"send-message","id":"t9s0","message":{"type":"text","content":"newest answer"},"timestampMs":901}
],"nextBeforeSeq":40}}
JSON
    ;;
esac
"#;

#[test]
fn gateway_backfill_pages_until_the_cursor_runs_out() {
    let fx = seeded();
    let stub = fx.fake_grokctl(TWO_PAGE_STUB);

    let result = fx.sync(&stub, &["--bot", "Grokctl"]);
    assert_eq!(result["gateway"]["reachable"], json!(true));

    let bot = &result["bots"][0];
    assert_eq!(bot["name"], json!("Grokctl"));
    assert_eq!(bot["pages"], json!(2), "second page has no cursor");
    assert_eq!(bot["complete"], json!(true));

    // Two local entries plus three from the gateway.
    assert_eq!(
        fx.scalar(&format!(
            "SELECT COUNT(*) FROM grok_entries WHERE bot_id = '{BOT_A}'"
        )),
        "5"
    );
    assert_eq!(
        fx.scalar("SELECT text FROM grok_entries WHERE entry_id = 't5u'"),
        "older question"
    );
}

#[test]
fn gateway_entries_dedupe_against_local_rows_by_entry_id() {
    let fx = seeded();
    // The gateway returns the same entry_id the local replica already holds.
    let stub = fx.fake_grokctl(
        r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
cat <<'JSON'
{"result":{"entries":[
  {"kind":"message","id":"t0u","role":"user","content":"can you use grokctl?","timestampMs":1787875800675}
]}}
JSON
"#,
    );

    let before = fx.scalar("SELECT COUNT(*) FROM grok_entries");
    fx.sync(&stub, &["--bot", "Grokctl"]);
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM grok_entries"),
        before,
        "a duplicate entry_id must merge, not insert"
    );
    // Seen from both paths, which is how a lagging local cache is detected.
    assert_eq!(
        fx.scalar("SELECT provenance FROM grok_entries WHERE entry_id = 't0u'"),
        "both"
    );
}

#[test]
fn a_non_decreasing_cursor_terminates_the_walk() {
    let fx = seeded();
    // Always answers with the same cursor: a naive loop would never stop.
    let stub = fx.fake_grokctl(
        r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
cat <<'JSON'
{"result":{"entries":[
  {"kind":"message","id":"t7u","role":"user","content":"stuck","timestampMs":700}
],"nextBeforeSeq":40}}
JSON
"#,
    );

    let result = fx.sync(
        &stub,
        &["--bot", "Grokctl", "--max-pages=99", "--full", "true"],
    );
    let pages = result["bots"][0]["pages"].as_u64().expect("pages");
    assert!(
        pages <= 2,
        "the guard must stop the walk quickly, got {pages}"
    );
}

#[test]
fn a_failing_grokctl_still_commits_local_ingest() {
    let fx = seeded();
    let stub = fx.fake_grokctl(
        r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
echo "gateway unreachable" >&2
exit 1
"#,
    );

    // The command succeeds: local replicas are already committed.
    let result = fx.sync(&stub, &["--bot", "Grokctl"]);
    assert_eq!(result["local"]["entries"], json!(3));
    assert!(
        result["bots"][0]["note"].is_string(),
        "the failure must be reported as a note"
    );
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM grok_ingest_errors WHERE error_kind = 'gateway_page'"),
        "1"
    );
}

#[test]
fn non_json_grokctl_output_is_reported_not_fatal() {
    let fx = seeded();
    let stub = fx.fake_grokctl(
        r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
echo "this is not json"
"#,
    );

    let result = fx.sync(&stub, &["--bot", "Grokctl"]);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
    assert!(result["bots"][0]["note"].is_string());
}

/// An empty transcript comes back with exit 0, so emptiness must never be read
/// as an error.
#[test]
fn an_empty_transcript_is_not_an_error() {
    let fx = seeded();
    let stub = fx.fake_grokctl(
        r#"
case "$*" in
  *--version*) echo "fake 0.0.0"; exit 0 ;;
esac
echo '{"result":{"entries":[]}}'
"#,
    );

    let result = fx.sync(&stub, &["--bot", "Grokctl"]);
    assert_eq!(result["gateway"]["reachable"], json!(true));
    assert_eq!(result["bots"][0]["entries_written"], json!(0));
    assert!(
        result["bots"][0]["note"].is_null(),
        "empty is not a failure"
    );
}

#[test]
fn a_missing_grokctl_binary_degrades_to_local_only() {
    let fx = seeded();
    let missing = fx.root.path().join("definitely-not-installed");

    let result = fx.sync(&missing, &[]);
    assert_eq!(result["gateway"]["reachable"], json!(false));
    assert!(result["gateway"]["reason"].is_string());
    assert_eq!(result["local"]["entries"], json!(3));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM grok_entries"), "3");
}
