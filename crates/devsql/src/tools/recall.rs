//! `devsql recall` -- load prior work relevant to search terms, ranked by
//! how many terms each row matches (then recency).

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example};
use incurs::output::CommandResult;
use serde_json::{json, Value};

use super::engine_from_options;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize)]
#[allow(dead_code)]
struct RecallArgs {
    /// Space-separated terms to recall prior work for
    query: String,
}

#[derive(incurs::Options, serde::Deserialize)]
#[allow(dead_code)]
struct RecallOptions {
    /// Git repository path (scopes the commits source)
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Maximum number of results per source
    #[incurs(alias = "n", default = 8)]
    limit: i64,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct RecallHandler;

/// Build `((E LIKE '%t1%') + (E LIKE '%t2%') + ...)` -- SQLite booleans sum to
/// a match count, so this scores a row by how many terms it hits.
pub(crate) fn score_expr(expr: &str, terms: &[String]) -> String {
    let parts: Vec<String> = terms
        .iter()
        .map(|t| format!("({expr} LIKE '%{t}%')"))
        .collect();
    format!("({})", parts.join(" + "))
}

/// Build `((E LIKE '%t1%') OR (E LIKE '%t2%') OR ...)` -- the match predicate.
pub(crate) fn match_expr(expr: &str, terms: &[String]) -> String {
    let parts: Vec<String> = terms
        .iter()
        .map(|t| format!("({expr} LIKE '%{t}%')"))
        .collect();
    format!("({})", parts.join(" OR "))
}

#[async_trait::async_trait]
impl CommandHandler for RecallHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let query = match ctx.args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => {
                return CommandResult::Error {
                    code: "MISSING_ARG".into(),
                    message: "Missing required argument: query".into(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let limit = ctx
            .options
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(8);

        // Parse terms: split on whitespace, drop tokens < 2 chars, escape quotes.
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|t| t.chars().count() >= 2)
            .map(|t| t.replace('\'', "''"))
            .collect();

        // No usable terms -> empty result, not an error.
        if terms.is_empty() {
            return CommandResult::Ok {
                data: json!({
                    "terms": terms,
                    "sessions": Value::Array(vec![]),
                    "codex_threads": Value::Array(vec![]),
                    "commits": Value::Array(vec![]),
                    "prompts": Value::Array(vec![]),
                    "total": 0,
                }),
                cta: None,
            };
        }

        let (mut engine, _repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if let Err(e) =
            engine.load_claude_tables(&["sessions", "history", "codex_threads", "codex_messages"])
        {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load claude tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        if let Err(e) = engine.load_git_tables(&["commits"]) {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load git tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        // sessions
        let sessions_sql = format!(
            "SELECT title, substr(last_timestamp, 1, 10) AS date, project, \
                    {score} AS score \
             FROM sessions \
             WHERE {matches} \
             ORDER BY score DESC, last_timestamp DESC \
             LIMIT {limit}",
            score = score_expr("title", &terms),
            matches = match_expr("title", &terms),
        );
        let sessions = match engine.query(&sessions_sql) {
            Ok(rows) => rows,
            Err(e) => return query_error("Sessions", e),
        };

        let codex_score = score_expr("message.text", &terms);
        let codex_sql = format!(
            "WITH ranked AS (
               SELECT message.thread_id,
                      substr(COALESCE(message.timestamp, thread.last_event_at), 1, 10) AS date,
                      thread.cwd,
                      message.role,
                      substr(replace(message.text, char(10), ' '), 1, 200) AS excerpt,
                      thread.state,
                      {score} AS score,
                      ROW_NUMBER() OVER (
                        PARTITION BY message.thread_id
                        ORDER BY {score} DESC, message.timestamp DESC
                      ) AS rank
               FROM codex_messages AS message
               JOIN codex_threads AS thread
                 ON thread.thread_id = message.thread_id
               WHERE message.is_canonical = 1 AND {matches}
             )
             SELECT thread_id, date, cwd, role, excerpt, state, score
             FROM ranked
             WHERE rank = 1
             ORDER BY score DESC, date DESC
             LIMIT {limit}",
            score = codex_score,
            matches = match_expr("message.text", &terms),
        );
        let mut codex_threads = match engine.query(&codex_sql) {
            Ok(rows) => rows,
            Err(e) => return query_error("Codex threads", e),
        };
        for thread in &mut codex_threads {
            if let Some(excerpt) = thread.get_mut("excerpt") {
                if let Some(text) = excerpt.as_str() {
                    *excerpt = Value::String(crate::redaction::redact_sensitive_text(text));
                }
            }
        }

        // commits (scoped to repo)
        let commit_expr = "(summary || ' ' || coalesce(message, ''))";
        let commits_sql = format!(
            "SELECT short_id, substr(authored_at, 1, 10) AS date, \
                    substr(summary, 1, 90) AS summary, {score} AS score \
             FROM commits \
             WHERE {matches} \
             ORDER BY score DESC, authored_at DESC \
             LIMIT {limit}",
            score = score_expr(commit_expr, &terms),
            matches = match_expr(commit_expr, &terms),
        );
        let commits = match engine.query(&commits_sql) {
            Ok(rows) => rows,
            Err(e) => return query_error("Commits", e),
        };

        // prompts (history)
        let prompts_sql = format!(
            "SELECT substr(replace(display, char(10), ' '), 1, 110) AS prompt, \
                    date(timestamp / 1000, 'unixepoch') AS date, project, \
                    {score} AS score \
             FROM history \
             WHERE {matches} \
             ORDER BY score DESC, timestamp DESC \
             LIMIT {limit}",
            score = score_expr("display", &terms),
            matches = match_expr("display", &terms),
        );
        let prompts = match engine.query(&prompts_sql) {
            Ok(rows) => rows,
            Err(e) => return query_error("Prompts", e),
        };

        let total = sessions.len() + codex_threads.len() + commits.len() + prompts.len();

        CommandResult::Ok {
            data: json!({
                "terms": terms,
                "sessions": Value::Array(sessions),
                "codex_threads": Value::Array(codex_threads),
                "commits": Value::Array(commits),
                "prompts": Value::Array(prompts),
                "total": total,
            }),
            cta: None,
        }
    }
}

fn query_error(source: &str, e: crate::Error) -> CommandResult {
    CommandResult::Error {
        code: "QUERY_ERROR".into(),
        message: format!("{source} recall query failed: {e}"),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::build("recall", RecallHandler)
        .description(
            "Load prior work (Claude sessions, Codex threads, commits, prompts) relevant to search terms, \
             ranked by term-match count then recency",
        )
        .args::<RecallArgs>()
        .options::<RecallOptions>()
        .examples(vec![
            Example {
                command: "vision simulator mute --json".to_string(),
                description: Some(
                    "Recall prior work about the Vision Pro simulator, ranked".to_string(),
                ),
            },
            Example {
                command: "auth token refresh -r /path/to/repo".to_string(),
                description: Some("Recall prior work, scoping commits to a repo".to_string()),
            },
        ])
        .done()
}
