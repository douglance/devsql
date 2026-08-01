//! `devsql history` -- show Git commit history for a specific file.

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example, TypedContext};
use incurs::output::CommandResult;
use serde_json::{json, Value};

use super::{engine_from_options, legacy_context, read_only_mcp, typed_from_result};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct HistoryArgs {
    /// File path (or partial path) to get history for
    file: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct HistoryOptions {
    /// Git repository path
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Maximum number of commits to return
    #[incurs(alias = "n", default = 20)]
    limit: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct HistoryOutput {
    file_pattern: String,
    total: usize,
    commits: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct HistoryHandler;

#[async_trait::async_trait]
impl CommandHandler for HistoryHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let file = match ctx.args.get("file").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => {
                return CommandResult::Error {
                    code: "MISSING_ARG".into(),
                    message: "Missing required argument: file".into(),
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
            .unwrap_or(20);

        let (mut engine, _repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Load commits and diff_files tables
        if let Err(e) = engine.load_git_tables(&["commits", "diff_files"]) {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load git tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        let sql = format!(
            "SELECT c.short_id, c.author_name, c.authored_at, c.summary, \
                    df.status, df.insertions, df.deletions \
             FROM diff_files df \
             JOIN commits c ON df.commit_id = c.id \
             WHERE df.path LIKE '%{file}%' \
             ORDER BY c.authored_at DESC \
             LIMIT {limit}"
        );

        match engine.query(&sql) {
            Ok(commits) => CommandResult::Ok {
                data: json!({
                    "file_pattern": file,
                    "total": commits.len(),
                    "commits": Value::Array(commits),
                }),
                cta: None,
            },
            Err(e) => CommandResult::Error {
                code: "QUERY_ERROR".into(),
                message: format!("History query failed: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::typed::<HistoryArgs, HistoryOptions, (), HistoryOutput, _, _>(
        "history",
        |ctx: TypedContext<HistoryArgs, HistoryOptions, ()>| async move {
            match legacy_context(ctx) {
                Ok(ctx) => typed_from_result(HistoryHandler.run(ctx).await),
                Err(error) => error.into_typed(),
            }
        },
    )
    .description("Show Git commit history for a specific file")
    .args::<HistoryArgs>()
    .options::<HistoryOptions>()
    .examples(vec![
        Example {
            command: "src/engine.rs --json".to_string(),
            description: Some("Show commit history for engine.rs".to_string()),
        },
        Example {
            command: "Cargo.toml --limit 5 --json".to_string(),
            description: Some("Show last 5 commits touching Cargo.toml".to_string()),
        },
    ])
    .mcp(read_only_mcp())
    .done()
}
