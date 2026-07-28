//! DevSQL CLI - Unified SQL queries across developer-local data
//!
//! Built on the incurs framework, giving devsql all built-in CLI features:
//! --help, --version, --llms, --llms-full, --mcp, --json, --csv, --table,
//! --format, --filter-output, --verbose, shell completions, and skills.

use std::path::PathBuf;

use devsql::{engine::detect_tables, UnifiedEngine};
use incurs::cli::Cli;
use incurs::command::{CommandContext, CommandDef, CommandHandler, Example};
use incurs::output::{CommandResult, Format};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Schemas (derive macros replace manual FieldMeta construction)
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize)]
#[allow(dead_code)]
struct QueryArgs {
    /// SQL query to execute
    query: String,
}

#[derive(incurs::Options, serde::Deserialize)]
#[allow(dead_code)]
struct QueryOptions {
    /// Git repository path
    #[incur(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incur(alias = "d")]
    data_dir: Option<String>,
    /// Omit header row in table/csv output
    #[incur(alias = "H")]
    no_header: bool,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct SqlHandler;

#[async_trait::async_trait]
impl CommandHandler for SqlHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let query = match ctx.args.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.is_empty() => q.to_string(),
            _ => {
                return CommandResult::Error {
                    code: "MISSING_QUERY".to_string(),
                    message: "No SQL query provided. Run `devsql --help` for usage examples."
                        .to_string(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let repo_str = ctx
            .options
            .get("repo")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let repo_path = if repo_str == "." {
            match std::env::current_dir() {
                Ok(p) => p,
                Err(e) => {
                    return CommandResult::Error {
                        code: "PATH_ERROR".to_string(),
                        message: format!("Cannot determine current directory: {e}"),
                        retryable: false,
                        exit_code: Some(1),
                        cta: None,
                    };
                }
            }
        } else {
            PathBuf::from(repo_str)
        };

        let claude_dir = match ctx.options.get("data_dir").and_then(|v| v.as_str()) {
            Some(d) => PathBuf::from(d),
            None => dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude"),
        };

        let mut engine = match UnifiedEngine::new(claude_dir, repo_path) {
            Ok(e) => e,
            Err(e) => {
                return CommandResult::Error {
                    code: "ENGINE_ERROR".to_string(),
                    message: format!("Failed to create engine: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let (claude_tables, git_tables, code_tables, shell_tables) = detect_tables(&query);
        let claude_refs: Vec<&str> = claude_tables.iter().map(|s| s.as_str()).collect();
        let git_refs: Vec<&str> = git_tables.iter().map(|s| s.as_str()).collect();
        let code_refs: Vec<&str> = code_tables.iter().map(|s| s.as_str()).collect();

        if let Err(e) = engine.load_claude_tables(&claude_refs) {
            return CommandResult::Error {
                code: "LOAD_ERROR".to_string(),
                message: format!("Failed to load Claude tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        if let Err(e) = engine.load_git_tables(&git_refs) {
            return CommandResult::Error {
                code: "LOAD_ERROR".to_string(),
                message: format!("Failed to load Git tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        if let Err(e) = engine.load_code_tables(&code_refs) {
            return CommandResult::Error {
                code: "LOAD_ERROR".to_string(),
                message: format!("Failed to load code tables: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            };
        }
        if !shell_tables.is_empty() {
            if let Err(e) = engine.load_shell_history() {
                return CommandResult::Error {
                    code: "LOAD_ERROR".to_string(),
                    message: format!("Failed to load shell-history tables: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        }

        match engine.query(&query) {
            Ok(results) => CommandResult::Ok {
                data: Value::Array(results),
                cta: None,
            },
            Err(e) => CommandResult::Error {
                code: "QUERY_ERROR".to_string(),
                message: format!("Query failed: {e}"),
                retryable: false,
                exit_code: Some(1),
                cta: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// CLI construction
// ---------------------------------------------------------------------------

fn build_cli() -> Cli {
    Cli::create("devsql")
        .description(
            "Query AI coding, Git, source code, and shell history with SQL.\n\n\
             Join developer-local data to recall prior work and understand how changes were made.",
        )
        .version(env!("CARGO_PKG_VERSION"))
        .format(Format::Table)
        .root(
            CommandDef::build("devsql", SqlHandler)
                .description("Execute a SQL query against developer-local data")
                .args::<QueryArgs>()
                .options::<QueryOptions>()
                .examples(vec![
                    Example {
                        command: r#""SELECT * FROM commits LIMIT 5""#.to_string(),
                        description: Some("List recent commits".to_string()),
                    },
                    Example {
                        command: r#""SELECT h.message, COUNT(c.id) as commits FROM history h LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at) GROUP BY h.message HAVING commits > 0 ORDER BY commits DESC LIMIT 10""#.to_string(),
                        description: Some("Most productive prompts".to_string()),
                    },
                    Example {
                        command: r#""SELECT DATE(h.timestamp) as day, COUNT(*) as prompts, COUNT(DISTINCT c.id) as commits FROM history h LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at) GROUP BY day ORDER BY prompts DESC LIMIT 10""#.to_string(),
                        description: Some("Struggle days".to_string()),
                    },
                    Example {
                        command: r#""SELECT datetime(timestamp/1000, 'unixepoch') as time, display FROM jhistory ORDER BY timestamp DESC LIMIT 10""#.to_string(),
                        description: Some("Recent Codex prompts".to_string()),
                    },
                    Example {
                        command: r#""SELECT source, timestamp, command FROM shell_history ORDER BY timestamp DESC LIMIT 10""#.to_string(),
                        description: Some("Recent Atuin, zsh, and bash commands".to_string()),
                    },
                ])
                .hint(
                    "TABLES:\n  \
                     Claude Code:  history (prompts), transcripts (conversations), sessions (per-session stats), todos\n  \
                     Codex CLI:    jhistory / codex_history (session_id, ts, text, display, timestamp)\n  \
                     Git:          commits, diffs, diff_files, branches\n\n\
                     Shell:        shell_history (Atuin, zsh, bash)\n\n\
                     TELL YOUR AI AGENT:\n  \
                     \"Use devsql to find my most effective prompts from the past month\"\n  \
                     \"Query my history to find when I struggled most\"\n  \
                     \"Analyze what my productive days have in common using devsql\"\n\n\
                     Learn more: https://github.com/douglance/devsql",
                )
                .done(),
        )
        .command("diff", devsql::tools::diff::build())
        .command("search", devsql::tools::search::build())
        .command("context", devsql::tools::context::build())
        .command("history", devsql::tools::history::build())
        .command("impact", devsql::tools::impact::build())
        .command("recall", devsql::tools::recall::build())
        .command("gather", devsql::tools::gather::build())
}

#[tokio::main]
async fn main() {
    let cli = build_cli();
    if let Err(e) = cli.serve().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
