//! DevSQL CLI - Unified SQL queries across developer-local data
//!
//! Built on the incurs framework, giving devsql all built-in CLI features:
//! --help, --version, --llms, --llms-full, --mcp, --json, --format,
//! --filter-output, --verbose, shell completions, and skills.

use std::sync::Arc;

use devsql::engine::detect_tables;
use incurs::cli::Cli;
use incurs::command::{CommandDef, Example, TypedContext, TypedResult};
use incurs::output::{CtaBlock, CtaEntry};
use incurs_codemode::{
    CodeMode, CodeModeRunOptions, CodeModeService, ExecutionState, IncurConnector, MemoryStore,
    SearchOutput,
};
use incurs_codemode_local::{LocalCodeModeService, LocalExecutor};
use incurs_extras::{CliExtras, ExtraFormat};
use serde_json::Value;

#[derive(Clone)]
struct DurableCodeModeService(LocalCodeModeService);

fn durable_options(options: CodeModeRunOptions) -> CodeModeRunOptions {
    CodeModeRunOptions {
        request: options.request,
        ..CodeModeRunOptions::default()
    }
}

#[async_trait::async_trait]
impl CodeModeService for DurableCodeModeService {
    async fn search(&self, query: String) -> Result<SearchOutput, String> {
        self.0.search(query).await
    }

    async fn execute(
        &self,
        code: String,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        self.0.execute(code, durable_options(options)).await
    }

    async fn execution(&self, execution_id: String) -> Result<ExecutionState, String> {
        self.0.execution(execution_id).await
    }

    async fn artifact(&self, execution_id: String, artifact_id: String) -> Result<Value, String> {
        self.0.artifact(execution_id, artifact_id).await
    }

    async fn approve(
        &self,
        execution_id: String,
        seq: u64,
        options: CodeModeRunOptions,
    ) -> Result<ExecutionState, String> {
        self.0
            .approve(execution_id, seq, durable_options(options))
            .await
    }

    async fn reject(&self, execution_id: String, seq: u64) -> Result<ExecutionState, String> {
        self.0.reject(execution_id, seq).await
    }

    async fn cancel(&self, execution_id: String) -> Result<ExecutionState, String> {
        self.0.cancel(execution_id).await
    }
}

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
    /// Relative macOS Unified Log window (defaults to 15m)
    log_last: Option<String>,
    /// Absolute macOS Unified Log start time (requires --log-end)
    log_start: Option<String>,
    /// Absolute macOS Unified Log end time (requires --log-start)
    log_end: Option<String>,
    /// Additional macOS Unified Log NSPredicate
    log_predicate: Option<String>,
    /// Read a .logarchive instead of the live macOS log datastore
    log_archive: Option<String>,
    /// macOS Unified Log level: standard, info, or debug
    #[incurs(default = "standard")]
    log_level: String,
    /// Maximum macOS Unified Log records to scan
    #[incurs(default = 50000)]
    log_max_rows: usize,
    /// Maximum macOS Unified Log scan duration in seconds
    #[incurs(default = 30)]
    log_timeout: u64,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

type QueryOutput = Vec<Value>;

async fn run_query(ctx: TypedContext<QueryArgs, QueryOptions, ()>) -> TypedResult<QueryOutput> {
    match tokio::task::spawn_blocking(move || run_query_blocking(ctx)).await {
        Ok(result) => result,
        Err(error) => {
            TypedResult::error("QUERY_TASK_ERROR", format!("Query worker failed: {error}"))
        }
    }
}

