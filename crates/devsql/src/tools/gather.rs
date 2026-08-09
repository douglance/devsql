//! `devsql gather` -- run every context-gathering action concurrently and
//! return one token-budgeted bundle: prior work, repo state, code search,
//! symbols, excerpts, and recent activity, all ranked by search terms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example, TypedContext};
use incurs::output::CommandResult;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Value};

use super::engine_from_options;
use super::recall::{match_expr, score_expr};
use super::{legacy_context, read_only_mcp, typed_from_result};
use crate::UnifiedEngine;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct GatherArgs {
    /// Space-separated terms to gather context for
    terms: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct GatherOptions {
    /// Git repository path (scopes repo_state, code_search, symbols, excerpts)
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Token budget for the bundle; lowest-ranked rows are dropped round-robin
    /// per section (never mid-row) until the bundle fits
    #[incurs(default = 8000)]
    budget: i64,
}

/// Rows fetched per section before budget trimming.
const SECTION_LIMIT: i64 = 10;
const RECENT_CODEX_SEARCH_LIMIT: i64 = 50_000;
const RECENT_ACTIVITY_SEARCH_LIMIT: i64 = 10_000;
const RECENT_COMMAND_SEARCH_LIMIT: i64 = 2_000;
const RECENT_AGENT_COMMANDS_PER_SOURCE: i64 = RECENT_COMMAND_SEARCH_LIMIT / 2;
const RECENT_CODEX_COMMAND_SCAN_LIMIT: i64 = 5_000;

/// Section names, in materialization/round-robin-trim order.
const SECTION_ORDER: [&str; 6] = [
    "prior_work",
    "repo_state",
    "code_search",
    "symbols",
    "excerpts",
    "activity",
];

// ---------------------------------------------------------------------------
// Section result
// ---------------------------------------------------------------------------

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct SectionResult {
    rows: Vec<Value>,
    note: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct GatherOutput {
    terms: Vec<String>,
    budget: usize,
    prior_work: SectionResult,
    repo_state: SectionResult,
    code_search: SectionResult,
    symbols: SectionResult,
    excerpts: SectionResult,
    activity: SectionResult,
}

impl SectionResult {
    fn ok(rows: Vec<Value>) -> Self {
        Self { rows, note: None }
    }

    fn err(note: impl Into<String>) -> Self {
        Self {
            rows: Vec::new(),
            note: Some(note.into()),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "rows": self.rows,
            "note": self.note,
        })
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct GatherHandler;

#[async_trait::async_trait]
impl CommandHandler for GatherHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let terms_arg = match ctx.args.get("terms").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => {
                return CommandResult::Error {
                    code: "MISSING_ARG".into(),
                    message: "Missing required argument: terms".into(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let budget = ctx
            .options
            .get("budget")
            .and_then(|v| v.as_i64())
            .unwrap_or(8000)
            .max(0) as usize;

        let terms: Vec<String> = terms_arg
            .split_whitespace()
            .filter(|t| t.chars().count() >= 2)
            .map(|t| t.replace('\'', "''"))
            .collect();

        let (render_engine, repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let claude_dir = resolve_claude_dir(&ctx.options);

        // Run independent data domains concurrently while sharing each domain's
        // expensive loaders. This avoids syncing Codex twice and reading every
        // source file three times for one gather request.
        let mut sections: HashMap<&'static str, SectionResult> = std::thread::scope(|scope| {
            let tokenizer = scope.spawn(|| {
                let _ = BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok());
            });
            let handles = [
                scope.spawn(|| {
                    sections_prior_work_and_activity(&claude_dir, &repo_path, &terms, SECTION_LIMIT)
                }),
                scope.spawn(|| vec![("repo_state", section_repo_state(&repo_path))]),
                scope.spawn(|| sections_code(&claude_dir, &repo_path, &terms, SECTION_LIMIT)),
            ];

            let mut collected: HashMap<&'static str, SectionResult> = HashMap::new();
            for handle in handles {
                let results = handle
                    .join()
                    .unwrap_or_else(|_| vec![("unknown", SectionResult::err("section panicked"))]);
                for (name, result) in results {
                    if SECTION_ORDER.contains(&name) {
                        collected.insert(name, result);
                    }
                }
            }
            let _ = tokenizer.join();
            collected
        });

        // Ensure every section is present even if a thread somehow didn't
        // report back (defensive; join() above always inserts on success).
        for name in SECTION_ORDER {
            sections
                .entry(name)
                .or_insert_with(|| SectionResult::err("section did not report a result"));
        }

        enforce_budget(&mut sections, &terms, budget);

        // Materialize each (possibly trimmed) section as `gather_<name>` in
        // the connection used for rendering.
        for name in SECTION_ORDER {
            if let Some(section) = sections.get(name) {
                if let Err(e) = materialize_section(
                    render_engine.conn(),
                    name,
                    &section.rows,
                    section.note.as_deref(),
                ) {
                    // Materialization is best-effort; never fail the bundle over it.
                    eprintln!("gather: failed to materialize gather_{name}: {e}");
                }
            }
        }

        let data = build_data_json(&terms, budget, &sections);

        CommandResult::Ok { data, cta: None }
    }
}

/// Build the response JSON from the current (possibly trimmed) sections.
fn build_data_json(
    terms: &[String],
    budget: usize,
    sections: &HashMap<&'static str, SectionResult>,
) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("terms".to_string(), json!(terms));
    obj.insert("budget".to_string(), json!(budget));
    for name in SECTION_ORDER {
        let value = sections
            .get(name)
            .map(SectionResult::to_json)
            .unwrap_or_else(|| SectionResult::err("missing section").to_json());
        obj.insert(name.to_string(), value);
    }
    Value::Object(obj)
}

/// The BPE table is expensive to build, and budget enforcement calls
/// `count_tokens` once per row dropped -- build it once per process.
static BPE: std::sync::OnceLock<Option<tiktoken_rs::CoreBPE>> = std::sync::OnceLock::new();
const CLI_CTA_TOKEN_RESERVE: usize = 32;

/// Count tokens the same way `--format json` will render the bundle
/// (`serde_json::to_string_pretty`), using the same BPE incurs uses for its
/// built-in `--token-count`/`--token-limit` flags.
fn count_tokens(value: &Value) -> usize {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    match BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()) {
        Some(bpe) => bpe.encode_with_special_tokens(&text).len(),
        None => text.split_whitespace().count(),
    }
}

