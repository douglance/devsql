//! `devsql search` -- find symbols by name across the codebase.

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example, TypedContext};
use incurs::output::CommandResult;
use serde_json::{json, Value};

use super::{engine_from_options, legacy_context, read_only_mcp, typed_from_result};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SearchArgs {
    /// Symbol name or pattern to search for
    query: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SearchOptions {
    /// Git repository path
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Filter by symbol kind (function, struct, enum, trait, impl, etc.)
    #[incurs(alias = "k")]
    kind: Option<String>,
    /// Maximum number of results
    #[incurs(alias = "n", default = 50)]
    limit: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct SearchOutput {
    query: String,
    kind_filter: Option<String>,
    total: usize,
    matches: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct SearchHandler;

#[async_trait::async_trait]
impl CommandHandler for SearchHandler {
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

        let kind = ctx
            .options
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let limit = ctx
            .options
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(50);

        let (mut engine, _repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Load the symbols table
        if let Err(e) = engine.load_code_tables(&["symbols"]) {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load symbols table: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        // Build SQL query
        let sql = if let Some(ref k) = kind {
            format!(
                "SELECT file_path, name, kind, line_start, signature \
                 FROM symbols \
                 WHERE name LIKE '%{query}%' AND kind = '{k}' \
                 LIMIT {limit}"
            )
        } else {
            format!(
                "SELECT file_path, name, kind, line_start, signature \
                 FROM symbols \
                 WHERE name LIKE '%{query}%' \
                 LIMIT {limit}"
            )
        };

        match engine.query(&sql) {
            Ok(matches) => CommandResult::Ok {
                data: json!({
                    "query": query,
                    "kind_filter": kind,
                    "total": matches.len(),
                    "matches": Value::Array(matches),
                }),
                cta: None,
            },
            Err(e) => CommandResult::Error {
                code: "QUERY_ERROR".into(),
                message: format!("Search query failed: {e}"),
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
    CommandDef::typed::<SearchArgs, SearchOptions, (), SearchOutput, _, _>(
        "search",
        |ctx: TypedContext<SearchArgs, SearchOptions, ()>| async move {
            match legacy_context(ctx) {
                Ok(ctx) => typed_from_result(SearchHandler.run(ctx).await),
                Err(error) => error.into_typed(),
            }
        },
    )
    .description("Search for symbols by name across the codebase")
    .args::<SearchArgs>()
    .options::<SearchOptions>()
    .examples(vec![
        Example {
            command: "UnifiedEngine --json".to_string(),
            description: Some("Find all symbols matching 'UnifiedEngine'".to_string()),
        },
        Example {
            command: "load --kind function --json".to_string(),
            description: Some("Find all functions with 'load' in the name".to_string()),
        },
    ])
    .mcp(read_only_mcp())
    .done()
}
