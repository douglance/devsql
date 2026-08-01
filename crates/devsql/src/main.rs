//! DevSQL CLI - Unified SQL queries across Claude/Codex + Git data
//!
//! Built on the incurs framework, giving devsql all built-in CLI features:
//! --help, --version, --llms, --llms-full, --mcp, --json, --format,
//! --filter-output, --verbose, shell completions, and skills.

use devsql::engine::detect_tables;
use incurs::cli::Cli;
use incurs::command::{CommandDef, Example, TypedContext, TypedResult};
use incurs::mcp::{McpDiscovery, McpServeOptions, McpToolFilter};
use incurs_extras::{CliExtras, ExtraFormat};
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
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
    /// Omit header row in table/csv output
    #[incurs(alias = "H")]
    #[serde(default)]
    no_header: bool,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

type QueryOutput = Vec<Value>;

async fn run_query(ctx: TypedContext<QueryArgs, QueryOptions, ()>) -> TypedResult<QueryOutput> {
    let query = ctx.args.query;
    let (mut engine, _) = match devsql::tools::engine_from_paths(
        &ctx.options.repo,
        ctx.options.data_dir.as_deref(),
    ) {
        Ok(value) => value,
        Err(error) => return error.into_typed(),
    };

    let (claude_tables, git_tables, code_tables, work_tables) = detect_tables(&query);
    let claude_refs: Vec<&str> = claude_tables.iter().map(|s| s.as_str()).collect();
    let git_refs: Vec<&str> = git_tables.iter().map(|s| s.as_str()).collect();
    let code_refs: Vec<&str> = code_tables.iter().map(|s| s.as_str()).collect();
    let work_refs: Vec<&str> = work_tables.iter().map(|s| s.as_str()).collect();

    if let Err(e) = engine.load_claude_tables(&claude_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load Claude tables: {e}"));
    }
    if let Err(e) = engine.load_git_tables(&git_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load Git tables: {e}"));
    }
    if let Err(e) = engine.load_code_tables(&code_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load code tables: {e}"));
    }
    if let Err(e) = engine.load_work_tables(&work_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load work tables: {e}"));
    }

    match engine.query(&query) {
        Ok(results) => TypedResult::ok(results),
        Err(e) => TypedResult::error("QUERY_ERROR", format!("Query failed: {e}")),
    }
}

fn query_command(name: &str) -> CommandDef {
    CommandDef::typed::<QueryArgs, QueryOptions, (), QueryOutput, _, _>(name, run_query)
        .description("Execute a SQL query against your Claude/Codex + Git data")
        .examples(query_examples())
        .hint(query_hint())
        .mcp(devsql::tools::read_only_mcp())
        .done()
}

fn query_examples() -> Vec<Example> {
    vec![
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
    ]
}

fn query_hint() -> &'static str {
    "TABLES:\n  Claude Code:  history (prompts), transcripts (conversations), sessions (per-session stats), todos\n  Codex CLI:    jhistory / codex_history (session_id, ts, text, display, timestamp)\n  Git:          commits, diffs, diff_files, branches\n  Worklog:      work_tasks, work_events (durable day memory; write via `devsql work`)\n\nWORKDAY MEMORY:\n  devsql work start|update|done|note|list   # agents write structured work events\n  devsql today | day [date] | days          # human day timeline\n\nTELL YOUR AI AGENT:\n  \"Use devsql to find my most effective prompts from the past month\"\n  \"Start a worklog task when beginning non-trivial work\"\n  \"Show me what I did today with devsql today\"\n\nLearn more: https://github.com/douglance/devsql"
}

// ---------------------------------------------------------------------------
// CLI construction
// ---------------------------------------------------------------------------

fn build_cli() -> Cli {
    Cli::create("devsql")
        .description(
            "Query your AI coding history to become a better prompter.\n\n\
             Join Claude/Codex conversations with Git commits to find your most productive\n\
             prompts, identify struggle sessions, and learn what actually works for you.",
        )
        .version(env!("CARGO_PKG_VERSION"))
        .default_extra_format(ExtraFormat::Table)
        .mcp(McpServeOptions {
            instructions: Some(
                "Use DevSQL to query coding history, Git state, and source-code context. \
                 Use work start/update/done/note to populate the user's cross-project day timeline; \
                 use today/day/days to read it."
                    .to_string(),
            ),
            tools: McpToolFilter {
                discovery: McpDiscovery::Direct,
                ..Default::default()
            },
            ..Default::default()
        })
        .root(query_command("devsql"))
        .command("query", query_command("query"))
        .command("diff", devsql::tools::diff::build())
        .command("search", devsql::tools::search::build())
        .command("context", devsql::tools::context::build())
        .command("history", devsql::tools::history::build())
        .command("impact", devsql::tools::impact::build())
        .command("recall", devsql::tools::recall::build())
        .command("gather", devsql::tools::gather::build())
        .group(devsql::tools::work::build_group())
        .command("today", devsql::tools::day::build_today())
        .command("day", devsql::tools::day::build_day())
        .command("days", devsql::tools::day::build_days())
}

#[tokio::main]
async fn main() {
    let cli = build_cli();
    if let Err(e) = cli.serve().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