fn count_rendered_tokens(value: &Value) -> usize {
    let mut rendered = value.clone();
    if let Some(object) = rendered.as_object_mut() {
        // Incurs may append a typed result CTA to JSON output. Include its
        // field here; the reserve below covers dynamic CTA content such as a
        // skills-update notice that the command cannot know in advance.
        object.insert("cta".to_string(), Value::Null);
    }
    count_tokens(&rendered)
}

/// Drop lowest-ranked rows (the tail of each section's ranked list) one at a
/// time, round-robin across sections, until the bundle fits the budget or
/// there is nothing left to drop. Never truncates a row's content.
fn enforce_budget(
    sections: &mut HashMap<&'static str, SectionResult>,
    terms: &[String],
    budget: usize,
) {
    let mut cursor = 0usize;
    loop {
        let data = build_data_json(terms, budget, sections);
        if count_rendered_tokens(&data).saturating_add(CLI_CTA_TOKEN_RESERVE) <= budget {
            return;
        }

        let mut trimmed_any = false;
        for _ in 0..SECTION_ORDER.len() {
            let name = SECTION_ORDER[cursor % SECTION_ORDER.len()];
            cursor += 1;
            if let Some(section) = sections.get_mut(name) {
                if section.rows.pop().is_some() {
                    trimmed_any = true;
                    break;
                }
            }
        }

        if !trimmed_any {
            // Nothing left to drop; the skeleton itself exceeds the budget.
            return;
        }
    }
}

