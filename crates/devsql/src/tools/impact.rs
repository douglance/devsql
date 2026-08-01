//! `devsql impact` -- analyze a file's exported symbols and potential dependents.

use incurs::command::{CommandContext, CommandDef, CommandHandler, Example, TypedContext};
use incurs::output::CommandResult;
use serde_json::{json, Value};

use super::{engine_from_options, legacy_context, read_only_mcp, typed_from_result};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct ImpactArgs {
    /// File path (or partial path) to analyze
    file: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct ImpactOptions {
    /// Git repository path
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Depth of dependency analysis (reserved for future use)
    #[incurs(default = 1)]
    depth: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct ImpactOutput {
    file_pattern: String,
    exported_symbols: Vec<Value>,
    potential_dependents: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct ImpactHandler;

#[async_trait::async_trait]
impl CommandHandler for ImpactHandler {
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

        let (mut engine, _repo_path) = match engine_from_options(&ctx.options) {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Load symbols and imports tables
        if let Err(e) = engine.load_code_tables(&["symbols", "imports"]) {
            return CommandResult::Error {
                code: "LOAD_ERROR".into(),
                message: format!("Failed to load code tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }

        // Find exported symbols from the target file
        let exports_sql = format!(
            "SELECT name, kind, line_start, signature, visibility \
             FROM symbols \
             WHERE file_path LIKE '%{file}%' \
               AND (visibility = 'pub' OR visibility = 'public' OR visibility IS NULL) \
             ORDER BY line_start"
        );

        let exported_symbols = match engine.query(&exports_sql) {
            Ok(rows) => rows,
            Err(e) => {
                return CommandResult::Error {
                    code: "QUERY_ERROR".into(),
                    message: format!("Exports query failed: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        // Collect exported symbol names for dependency search
        let symbol_names: Vec<String> = exported_symbols
            .iter()
            .filter_map(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
            .collect();

        // Find potential dependents via imports table
        let mut potential_dependents = Vec::new();
        if !symbol_names.is_empty() {
            // Build a query to find files that import from this module
            let imports_sql = format!(
                "SELECT DISTINCT file_path, source \
                 FROM imports \
                 WHERE source LIKE '%{file}%'"
            );

            if let Ok(import_rows) = engine.query(&imports_sql) {
                for row in import_rows {
                    potential_dependents.push(row);
                }
            }

            // Also search for files that reference exported symbol names
            // via the symbols table (e.g., types used as parameters/return types)
            for name in &symbol_names {
                let refs_sql = format!(
                    "SELECT DISTINCT file_path, name, kind \
                     FROM symbols \
                     WHERE file_path NOT LIKE '%{file}%' \
                       AND (parameters LIKE '%{name}%' OR return_type LIKE '%{name}%') \
                     LIMIT 20"
                );

                if let Ok(ref_rows) = engine.query(&refs_sql) {
                    for row in ref_rows {
                        potential_dependents.push(row);
                    }
                }
            }
        }

        CommandResult::Ok {
            data: json!({
                "file_pattern": file,
                "exported_symbols": Value::Array(exported_symbols),
                "potential_dependents": Value::Array(potential_dependents),
            }),
            cta: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::typed::<ImpactArgs, ImpactOptions, (), ImpactOutput, _, _>(
        "impact",
        |ctx: TypedContext<ImpactArgs, ImpactOptions, ()>| async move {
            match legacy_context(ctx) {
                Ok(ctx) => typed_from_result(ImpactHandler.run(ctx).await),
                Err(error) => error.into_typed(),
            }
        },
    )
    .description("Analyze a file's exported symbols and potential dependents")
    .args::<ImpactArgs>()
    .options::<ImpactOptions>()
    .examples(vec![Example {
        command: "src/engine.rs --json".to_string(),
        description: Some("Analyze impact of changes to engine.rs".to_string()),
    }])
    .mcp(read_only_mcp())
    .done()
}
