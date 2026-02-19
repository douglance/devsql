use clap::{Parser, Subcommand};
use std::path::PathBuf;

use ccql::cli::commands;
use ccql::cli::OutputFormat;
use ccql::config::Config;
use ccql::error::Result;
use ccql::models::TodoStatus;

const LONG_ABOUT: &str = r#"SQL query engine for Claude Code and Codex CLI data.

Tables: history, jhistory, codex_history, transcripts, todos  (run 'ccql tables' for schemas)

QUICK START
═══════════════════════════════════════════════════════════════════════════════

  ccql "SELECT display FROM history LIMIT 5"
  ccql "SELECT * FROM todos WHERE status='pending'"
  ccql "SELECT tool_name, COUNT(*) FROM transcripts WHERE type='tool_use' GROUP BY tool_name"

  ccql tables              # Show table schemas
  ccql -f json "..."       # Output as JSON
  ccql --help              # More examples"#;

const AFTER_LONG_HELP: &str = r#"
TABLES
═══════════════════════════════════════════════════════════════════════════════

history        User prompts (display, timestamp, project, pastedContents)
jhistory       Codex CLI prompt history (display, timestamp, session_id, text)
codex_history  Alias of jhistory
transcripts    Logs (_session_id, type, content, tool_name, tool_input, tool_output)
todos          Tasks (_workspace_id, content, status, activeForm)

EXAMPLES
═══════════════════════════════════════════════════════════════════════════════

  ccql "SELECT display FROM history WHERE display LIKE '%error%'"
  ccql "SELECT tool_name, COUNT(*) as n FROM transcripts WHERE type='tool_use' GROUP BY tool_name"
  ccql "SELECT _session_id, COUNT(*) as n FROM transcripts GROUP BY _session_id ORDER BY n DESC LIMIT 5"
  ccql "SELECT status, COUNT(*) FROM todos GROUP BY status"

OUTPUT FORMATS: -f table | json | jsonl | raw

WRITE MODE: --dry-run to preview, --write to execute (auto-backup)"#;

#[derive(Parser)]
#[command(name = "ccql")]
#[command(author = "Claude Code Query")]
#[command(version = "0.1.0")]
#[command(about = "Query Claude Code and Codex CLI data with SQL", long_about = LONG_ABOUT)]
#[command(after_long_help = AFTER_LONG_HELP)]
struct Cli {
    /// SQL query to execute (default command)
    /// Example: "SELECT * FROM history LIMIT 10"
    #[arg(value_name = "QUERY")]
    query: Option<String>,

    /// Path to Claude data directory (default: ~/.claude)
    #[arg(long, env = "CLAUDE_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,

    /// Output format: table, json, jsonl, raw
    #[arg(short, long, value_enum, default_value = "table", global = true)]
    format: OutputFormat,

    /// Enable verbose/debug output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Enable write operations (INSERT, UPDATE, DELETE)
    #[arg(long)]
    write: bool,