/// Materialize a section's rows as `gather_<name>` in the given connection.
/// Columns are inferred from the union of keys seen across rows; missing
/// keys are stored as NULL. Errored/empty sections get a `note` column.
fn materialize_section(
    conn: &Connection,
    name: &str,
    rows: &[Value],
    note: Option<&str>,
) -> rusqlite::Result<()> {
    let table = format!("gather_{name}");
    conn.execute(&format!("DROP TABLE IF EXISTS \"{table}\""), [])?;

    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }

    if columns.is_empty() {
        conn.execute(&format!("CREATE TABLE \"{table}\" (note TEXT)"), [])?;
        if let Some(n) = note {
            conn.execute(&format!("INSERT INTO \"{table}\" (note) VALUES (?1)"), [n])?;
        }
        return Ok(());
    }

    let col_defs = columns
        .iter()
        .map(|c| format!("\"{c}\" TEXT"))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(&format!("CREATE TABLE \"{table}\" ({col_defs})"), [])?;

    let col_list = columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=columns.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})"
    ))?;

    for row in rows {
        let obj = row.as_object();
        let values: Vec<SqlValue> = columns
            .iter()
            .map(|c| {
                obj.and_then(|o| o.get(c))
                    .map(json_to_sql)
                    .unwrap_or(SqlValue::Null)
            })
            .collect();
        stmt.execute(params_from_iter(values))?;
    }

    Ok(())
}

fn json_to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Null
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Path resolution (mirrors the Claude-data-dir half of `engine_from_options`,
// exposed separately so each section thread can build its own engine)
// ---------------------------------------------------------------------------

fn resolve_claude_dir(options: &Value) -> PathBuf {
    match options.get("data_dir").and_then(|v| v.as_str()) {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude"),
    }
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

fn sections_prior_work_and_activity(
    claude_dir: &Path,
    repo_path: &Path,
    terms: &[String],
    limit: i64,
) -> Vec<(&'static str, SectionResult)> {
    if terms.is_empty() {
        return vec![
            ("prior_work", SectionResult::ok(Vec::new())),
            ("activity", SectionResult::ok(Vec::new())),
        ];
    }

    let (prior_work, activity) = std::thread::scope(|scope| {
        let (claude_ready_tx, claude_ready_rx) = std::sync::mpsc::sync_channel(0);
        let (codex_ready_tx, codex_ready_rx) = std::sync::mpsc::sync_channel(0);
        let non_codex = scope.spawn(move || {
            prior_work_non_codex(claude_dir, repo_path, terms, limit, claude_ready_tx)
        });
        let codex = scope
            .spawn(move || prior_work_codex(claude_dir, repo_path, terms, limit, codex_ready_tx));
        let activity = scope.spawn(move || {
            let claude_cache_ready = claude_ready_rx.recv().is_ok();
            let codex_cache_ready = codex_ready_rx.recv().is_ok();
            let mut engine =
                match UnifiedEngine::new(claude_dir.to_path_buf(), repo_path.to_path_buf()) {
                    Ok(engine) => engine,
                    Err(error) => {
                        return SectionResult::err(format!("engine init failed: {error}"));
                    }
                };
            if claude_cache_ready {
                if let Err(error) = engine.load_current_claude_tool_calls() {
                    return SectionResult::err(format!("failed to load command events: {error}"));
                }
            }
            if codex_cache_ready {
                if let Err(error) = engine.load_current_codex_tables() {
                    return SectionResult::err(format!("failed to load command events: {error}"));
                }
            }
            section_activity(&mut engine, terms, limit)
        });

        let non_codex = non_codex
            .join()
            .unwrap_or_else(|_| Err("prior-work source panicked".to_string()));
        let codex = codex
            .join()
            .unwrap_or_else(|_| Err("Codex prior-work source panicked".to_string()));
        let activity = activity
            .join()
            .unwrap_or_else(|_| SectionResult::err("activity source panicked"));

        let prior_work = match (non_codex, codex) {
            (Ok((sessions, commits, prompts)), Ok(codex)) => SectionResult::ok(
                merge_prior_work_rows(sessions, codex, commits, prompts, limit),
            ),
            (Err(error), _) | (_, Err(error)) => SectionResult::err(error),
        };
        (prior_work, activity)
    });

    vec![("prior_work", prior_work), ("activity", activity)]
}

fn merge_prior_work_rows(
    mut sessions: Vec<Value>,
    codex: Vec<Value>,
    commits: Vec<Value>,
    prompts: Vec<Value>,
    limit: i64,
) -> Vec<Value> {
    sessions.extend(codex);
    sessions.extend(commits);
    sessions.extend(prompts);
    sessions.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa)
    });
    sessions.truncate(limit as usize);
    sessions
}

