use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn parse_json(output: &[u8]) -> Value {
    serde_json::from_slice(output).expect("valid json")
}

#[cfg(unix)]
fn write_mock_log(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("mock-log");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n: > \"$MOCK_LOG_ARGS\"\nfor arg in \"$@\"; do printf '<%s>\\n' \"$arg\" >> \"$MOCK_LOG_ARGS\"; done\ncat <<'EOF'\n{body}\nEOF\n"
        ),
    )
    .expect("mock log");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}

#[cfg(unix)]
fn write_slow_mock_log(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("slow-mock-log");
    fs::write(
        &path,
        format!("#!/bin/sh\ncat <<'EOF'\n{body}\nEOF\nexec sleep 5\n"),
    )
    .expect("slow mock log");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}

#[cfg(unix)]
fn write_failing_mock_log(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("failing-mock-log");
    fs::write(&path, "#!/bin/sh\nprintf 'log access denied' >&2\nexit 7\n")
        .expect("failing mock log");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}

#[cfg(unix)]
#[test]
fn macos_logs_query_pushes_safe_options_and_preserves_raw_json() {
    let temp = TempDir::new().expect("temp");
    let raw = r#"{"timestamp":"2026-08-07 10:00:00.000000-0400","eventType":"logEvent","subsystem":"com.example.app","category":"network","process":"ExampleApp","processID":4242,"threadID":99,"sender":"ExampleApp","eventMessage":"hello from logs","messageType":"Info","activityIdentifier":123,"traceID":456,"bootUUID":"BOOT-1"}"#;
    let log_bin = write_mock_log(temp.path(), raw);
    let args_path = temp.path().join("args.txt");

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT timestamp, subsystem, category, process, process_id, thread_id, \
             sender, message, level, activity_id, trace_id, boot_uuid, raw_json, provenance, \
             source, archive_path, source_order, timestamp_ms \
             FROM macos_logs \
             WHERE subsystem = 'com.example.app' AND process = 'ExampleApp' \
             AND process_id = 4242 AND category = 'network' AND level = 'Info' \
             AND event_type = 'logEvent' AND message LIKE '%hello%'",
            "--log-start",
            "2026-08-07 09:45:00",
            "--log-end",
            "2026-08-07 10:15:00",
            "--log-predicate",
            "category == \"network\"",
            "--log-archive",
            "/tmp/example.logarchive",
            "--log-level",
            "info",
            "--log-max-rows",
            "7",
            "--log-timeout",
            "9",
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
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], "2026-08-07 10:00:00.000000-0400");
    assert_eq!(rows[0]["subsystem"], "com.example.app");
    assert_eq!(rows[0]["category"], "network");
    assert_eq!(rows[0]["process"], "ExampleApp");
    assert_eq!(rows[0]["process_id"], 4242);
    assert_eq!(rows[0]["thread_id"], 99);
    assert_eq!(rows[0]["sender"], "ExampleApp");
    assert_eq!(rows[0]["message"], "hello from logs");
    assert_eq!(rows[0]["level"], "Info");
    assert_eq!(rows[0]["activity_id"], 123);
    assert_eq!(rows[0]["trace_id"], 456);
    assert_eq!(rows[0]["boot_uuid"], "BOOT-1");
    assert_eq!(rows[0]["raw_json"], raw);
    assert_eq!(rows[0]["provenance"], "macos_unified_log");
    assert_eq!(rows[0]["source"], "archive");
    assert_eq!(rows[0]["archive_path"], "/tmp/example.logarchive");
    assert_eq!(rows[0]["source_order"], 0);
    assert!(rows[0]["timestamp_ms"].as_i64().is_some());

    let args = fs::read_to_string(args_path).expect("args");
    assert!(args.contains("<show>\n<--style>\n<ndjson>"));
    assert!(args.contains("<--start>\n<2026-08-07 09:45:00>"));
    assert!(args.contains("<--end>\n<2026-08-07 10:15:00>"));
    assert!(args.contains("category == \"network\""));
    assert!(args.contains("subsystem == \"com.example.app\""));
    assert!(args.contains("process == \"ExampleApp\""));
    assert!(args.contains("processIdentifier == 4242"));
    assert!(args.contains("category == \"network\""));
    assert!(args.contains("logType == \"Info\""));
    assert!(args.contains("type == \"logEvent\""));
    assert!(args.contains("composedMessage CONTAINS \"hello\""));
    assert!(args.contains("<--archive>\n</tmp/example.logarchive>"));
    assert!(args.contains("<--info>"));
    assert!(!args.contains("<--last>\n<15m>"));
}

#[cfg(unix)]
#[test]
fn macos_logs_skips_completion_sentinels_and_blank_lines() {
    let temp = TempDir::new().expect("temp");
    let body = concat!(
        "\n",
        "{\"count\":1,\"finished\":1}\n",
        "{\"timestamp\":\"2026-08-07 10:00:00.000000-0400\",\"eventType\":\"logEvent\",\"eventMessage\":\"one event\"}"
    );
    let log_bin = write_mock_log(temp.path(), body);
    let args_path = temp.path().join("args.txt");

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT COUNT(*) AS count FROM macos_logs",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let rows = parse_json(&output);
    assert_eq!(rows[0]["count"], 1);
}

#[cfg(unix)]
#[test]
fn macos_logs_debug_level_includes_info_and_debug() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), "");
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args(["SELECT COUNT(*) FROM macos_logs", "--log-level", "debug"])
        .assert()
        .success();

    let args = fs::read_to_string(args_path).expect("args");
    assert!(args.contains("<--info>"));
    assert!(args.contains("<--debug>"));
}