    /// Preview what would be modified without making changes
    #[arg(long)]
    dry_run: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute SQL query (explicit)
    #[command(visible_alias = "q")]
    Sql {
        /// SQL query to execute
        query: String,

        /// Enable write operations (INSERT, UPDATE, DELETE)
        #[arg(long)]
        write: bool,

        /// Preview what would be modified without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Extract user prompts with filtering
    Prompts {
        /// Filter by session ID
        #[arg(long)]
        session: Option<String>,

        /// Filter by project path
        #[arg(long)]
        project: Option<String>,

        /// Filter by date range start (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Filter by date range end (YYYY-MM-DD)
        #[arg(long)]
        until: Option<String>,

        /// Limit number of results
        #[arg(short, long)]
        limit: Option<usize>,
    },

    /// Execute jq-style queries on raw data
    Query {
        /// jq-style query expression
        query: String,

        /// Data source: history, jhistory, codex_history, transcripts, stats, todos
        source: String,

        /// Filter by file pattern (for transcripts)
        #[arg(long)]
        file_pattern: Option<String>,
    },

    /// List and browse sessions
    Sessions {
        /// Show detailed session info
        #[arg(short, long)]
        detailed: bool,

        /// Filter by project path
        #[arg(long)]
        project: Option<String>,

        /// Sort by: time, size
        #[arg(long, default_value = "time")]
        sort_by: String,
    },

    /// Display usage statistics
    Stats {
        /// Group by: model, date
        #[arg(long, default_value = "model")]
        group_by: String,

        /// Filter by date range start
        #[arg(long)]
        since: Option<String>,

        /// Filter by date range end
        #[arg(long)]
        until: Option<String>,
    },

    /// Full-text search across all data
    Search {
        /// Search term or regex pattern
        term: String,

        /// Search scope: all, prompts, transcripts
        #[arg(long, default_value = "all")]
        scope: String,

        /// Case-sensitive search
        #[arg(short, long)]
        case_sensitive: bool,

        /// Use regex pattern
        #[arg(short, long)]
        regex: bool,

        /// Lines of context before match
        #[arg(short = 'B', long, default_value = "0")]
        before_context: usize,

        /// Lines of context after match
        #[arg(short = 'A', long, default_value = "0")]
        after_context: usize,
    },

    /// List todos with filtering
    Todos {
        /// Filter by status: pending, in_progress, completed
        #[arg(long)]
        status: Option<String>,

        /// Filter by agent ID
        #[arg(long)]
        agent: Option<String>,
    },

    /// Find repeated/similar prompts
    Duplicates {
        /// Similarity threshold (0.0-1.0)
        #[arg(short, long, default_value = "0.8")]
        threshold: f64,

        /// Minimum count to show
        #[arg(short, long, default_value = "2")]
        min_count: usize,

        /// Maximum clusters to show
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Show variants in each cluster
        #[arg(long)]
        show_variants: bool,

        /// Sort by: count, latest
        #[arg(short, long, default_value = "count")]
        sort: String,

        /// Minimum prompt length in characters
        #[arg(long, default_value = "4")]
        min_length: usize,
    },

    /// Show available tables and their schemas
    Tables,

    /// Show useful query examples
    Examples,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("ccql=debug")
            .init();
    }