fn sections_code(
    claude_dir: &Path,
    repo_path: &Path,
    terms: &[String],
    limit: i64,
) -> Vec<(&'static str, SectionResult)> {
    if terms.is_empty() {
        return vec![
            ("code_search", SectionResult::ok(Vec::new())),
            ("symbols", SectionResult::ok(Vec::new())),
            ("excerpts", SectionResult::ok(Vec::new())),
        ];
    }

    let mut engine = match UnifiedEngine::new(claude_dir.to_path_buf(), repo_path.to_path_buf()) {
        Ok(engine) => engine,
        Err(error) => {
            let message = format!("engine init failed: {error}");
            return vec![
                ("code_search", SectionResult::err(message.clone())),
                ("symbols", SectionResult::err(message.clone())),
                ("excerpts", SectionResult::err(message)),
            ];
        }
    };

    let (code_search, excerpts) = match engine.load_code_tables(&["source_lines"]) {
        Ok(()) => (
            section_code_search(&engine, terms, limit),
            section_excerpts(&engine, terms),
        ),
        Err(error) => {
            let message = format!("failed to load source_lines: {error}");
            (
                SectionResult::err(message.clone()),
                SectionResult::err(message),
            )
        }
    };
    let symbols = match engine.load_code_tables(&["symbols"]) {
        Ok(()) => section_symbols(&engine, terms, limit),
        Err(error) => SectionResult::err(format!("failed to load symbols: {error}")),
    };
    vec![
        ("code_search", code_search),
        ("symbols", symbols),
        ("excerpts", excerpts),
    ]
}

/// Load and rank the non-Codex prior-work sources.
type NonCodexPriorWork = (Vec<Value>, Vec<Value>, Vec<Value>);

fn prior_work_non_codex(
    claude_dir: &Path,
    repo_path: &Path,
    terms: &[String],
    limit: i64,
    ready: std::sync::mpsc::SyncSender<()>,
) -> std::result::Result<NonCodexPriorWork, String> {
    let mut engine = UnifiedEngine::new(claude_dir.to_path_buf(), repo_path.to_path_buf())
        .map_err(|error| format!("engine init failed: {error}"))?;
    engine
        .load_claude_tables(&["sessions", "history"])
        .map_err(|error| format!("failed to load claude tables: {error}"))?;
    let _ = ready.send(());
    engine
        .load_git_tables(&["commits"])
        .map_err(|error| format!("failed to load git tables: {error}"))?;

    let sessions_sql = format!(
        "SELECT 'session' AS kind, title AS text, substr(last_timestamp, 1, 10) AS date, \
                {score} AS score \
         FROM sessions WHERE {matches} ORDER BY score DESC, last_timestamp DESC LIMIT {limit}",
        score = score_expr("title", terms),
        matches = match_expr("title", terms),
    );
    let sessions = engine
        .query(&sessions_sql)
        .map_err(|error| format!("sessions query failed: {error}"))?;

    let commit_expr = "(summary || ' ' || coalesce(message, ''))";
    let commits_sql = format!(
        "SELECT 'commit' AS kind, substr(summary, 1, 90) AS text, substr(authored_at, 1, 10) AS date, \
                {score} AS score \
         FROM commits WHERE {matches} ORDER BY score DESC, authored_at DESC LIMIT {limit}",
        score = score_expr(commit_expr, terms),
        matches = match_expr(commit_expr, terms),
    );
    let commits = engine
        .query(&commits_sql)
        .map_err(|error| format!("commits query failed: {error}"))?;

    let prompts_sql = format!(
        "SELECT 'prompt' AS kind, substr(replace(display, char(10), ' '), 1, 110) AS text, \
                date(timestamp / 1000, 'unixepoch') AS date, {score} AS score \
         FROM history WHERE {matches} ORDER BY score DESC, timestamp DESC LIMIT {limit}",
        score = score_expr("display", terms),
        matches = match_expr("display", terms),
    );
    let prompts = engine
        .query(&prompts_sql)
        .map_err(|error| format!("prompts query failed: {error}"))?;

    Ok((sessions, commits, prompts))
}

