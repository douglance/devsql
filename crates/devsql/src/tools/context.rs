//! `devsql context` -- retrieve file metadata and symbols for a given path.

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example};
use incurs::output::CommandResult;
use serde_json::{json, Value};

use super::engine_from_options;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize)]
#[allow(dead_code)]
struct ContextArgs {
    /// File path (or partial path) to get context for
    file: String,
}

#[derive(incurs::Options, serde::Deserialize)]
#[allow(dead_code)]
struct ContextOptions {
    /// Git repository path
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Include symbol details
    #[incurs(alias = "s")]
    symbols: bool,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct ContextHandler;

#[async_trait::async_trait]
impl CommandHandler for ContextHandler {
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

        let include_symbols = ctx
            .options
            .get("symbols")
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // default to true -- symbols are the main value

        let (mut engine, _repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Load source_files and symbols tables
        let mut tables = vec!["source_files"];
        if include_symbols {
            tables.push("symbols");
        }
        if let Err(e) = engine.load_code_tables(&tables) {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load code tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        // Query source files matching the path
        let files_sql = format!(
            "SELECT path, language, line_count, size_bytes \
             FROM source_files \
             WHERE path LIKE '%{file}%'"
        );

        let file_rows = match engine.query(&files_sql) {
            Ok(rows) => rows,
            Err(e) => {
                return CommandResult::Error {
                    code: "QUERY_ERROR".into(),
                    message: format!("File query failed: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        // For each file, optionally get its symbols
        let mut results = Vec::new();
        for file_row in &file_rows {
            let path = file_row
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let mut entry = file_row.clone();

            if include_symbols {
                let sym_sql = format!(
                    "SELECT name, kind, line_start, line_end, signature, visibility \
                     FROM symbols \
                     WHERE file_path = '{path}' \
                     ORDER BY line_start"
                );

                if let Ok(syms) = engine.query(&sym_sql) {
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("symbols".to_string(), Value::Array(syms));
                    }
                }
            }

            results.push(entry);
        }

        CommandResult::Ok {
            data: json!({
                "file_pattern": file,
                "total": results.len(),
                "files": results,
            }),
            cta: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::build("context", ContextHandler)
        .description("Get file metadata and symbols for a given path")
        .args::<ContextArgs>()
        .options::<ContextOptions>()
        .examples(vec![
            Example {
                command: "src/engine.rs --json".to_string(),
                description: Some("Get context for engine.rs".to_string()),
            },
            Example {
                command: "main.rs --no-symbols --json".to_string(),
                description: Some("Get file info without symbols".to_string()),
            },
        ])
        .done()
}