fn run_query_blocking(ctx: TypedContext<QueryArgs, QueryOptions, ()>) -> TypedResult<QueryOutput> {
    let query = ctx.args.query;
    let (mut engine, _) = match devsql::tools::engine_from_paths(
        &ctx.options.repo,
        ctx.options.data_dir.as_deref(),
    ) {
        Ok(value) => value,
        Err(error) => return error.into_typed(),
    };

    let needed = detect_tables(&query);
    let (shell_tables, system_tables) = (needed.shell, needed.system);
    let claude_refs: Vec<&str> = needed.claude.iter().map(|s| s.as_str()).collect();
    let git_refs: Vec<&str> = needed.git.iter().map(|s| s.as_str()).collect();
    let code_refs: Vec<&str> = needed.code.iter().map(|s| s.as_str()).collect();
    let work_refs: Vec<&str> = needed.work.iter().map(|s| s.as_str()).collect();
    let grok_refs: Vec<&str> = needed.grok.iter().map(|s| s.as_str()).collect();

    if let Err(e) = engine.load_claude_tables(&claude_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load Claude tables: {e}"));
    }
    if let Err(e) = engine.load_git_tables(&git_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load Git tables: {e}"));
    }
    if let Err(e) = engine.load_code_tables(&code_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load code tables: {e}"));
    }
    if !shell_tables.is_empty() {
        let load_result = if shell_tables.iter().any(|table| table == "command_events") {
            engine.load_command_events()
        } else {
            engine.load_shell_history()
        };
        if let Err(e) = load_result {
            return TypedResult::error(
                "LOAD_ERROR",
                format!("Failed to load command-history tables: {e}"),
            );
        }
    }
    if let Err(e) = engine.load_work_tables(&work_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load work tables: {e}"));
    }
    if let Err(e) = engine.load_grok_tables(&grok_refs) {
        return TypedResult::error("LOAD_ERROR", format!("Failed to load Grok tables: {e}"));
    }
    if system_tables.iter().any(|table| table == "macos_logs") {
        let config = devsql::providers::macos_logs::MacosLogConfig {
            last: ctx.options.log_last,
            start: ctx.options.log_start,
            end: ctx.options.log_end,
            predicate: ctx.options.log_predicate,
            archive: ctx.options.log_archive,
            level: ctx.options.log_level,
            max_rows: ctx.options.log_max_rows,
            timeout: std::time::Duration::from_secs(ctx.options.log_timeout),
        };
        if let Err(error) = engine.load_macos_logs(config) {
            return TypedResult::error(
                "LOAD_ERROR",
                format!("Failed to load macOS Unified Logs: {error}"),
            );
        }
    }

    match engine.query(&query) {
        Ok(results) => {
            if let Some((rows, reason)) = engine.macos_log_truncation() {
                TypedResult::Ok {
                    data: results,
                    cta: Some(CtaBlock {
                        commands: vec![CtaEntry::Detailed {
                            command: "query <SQL> --log-last 5m".to_string(),
                            description: Some(
                                "Rerun with a narrower window or predicate".to_string(),
                            ),
                        }],
                        description: Some(format!(
                            "macos_logs returned a partial scan after {rows} rows: {reason}. Narrow the time window or predicate, or raise the relevant log bound."
                        )),
                    }),
                }
            } else {
                TypedResult::ok(results)
            }
        }
        Err(e) => TypedResult::error("QUERY_ERROR", format!("Query failed: {e}")),
    }
}

fn query_command(name: &str, description: &str) -> CommandDef {
    CommandDef::typed::<QueryArgs, QueryOptions, (), QueryOutput, _, _>(name, run_query)
        .description(description)
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
        Example {
            command: r#""SELECT thread_id, cwd, last_event_at FROM codex_threads ORDER BY last_event_at DESC LIMIT 10""#.to_string(),
            description: Some("Recent Codex conversations".to_string()),
        },
        Example {
            command: r#""SELECT source, timestamp, command FROM shell_history ORDER BY timestamp DESC LIMIT 10""#.to_string(),
            description: Some("Recent Atuin, zsh, and bash commands".to_string()),
        },
        Example {
            command: r#""SELECT timestamp, process, subsystem, category, message FROM macos_logs WHERE process = 'ExampleApp' ORDER BY timestamp DESC LIMIT 100" --log-last 10m --log-level info"#.to_string(),
            description: Some("Recent macOS Unified Log events".to_string()),
        },
    ]
}