fn prior_work_codex(
    claude_dir: &Path,
    repo_path: &Path,
    terms: &[String],
    limit: i64,
    ready: std::sync::mpsc::SyncSender<()>,
) -> std::result::Result<Vec<Value>, String> {
    let mut engine = UnifiedEngine::new(claude_dir.to_path_buf(), repo_path.to_path_buf())
        .map_err(|error| format!("engine init failed: {error}"))?;
    engine
        .load_claude_tables(&["codex_threads", "codex_messages"])
        .map_err(|error| format!("failed to load claude tables: {error}"))?;
    let _ = ready.send(());

    let codex_score = score_expr("message.text", terms);
    let codex_sql = format!(
        "WITH recent_messages AS MATERIALIZED (
           SELECT thread_id, timestamp, text
           FROM codex_messages
           WHERE is_canonical = 1
           ORDER BY timestamp DESC
           LIMIT {recent_limit}
         ),
         ranked AS (
           SELECT 'codex_thread' AS kind,
                  substr(replace(message.text, char(10), ' '), 1, 200) AS text,
                  substr(COALESCE(message.timestamp, thread.last_event_at), 1, 10) AS date,
                  {score} AS score,
                  ROW_NUMBER() OVER (
                    PARTITION BY message.thread_id
                    ORDER BY {score} DESC, message.timestamp DESC
                  ) AS rank
           FROM recent_messages AS message
           JOIN codex_threads AS thread
             ON thread.thread_id = message.thread_id
           WHERE {matches}
         )
         SELECT kind, text, date, score
         FROM ranked
         WHERE rank = 1
         ORDER BY score DESC, date DESC
         LIMIT {limit}",
        score = codex_score,
        matches = match_expr("message.text", terms),
        recent_limit = RECENT_CODEX_SEARCH_LIMIT,
    );
    let mut rows = engine
        .query(&codex_sql)
        .map_err(|error| format!("codex threads query failed: {error}"))?;
    for row in &mut rows {
        redact_row_field(row, "text");
    }
    Ok(rows)
}

/// 2. repo_state -- branch, ahead/behind, dirty files, working-diff stats,
///    last 10 commits.
fn section_repo_state(repo_path: &Path) -> SectionResult {
    let repo = match git2::Repository::open(repo_path) {
        Ok(r) => r,
        Err(e) => return SectionResult::err(format!("failed to open repo: {e}")),
    };

    let mut rows = Vec::new();

    let branch_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_default();

    let (ahead, behind) = repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .ok()
        .and_then(|branch| branch.upstream().ok())
        .and_then(|upstream| upstream.get().target())
        .and_then(|upstream_oid| {
            let head_oid = repo.head().ok()?.target()?;
            repo.graph_ahead_behind(head_oid, upstream_oid).ok()
        })
        .unwrap_or((0, 0));

    rows.push(json!({
        "kind": "summary",
        "branch": branch_name,
        "ahead": ahead,
        "behind": behind,
    }));

    let mut status_opts = git2::StatusOptions::new();
    status_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    if let Ok(statuses) = repo.statuses(Some(&mut status_opts)) {
        for entry in statuses.iter() {
            rows.push(json!({
                "kind": "dirty_file",
                "path": entry.path().unwrap_or(""),
                "status": format!("{:?}", entry.status()),
            }));
        }
    }

    if let Ok(diff) = repo.diff_index_to_workdir(None, None) {
        if let Ok(stats) = diff.stats() {
            rows.push(json!({
                "kind": "diff_stat",
                "files_changed": stats.files_changed(),
                "insertions": stats.insertions(),
                "deletions": stats.deletions(),
            }));
        }
    }

    if let Ok(mut revwalk) = repo.revwalk() {
        if revwalk.push_head().is_ok() {
            for oid in revwalk.filter_map(|r| r.ok()).take(10) {
                if let Ok(commit) = repo.find_commit(oid) {
                    let id = commit.id().to_string();
                    rows.push(json!({
                        "kind": "commit",
                        "short_id": &id[..7.min(id.len())],
                        "summary": commit.summary().unwrap_or(""),
                    }));
                }
            }
        }
    }

    SectionResult::ok(rows)
}