    let data_dir = cli
        .data_dir
        .or_else(|| std::env::var("CLAUDE_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(Config::default_data_dir);

    let config = Config::new(data_dir)?;

    // Handle default SQL command when no subcommand is provided
    if let Some(query) = cli.query {
        if cli.command.is_none() {
            return commands::sql(&config, &query, cli.write, cli.dry_run, cli.format).await;
        }
    }

    match cli.command {
        Some(Commands::Sql {
            query,
            write,
            dry_run,
        }) => {
            commands::sql(&config, &query, write, dry_run, cli.format).await?;
        }
        Some(Commands::Prompts {
            session,
            project,
            since,
            until,
            limit,
        }) => {
            commands::prompts(&config, session, project, since, until, limit, cli.format).await?;
        }
        Some(Commands::Query {
            query,
            source,
            file_pattern,
        }) => {
            commands::query(&config, &query, &source, file_pattern, cli.format).await?;
        }
        Some(Commands::Sessions {
            detailed,
            project,
            sort_by,
        }) => {
            commands::sessions(&config, detailed, project, &sort_by, cli.format).await?;
        }
        Some(Commands::Stats {
            group_by,
            since,
            until,
        }) => {
            commands::stats(&config, &group_by, since, until, cli.format).await?;
        }
        Some(Commands::Search {
            term,
            scope,
            case_sensitive,
            regex,
            before_context,
            after_context,
        }) => {
            commands::search(
                &config,
                &term,
                &scope,
                case_sensitive,
                regex,
                before_context,
                after_context,
                cli.format,
            )
            .await?;
        }
        Some(Commands::Todos { status, agent }) => {
            let status = status.and_then(|s| match s.as_str() {
                "pending" => Some(TodoStatus::Pending),
                "in_progress" => Some(TodoStatus::InProgress),
                "completed" => Some(TodoStatus::Completed),
                _ => None,
            });
            commands::todos(&config, status, agent, cli.format).await?;
        }
        Some(Commands::Duplicates {
            threshold,
            min_count,
            limit,
            show_variants,
            sort,
            min_length,
        }) => {
            commands::duplicates(
                &config,
                threshold,
                min_count,
                limit,
                show_variants,
                &sort,
                min_length,
                cli.format,
            )
            .await?;
        }
        Some(Commands::Tables) => {
            print_tables_info(&config);
        }
        Some(Commands::Examples) => {
            print_examples();
        }
        None => {
            // No query and no subcommand - show help
            use clap::CommandFactory;
            Cli::command().print_help()?;
        }
    }

    Ok(())
}

fn print_tables_info(config: &Config) {
    let history_exists = config.history_file().exists();
    let jhistory_exists = config.jhistory_file().exists();
    let transcripts_exists = config.transcripts_dir().exists();
    let todos_exists = config.todos_dir().exists();

    println!("TABLES");
    println!("══════════════════════════════════════════════════════════════════════════════\n");

    // history
    let status = if history_exists { "✓" } else { "✗" };
    println!(
        "{} history                       {}",
        status,
        config.history_file().display()
    );
    println!("  ├── display        TEXT         The prompt text you typed");
    println!("  ├── timestamp      INTEGER      Unix timestamp (milliseconds)");
    println!("  ├── project        TEXT         Project directory path");
    println!("  └── pastedContents OBJECT       Pasted content (JSON)\n");

    // jhistory
    let status = if jhistory_exists { "✓" } else { "✗" };
    println!(
        "{} jhistory                     {}",
        status,
        config.jhistory_file().display()
    );
    println!("  ├── display        TEXT         Prompt text (normalized from text)");
    println!("  ├── timestamp      INTEGER      Unix timestamp (milliseconds)");
    println!("  ├── session_id     TEXT         Codex session id");
    println!("  ├── text           TEXT         Raw prompt text");
    println!("  └── ts             INTEGER      Raw Unix timestamp (seconds)\n");
    println!("  Alias: codex_history\n");

    // transcripts
    let status = if transcripts_exists { "✓" } else { "✗" };
    println!(
        "{} transcripts                   {}",
        status,
        config.transcripts_dir().display()
    );
    println!("  ├── _source_file   TEXT         Source file (ses_xxx.jsonl)");
    println!("  ├── _session_id    TEXT         Session ID");
    println!("  ├── type           TEXT         'user' | 'tool_use' | 'tool_result'");
    println!("  ├── timestamp      TEXT         ISO 8601 timestamp");
    println!("  ├── content        TEXT         Message text (type='user')");
    println!("  ├── tool_name      TEXT         Tool name (type='tool_*')");
    println!("  ├── tool_input     OBJECT       Tool parameters");
    println!("  └── tool_output    OBJECT       Tool response (type='tool_result')\n");

    // todos
    let status = if todos_exists { "✓" } else { "✗" };
    println!(
        "{} todos                         {}",
        status,
        config.todos_dir().display()
    );
    println!("  ├── _source_file   TEXT         Source filename");
    println!("  ├── _workspace_id  TEXT         Workspace ID");
    println!("  ├── _agent_id      TEXT         Agent ID");
    println!("  ├── content        TEXT         Todo description");
    println!("  ├── status         TEXT         'pending' | 'in_progress' | 'completed'");
    println!("  └── activeForm     TEXT         Display text when active\n");

    println!("Run 'ccql examples' for more query examples.");
    println!("\nData directory: {}", config.data_dir.display());
    println!("Codex directory: {}", config.codex_data_dir().display());
}

fn print_examples() {
    println!("FILTER BY CURRENT PROJECT");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  # Only prompts from current folder");
    println!("  ccql \"SELECT display FROM history WHERE project = '$(pwd)' LIMIT 10\"\n");
    println!("  # Transcripts from current project (via session join)");
    println!("  ccql \"SELECT t.tool_name, COUNT(*) as n FROM transcripts t");
    println!("        JOIN history h ON t._session_id = h.session_id");
    println!("        WHERE h.project = '$(pwd)' AND t.type='tool_use'");
    println!("        GROUP BY t.tool_name ORDER BY n DESC\"\n");

    println!("HISTORY QUERIES");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  # Recent prompts");
    println!("  ccql \"SELECT display FROM history ORDER BY timestamp DESC LIMIT 10\"\n");
    println!("  # Search prompts");
    println!("  ccql \"SELECT display FROM history WHERE display LIKE '%error%'\"\n");
    println!("  # Prompts by project");
    println!(
        "  ccql \"SELECT project, COUNT(*) as n FROM history GROUP BY project ORDER BY n DESC\"\n"
    );
    println!("  # Long prompts (likely pasted code)");
    println!("  ccql \"SELECT LENGTH(display) as len, SUBSTR(display, 1, 60) as preview");
    println!("        FROM history ORDER BY len DESC LIMIT 10\"\n");
    println!("  # Recent Codex prompts from jhistory");
    println!("  ccql \"SELECT datetime(timestamp/1000, 'unixepoch') as time, display");
    println!("        FROM jhistory ORDER BY timestamp DESC LIMIT 10\"\n");

    println!("TRANSCRIPT QUERIES");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  # Tool usage stats");
    println!("  ccql \"SELECT tool_name, COUNT(*) as n FROM transcripts");
    println!("        WHERE type='tool_use' GROUP BY tool_name ORDER BY n DESC\"\n");
    println!("  # Most active sessions");
    println!("  ccql \"SELECT _session_id, COUNT(*) as n FROM transcripts");
    println!("        GROUP BY _session_id ORDER BY n DESC LIMIT 10\"\n");
    println!("  # Recent tool calls");
    println!("  ccql \"SELECT tool_name, timestamp FROM transcripts");
    println!("        WHERE type='tool_use' ORDER BY timestamp DESC LIMIT 20\"\n");
    println!("  # All messages in a session");
    println!("  ccql \"SELECT type, SUBSTR(COALESCE(content, tool_name), 1, 50) as preview");
    println!("        FROM transcripts WHERE _session_id='SESSION_ID'\"\n");
    println!("  # Find sessions mentioning a topic");
    println!("  ccql \"SELECT DISTINCT _session_id FROM transcripts");
    println!("        WHERE content LIKE '%authentication%'\"\n");

    println!("TODO QUERIES");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  # Pending todos");
    println!("  ccql \"SELECT content FROM todos WHERE status='pending'\"\n");
    println!("  # Todo counts by status");
    println!("  ccql \"SELECT status, COUNT(*) as n FROM todos GROUP BY status\"\n");
    println!("  # Todos by workspace");
    println!("  ccql \"SELECT _workspace_id, COUNT(*) as n FROM todos");
    println!("        GROUP BY _workspace_id ORDER BY n DESC\"\n");

    println!("OUTPUT FORMATS");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  ccql -f json \"SELECT ...\"     # JSON array");
    println!("  ccql -f jsonl \"SELECT ...\"    # JSON lines (one per row)");
    println!("  ccql -f table \"SELECT ...\"    # Pretty table (default)");
    println!("  ccql -f raw \"SELECT ...\"      # Raw output\n");

    println!("WRITE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════\n");
    println!("  # Preview what would be deleted");
    println!("  ccql --dry-run \"DELETE FROM history WHERE timestamp < 1700000000000\"\n");
    println!("  # Execute deletion (creates backup first)");
    println!("  ccql --write \"DELETE FROM history WHERE timestamp < 1700000000000\"");
}
