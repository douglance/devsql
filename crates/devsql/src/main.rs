//! DevSQL CLI - Unified SQL queries across Claude Code + Git data

use clap::{Parser, ValueEnum};
use devsql::{engine::detect_tables, UnifiedEngine};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "devsql")]
#[command(about = "Query your AI coding history to become a better prompter.\n\nJoin Claude Code conversations with Git commits to find your most productive prompts,\nidentify struggle sessions, and learn what actually works for you.")]
#[command(version)]
#[command(after_help = r#"WHAT YOU CAN DISCOVER:
  • Which prompts led to the most commits
  • When you struggle (many messages, few commits)
  • What tools Claude uses during productive sessions
  • Patterns in your successful coding sessions

EXAMPLE PROMPTS FOR YOUR AI AGENT:
  "Use devsql to find my 10 most effective prompts from the past month"
  "Query my history to find sessions where I struggled—many prompts, few commits"
  "Analyze what my productive days have in common using devsql"

Learn more: https://github.com/douglance/devsql"#)]
struct Cli {
    /// SQL query to execute
    query: Option<String>,

    /// Git repository path (defaults to current directory)
    #[arg(short = 'r', long = "repo", default_value = ".")]
    repo: PathBuf,

    /// Claude data directory (defaults to ~/.claude)
    #[arg(short = 'd', long = "data-dir")]
    data_dir: Option<PathBuf>,

    /// Output format
    #[arg(short = 'f', long = "format", default_value = "table")]
    format: OutputFormat,

    /// Omit header row
    #[arg(short = 'H', long = "no-header")]
    no_header: bool,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Jsonl,
    Csv,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Handle no query - show help
    let query = match cli.query {
        Some(q) => q,
        None => {
            print_help();
            return Ok(());
        }
    };

    // Resolve paths
    let claude_dir = cli
        .data_dir
        .unwrap_or_else(|| dirs::home_dir().unwrap().join(".claude"));

    let repo_path = if cli.repo == PathBuf::from(".") {
        std::env::current_dir()?
    } else {
        cli.repo.clone()
    };

    // Create engine and load tables
    let mut engine = UnifiedEngine::new(claude_dir, repo_path)?;

    // Detect which tables are needed
    let (claude_tables, git_tables) = detect_tables(&query);

    // Load only needed tables
    let claude_refs: Vec<&str> = claude_tables.iter().map(|s| s.as_str()).collect();
    let git_refs: Vec<&str> = git_tables.iter().map(|s| s.as_str()).collect();

    engine.load_claude_tables(&claude_refs)?;
    engine.load_git_tables(&git_refs)?;

    // Execute query
    let results = engine.query(&query)?;

    // Format output
    match cli.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        OutputFormat::Jsonl => {
            for row in &results {
                println!("{}", serde_json::to_string(row)?);
            }
        }
        OutputFormat::Csv => {
            if results.is_empty() {
                return Ok(());
            }
            let headers: Vec<&str> = results[0]
                .as_object()
                .map(|o| o.keys().map(|k| k.as_str()).collect())
                .unwrap_or_default();

            if !cli.no_header {
                println!("{}", headers.join(","));
            }
            for row in &results {
                if let Some(obj) = row.as_object() {
                    let values: Vec<String> = headers
                        .iter()
                        .map(|h| {
                            obj.get(*h)
                                .map(|v| match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                                .unwrap_or_default()
                        })
                        .collect();
                    println!("{}", values.join(","));
                }
            }
        }
        OutputFormat::Table => {
            print_table(&results, !cli.no_header);
        }
    }

    Ok(())
}

fn print_table(results: &[serde_json::Value], show_header: bool) {
    if results.is_empty() {
        println!("No results");
        return;
    }

    let headers: Vec<String> = results[0]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in results {
        if let Some(obj) = row.as_object() {
            for (i, h) in headers.iter().enumerate() {
                let val_len = obj
                    .get(h)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.len(),
                        other => other.to_string().len(),
                    })
                    .unwrap_or(0);
                widths[i] = widths[i].max(val_len).min(50);
            }
        }
    }

    // Print header
    if show_header {
        let header_line: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:width$}", h, width = widths[i]))
            .collect();
        println!("{}", header_line.join(" | "));

        let separator: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        println!("{}", separator.join("-+-"));
    }

    // Print rows
    for row in results {
        if let Some(obj) = row.as_object() {
            let values: Vec<String> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let val = obj
                        .get(h)
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Null => String::new(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default();
                    let truncated = if val.len() > widths[i] {
                        format!("{}...", &val[..widths[i].saturating_sub(3)])
                    } else {
                        val
                    };
                    format!("{:width$}", truncated, width = widths[i])
                })
                .collect();
            println!("{}", values.join(" | "));
        }
    }
}

fn print_help() {
    println!(
        r#"devsql - Query your AI coding history to become a better prompter

Join Claude Code conversations with Git commits to find your most productive
prompts, identify struggle sessions, and learn what actually works for you.

USAGE:
  devsql [OPTIONS] "SQL QUERY"

WHAT YOU CAN DISCOVER:
  • Which prompts led to the most commits
  • When you struggle (many messages, few commits)
  • What tools Claude uses during productive sessions
  • Patterns in your successful coding sessions

TABLES:
  Claude Code:  history (prompts), transcripts (conversations), todos
  Git:          commits, diffs, diff_files, branches

EXAMPLES:

  Find your most productive prompts:
  devsql "SELECT h.message, COUNT(c.id) as commits
    FROM history h
    LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at)
    GROUP BY h.message HAVING commits > 0
    ORDER BY commits DESC LIMIT 10"

  Identify struggle days (many prompts, few commits):
  devsql "SELECT DATE(h.timestamp) as day,
    COUNT(*) as prompts, COUNT(DISTINCT c.id) as commits
    FROM history h
    LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at)
    GROUP BY day ORDER BY prompts DESC LIMIT 10"

OPTIONS:
  -r, --repo PATH       Git repository (default: current directory)
  -d, --data-dir PATH   Claude data (default: ~/.claude)
  -f, --format FORMAT   Output: table, json, jsonl, csv
  -h, --help            Show full help with more examples

TELL YOUR AI AGENT:
  "Use devsql to find my most effective prompts from the past month"
  "Query my history to find when I struggled most"

Learn more: https://github.com/douglance/devsql
"#
    );
}