/// 3. code_search -- term hits from `source_lines`, ranked by match count
///    per file.
fn section_code_search(engine: &UnifiedEngine, terms: &[String], limit: i64) -> SectionResult {
    if terms.is_empty() {
        return SectionResult::ok(Vec::new());
    }

    let sql = format!(
        "SELECT file_path AS path, COUNT(*) AS hits FROM source_lines \
         WHERE {matches} GROUP BY file_path ORDER BY hits DESC LIMIT {limit}",
        matches = match_expr("content", terms),
    );

    match engine.query(&sql) {
        Ok(rows) => SectionResult::ok(rows),
        Err(e) => SectionResult::err(format!("code_search query failed: {e}")),
    }
}

/// 4. symbols -- matching symbols from the regex-based symbols provider.
fn section_symbols(engine: &UnifiedEngine, terms: &[String], limit: i64) -> SectionResult {
    if terms.is_empty() {
        return SectionResult::ok(Vec::new());
    }

    let sql = format!(
        "SELECT file_path, name, kind, line_start AS line, {score} AS score \
         FROM symbols WHERE {matches} ORDER BY score DESC, line_start LIMIT {limit}",
        score = score_expr("name", terms),
        matches = match_expr("name", terms),
    );

    match engine.query(&sql) {
        Ok(rows) => SectionResult::ok(rows),
        Err(e) => SectionResult::err(format!("symbols query failed: {e}")),
    }
}

/// 5. excerpts -- top-5 files by match count, with matched line ranges
///    (+/-3 lines) from `source_lines`.
fn section_excerpts(engine: &UnifiedEngine, terms: &[String]) -> SectionResult {
    if terms.is_empty() {
        return SectionResult::ok(Vec::new());
    }

    let matches = match_expr("content", terms);
    let top_files_sql = format!(
        "SELECT file_path FROM source_lines WHERE {matches} \
         GROUP BY file_path ORDER BY COUNT(*) DESC LIMIT 5"
    );
    let top_files = match engine.query(&top_files_sql) {
        Ok(r) => r,
        Err(e) => return SectionResult::err(format!("excerpts top-files query failed: {e}")),
    };

    let mut rows = Vec::new();
    for file_row in &top_files {
        let Some(path) = file_row.get("file_path").and_then(|v| v.as_str()) else {
            continue;
        };
        let escaped = path.replace('\'', "''");

        let first_match_sql = format!(
            "SELECT line_number FROM source_lines \
             WHERE file_path = '{escaped}' AND {matches} ORDER BY line_number LIMIT 1"
        );
        let Ok(first_match) = engine.query(&first_match_sql) else {
            continue;
        };
        let Some(line_no) = first_match
            .first()
            .and_then(|r| r.get("line_number"))
            .and_then(|v| v.as_i64())
        else {
            continue;
        };

        let lo = (line_no - 3).max(1);
        let hi = line_no + 3;
        let ctx_sql = format!(
            "SELECT line_number, content FROM source_lines \
             WHERE file_path = '{escaped}' AND line_number BETWEEN {lo} AND {hi} \
             ORDER BY line_number"
        );
        let Ok(ctx_rows) = engine.query(&ctx_sql) else {
            continue;
        };

        let excerpt = ctx_rows
            .iter()
            .map(|r| {
                let n = r.get("line_number").and_then(|v| v.as_i64()).unwrap_or(0);
                let c = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!("{n}: {c}")
            })
            .collect::<Vec<_>>()
            .join("\n");

        rows.push(json!({
            "file_path": path,
            "line": line_no,
            "excerpt": excerpt,
        }));
    }

    SectionResult::ok(rows)
}