fn query_hint() -> &'static str {
    "PRIMARY AGENT INTERFACE:\n  devsql --mcp                 # five-tool Code Mode server\n  codemode_search              # discover devsql.* methods\n  codemode_execute             # run JavaScript across one or more methods\n  codemode_execution           # inspect a durable execution\n  codemode_decide / cancel     # approve writes or stop work\n\n  The direct CLI below is the human and scripting fallback.\n\nTABLES:\n  Claude Code:  history (prompts), transcripts (conversations), sessions (per-session stats), todos\n  Codex CLI:    jhistory / codex_history, codex_threads, codex_messages, codex_events,\n                codex_tool_executions / codex_tool_calls, codex_compactions, codex_ingest_errors\n  Grok Bots:    grok_bots, grok_entries, grok_messages, grok_ingest_errors (global; see `devsql grok`)\n  Git:          commits, diffs, diff_files, branches\n  Shell:        shell_history (Atuin, zsh, bash), command_events (shell + agent commands)\n  macOS:        macos_logs (bounded live or .logarchive stream with provenance)\n  Worklog:      work_tasks, work_events (durable day memory; write via `devsql work`)\n\nWORKDAY MEMORY:\n  devsql work start|update|done|note|list   # agents write structured work events\n  devsql today | day [date] | days          # human day timeline\n\nGROK BOTS (explicit search target, never in recall/gather):\n  devsql grok status | search <terms> | sync\n\nTELL YOUR AI AGENT:\n  \"Use DevSQL Code Mode to find my most effective prompts from the past month\"\n  \"Start a worklog task when beginning non-trivial work\"\n  \"Show me what I did today with DevSQL Code Mode\"\n\nLearn more: https://github.com/douglance/devsql"
}

// ---------------------------------------------------------------------------
// CLI construction
// ---------------------------------------------------------------------------

fn build_cli() -> Cli {
    Cli::create("devsql")
        .description(
            "Code Mode is the primary agent interface for querying AI coding, Git, source code, \
             shell history, macOS Unified Logs, and worklog data.\n\n\
             Run `devsql --mcp` for the Code Mode server. Use the direct CLI for human queries \
             and scripts.",
        )
        .version(env!("CARGO_PKG_VERSION"))
        .default_extra_format(ExtraFormat::Table)
        .root(query_command(
            "devsql",
            "Run a direct SQL query. For agents, prefer the Code Mode server: `devsql --mcp`.",
        ))
        .command(
            "query",
            query_command("query", "Execute a SQL query against developer-local data"),
        )
        .command("diff", devsql::tools::diff::build())
        .command("search", devsql::tools::search::build())
        .command("context", devsql::tools::context::build())
        .command("history", devsql::tools::history::build())
        .command("impact", devsql::tools::impact::build())
        .command("recall", devsql::tools::recall::build())
        .command("gather", devsql::tools::gather::build())
        .group(devsql::tools::work::build_group())
        .group(devsql::tools::grok::build_group())
        .command("today", devsql::tools::day::build_today())
        .command("day", devsql::tools::day::build_day())
        .command("days", devsql::tools::day::build_days())
}

fn code_mode_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    args.next().is_some_and(|arg| arg == "--mcp") && args.next().is_none()
}

async fn serve_code_mode(cli: &Cli) -> Result<(), String> {
    let connector = Arc::new(
        IncurConnector::new(cli.tool_catalog())
            .with_name("devsql")
            .with_instructions(
                "Use devsql.query for cross-source SQL; use devsql.gather when prior work and \
                 repository context should be loaded together. Use devsql.work_start, \
                 devsql.work_update, devsql.work_done, and devsql.work_note to maintain the \
                 durable day timeline.",
            ),
    );
    let service = LocalCodeModeService::spawn(move || {
        CodeMode::new(
            Arc::new(MemoryStore::default()),
            LocalExecutor::default(),
            vec![connector],
        )
    })?;
    incurs_codemode_mcp::serve_stdio(Arc::new(DurableCodeModeService(service))).await
}

#[tokio::main]
async fn main() {
    let cli = build_cli();
    let result = if code_mode_requested() {
        serve_code_mode(&cli)
            .await
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
    } else {
        cli.serve().await
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