#[cfg(unix)]
#[test]
fn macos_logs_timeout_returns_partial_rows_with_a_warning() {
    let temp = TempDir::new().expect("temp");
    let raw = r#"{"timestamp":"2026-08-07 10:00:00.000000-0400","eventType":"logEvent","eventMessage":"before timeout"}"#;
    let log_bin = write_slow_mock_log(temp.path(), raw);

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .args([
            "SELECT message FROM macos_logs",
            "--log-timeout",
            "1",
            "--format",
            "json",
            "--full-output",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("before timeout"))
        .stdout(predicate::str::contains("partial scan"))
        .stdout(predicate::str::contains("timeout after 1 seconds"));
}

#[cfg(unix)]
#[test]
fn macos_logs_row_cap_returns_partial_rows_with_a_warning() {
    let temp = TempDir::new().expect("temp");
    let body = concat!(
        "{\"timestamp\":\"2026-08-07 10:00:00.000000-0400\",\"eventType\":\"logEvent\",\"eventMessage\":\"first\"}\n",
        "{\"timestamp\":\"2026-08-07 10:00:01.000000-0400\",\"eventType\":\"logEvent\",\"eventMessage\":\"second\"}"
    );
    let log_bin = write_mock_log(temp.path(), body);
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT message FROM macos_logs",
            "--log-max-rows",
            "1",
            "--format",
            "json",
            "--full-output",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("first"))
        .stdout(predicate::str::contains("row limit 1 reached"));
}

#[cfg(unix)]
#[test]
fn macos_logs_defaults_to_recent_standard_bounded_query() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), "");
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT COUNT(*) AS count FROM macos_logs",
            "--format",
            "json",
        ])
        .assert()
        .success();

    let args = fs::read_to_string(args_path).expect("args");
    assert!(args.contains("<show>\n<--style>\n<ndjson>"));
    assert!(args.contains("<--last>\n<15m>"));
    assert!(!args.contains("<--debug>"));
    assert!(!args.contains("<--info>"));
}

#[cfg(unix)]
#[test]
fn macos_logs_surfaces_malformed_ndjson() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), "not-json");
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args(["SELECT message FROM macos_logs", "--format", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("invalid macOS log NDJSON"));
}

#[cfg(unix)]
#[test]
fn macos_logs_sql_limit_is_not_reported_as_truncation() {
    let temp = TempDir::new().expect("temp");
    let body = concat!(
        "{\"eventMessage\":\"first\"}\n",
        "{\"eventMessage\":\"second\"}"
    );
    let log_bin = write_mock_log(temp.path(), body);
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT message FROM macos_logs LIMIT 1",
            "--format",
            "json",
            "--full-output",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("first"))
        .stdout(predicate::str::contains("second").not())
        .stdout(predicate::str::contains("row limit").not());
}

#[cfg(unix)]
#[test]
fn macos_logs_limit_zero_does_not_spawn_the_log_process() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), r#"{"eventMessage":"unexpected"}"#);
    let args_path = temp.path().join("args.txt");

    let output = Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args(["SELECT message FROM macos_logs LIMIT 0", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(parse_json(&output), serde_json::json!([]));
    assert!(!args_path.exists());
}

#[cfg(unix)]
#[test]
fn macos_logs_surfaces_command_failure_and_stderr() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_failing_mock_log(temp.path());

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .args(["SELECT message FROM macos_logs", "--format", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("exit status: 7"))
        .stdout(predicate::str::contains("log access denied"));
}

#[cfg(unix)]
#[test]
fn macos_logs_passes_predicates_as_literal_arguments() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), "");
    let args_path = temp.path().join("args.txt");
    let marker = temp.path().join("injected");
    let predicate = format!("message == \"$(touch {})\"", marker.display());

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT COUNT(*) FROM macos_logs",
            "--log-predicate",
            &predicate,
        ])
        .assert()
        .success();

    assert!(!marker.exists());
    let args = fs::read_to_string(args_path).expect("args");
    let matching_args: Vec<_> = args
        .lines()
        .filter(|argument| argument.contains(&predicate))
        .collect();
    assert_eq!(matching_args.len(), 1, "{args:?}");
    assert!(matching_args[0].starts_with('<') && matching_args[0].ends_with('>'));
}

#[cfg(unix)]
#[test]
fn macos_logs_rejects_conflicting_windows_and_invalid_last_values() {
    let temp = TempDir::new().expect("temp");
    let log_bin = write_mock_log(temp.path(), "");
    let args_path = temp.path().join("args.txt");

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args([
            "SELECT COUNT(*) FROM macos_logs",
            "--log-last",
            "5m",
            "--log-start",
            "2026-08-07 09:45:00",
            "--log-end",
            "2026-08-07 10:15:00",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "either last or start/end, not both",
        ));

    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .env("DEVSQL_MACOS_LOG_BIN", &log_bin)
        .env("MOCK_LOG_ARGS", &args_path)
        .args(["SELECT COUNT(*) FROM macos_logs", "--log-last", "forever"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "last must be boot or a positive number",
        ));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn macos_logs_reports_explicit_unsupported_error_without_mock_log() {
    Command::new(env!("CARGO_BIN_EXE_devsql"))
        .args([
            "SELECT COUNT(*) AS count FROM macos_logs",
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "macos_logs is only supported on macOS",
        ));
}