/// 6. activity -- open todos, tool calls, and shell commands matching terms.
fn section_activity(engine: &mut UnifiedEngine, terms: &[String], limit: i64) -> SectionResult {
    if terms.is_empty() {
        return SectionResult::ok(Vec::new());
    }

    if let Err(e) = engine.load_claude_tables(&["todos"]) {
        return SectionResult::err(format!("failed to load activity tables: {e}"));
    }
    if let Err(e) = engine.load_command_events() {
        return SectionResult::err(format!("failed to load command events: {e}"));
    }

    let mut rows = Vec::new();

    let todos_sql = format!(
        "SELECT 'todo' AS kind, content AS text, status \
         FROM todos WHERE status != 'completed' AND {matches} LIMIT {limit}",
        matches = match_expr("content", terms),
    );
    match engine.query(&todos_sql) {
        Ok(r) => rows.extend(r),
        Err(e) => return SectionResult::err(format!("todos query failed: {e}")),
    }

    let tools_sql = format!(
        "WITH recent_tools AS MATERIALIZED (
           SELECT tool_name, target
           FROM tool_calls
           ORDER BY timestamp DESC
           LIMIT {recent_limit}
         )
         SELECT 'tool' AS kind, tool_name AS text, target, COUNT(*) AS count \
         FROM recent_tools \
         WHERE tool_name != 'Bash' AND {matches} \
         GROUP BY tool_name, target ORDER BY count DESC LIMIT {limit}",
        matches = match_expr("target", terms),
        recent_limit = RECENT_ACTIVITY_SEARCH_LIMIT,
    );
    match engine.query(&tools_sql) {
        Ok(r) => rows.extend(r),
        Err(e) => return SectionResult::err(format!("tool_calls query failed: {e}")),
    }

    let codex_tool_expr = "(tool_name || ' ' || coalesce(arguments_json, ''))";
    let codex_sql = format!(
        "WITH recent_tools AS MATERIALIZED (
           SELECT tool_name, arguments_json
           FROM codex_tool_executions
           ORDER BY called_at DESC
           LIMIT {recent_limit}
         )
         SELECT 'tool' AS kind, tool_name AS text, \
                substr(arguments_json, 1, 240) AS target, COUNT(*) AS count \
         FROM recent_tools \
         WHERE tool_name NOT IN ('exec_command', 'shell') AND {matches} \
         GROUP BY tool_name, arguments_json ORDER BY count DESC LIMIT {limit}",
        matches = match_expr(codex_tool_expr, terms),
        recent_limit = RECENT_ACTIVITY_SEARCH_LIMIT,
    );
    match engine.query(&codex_sql) {
        Ok(mut codex_rows) => {
            for row in &mut codex_rows {
                redact_row_field(row, "target");
            }
            rows.extend(codex_rows);
        }
        Err(e) => return SectionResult::err(format!("codex_tool_calls query failed: {e}")),
    }

    let command_expr = "(command || ' ' || coalesce(cwd, ''))";
    let agent_sql = format!(
        "WITH claude_commands AS MATERIALIZED (
           SELECT 'claude' AS source, 'agent' AS actor, agent_role, tool_name,
                  command, cwd, timestamp, rowid AS source_order
           FROM tool_calls
           WHERE tool_name = 'Bash' AND command IS NOT NULL
           ORDER BY timestamp DESC
           LIMIT {per_source_limit}
         ),
         codex_commands AS MATERIALIZED (
           SELECT 'codex' AS source, 'agent' AS actor, thread.agent_role,
                  execution.tool_name, execution.cmd AS command,
                  COALESCE(execution.cwd, thread.cwd) AS cwd,
                  execution.called_at AS timestamp,
                  execution.call_record_index AS source_order
           FROM (
             SELECT * FROM codex_tool_executions
             ORDER BY called_at DESC
             LIMIT {codex_scan_limit}
           ) AS execution
           LEFT JOIN codex_threads AS thread
             ON thread.thread_id = execution.thread_id
           WHERE execution.tool_name IN ('exec_command', 'shell')
             AND execution.cmd IS NOT NULL
           ORDER BY execution.called_at DESC
           LIMIT {per_source_limit}
         ),
         recent_commands AS MATERIALIZED (
           SELECT * FROM claude_commands
           UNION ALL
           SELECT * FROM codex_commands
         )
         SELECT 'agent_command' AS kind, source, source AS text, actor, agent_role, \
                tool_name, command, cwd, timestamp, {score} AS score \
         FROM recent_commands \
         WHERE {matches} \
         ORDER BY score DESC, coalesce(timestamp, '') DESC, source_order DESC \
         LIMIT {limit}",
        score = score_expr(command_expr, terms),
        matches = match_expr(command_expr, terms),
        per_source_limit = RECENT_AGENT_COMMANDS_PER_SOURCE,
        codex_scan_limit = RECENT_CODEX_COMMAND_SCAN_LIMIT,
    );
    match engine.query(&agent_sql) {
        Ok(r) => rows.extend(r),
        Err(e) => return SectionResult::err(format!("agent command query failed: {e}")),
    }

    let shell_sql = format!(
        "WITH recent_commands AS MATERIALIZED (
           SELECT source, command, cwd, exit_code, timestamp, source_order
           FROM shell_history
           ORDER BY timestamp DESC
           LIMIT {recent_limit}
         )
         SELECT 'shell_command' AS kind, source, source AS text, command, cwd, exit_code, \
                timestamp, {score} AS score \
         FROM recent_commands \
         WHERE {matches} \
         ORDER BY score DESC, coalesce(timestamp, '') DESC, source_order DESC \
         LIMIT {limit}",
        score = score_expr(command_expr, terms),
        matches = match_expr(command_expr, terms),
        recent_limit = RECENT_COMMAND_SEARCH_LIMIT,
    );
    match engine.query(&shell_sql) {
        Ok(r) => rows.extend(r),
        Err(e) => return SectionResult::err(format!("shell command query failed: {e}")),
    }

    SectionResult::ok(rows)
}

fn redact_row_field(row: &mut Value, field: &str) {
    if let Some(value) = row.get_mut(field) {
        if let Some(text) = value.as_str() {
            *value = Value::String(crate::redaction::redact_sensitive_text(text));
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::typed::<GatherArgs, GatherOptions, (), GatherOutput, _, _>(
        "gather",
        |ctx: TypedContext<GatherArgs, GatherOptions, ()>| async move {
            match legacy_context(ctx) {
                Ok(ctx) => typed_from_result(GatherHandler.run(ctx).await),
                Err(error) => error.into_typed(),
            }
        },
    )
    .description(
        "Run every context-gathering action (prior work, repo state, code search, \
             symbols, excerpts, activity) concurrently and return one token-budgeted bundle",
    )
    .args::<GatherArgs>()
    .options::<GatherOptions>()
    .examples(vec![
        Example {
            command: "recall ranking --json".to_string(),
            description: Some("Gather everything relevant to 'recall ranking'".to_string()),
        },
        Example {
            command: "auth token refresh -r /path/to/repo --budget 4000".to_string(),
            description: Some("Gather with a tighter token budget".to_string()),
        },
    ])
    .hint(
        "Each of the 6 sections (prior_work, repo_state, code_search, symbols, excerpts, \
             activity) is computed on its own connection concurrently, then materialized as a \
             `gather_<section>` table in the connection used to render this response -- a \
             section that errors yields an empty section with a `note` instead of failing the \
             whole bundle. Once the bundle exceeds --budget tokens, the lowest-ranked row is \
             dropped round-robin across sections (never mid-row) until it fits.",
    )
    .mcp(read_only_mcp())
    .done()
}

#[cfg(test)]
mod tests {
    use super::merge_prior_work_rows;
    use serde_json::json;

    #[test]
    fn prior_work_parallel_merge_preserves_original_tie_order() {
        let rows = merge_prior_work_rows(
            vec![json!({"kind": "session", "score": 3})],
            vec![json!({"kind": "codex", "score": 3})],
            vec![json!({"kind": "commit", "score": 3})],
            vec![json!({"kind": "prompt", "score": 3})],
            3,
        );

        let kinds = rows
            .iter()
            .map(|row| row["kind"].as_str().expect("kind"))
            .collect::<Vec<_>>();
        assert_eq!(kinds, ["session", "codex", "commit"]);
    }
}
