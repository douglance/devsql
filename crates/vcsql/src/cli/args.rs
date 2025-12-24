use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "vcsql",
    about = "SQL query engine for Git repository data",
    version,
    author
)]
pub struct Args {
    /// SQL query to execute
    #[arg(value_name = "SQL")]
    pub sql: Option<String>,

    /// Repository path (can be specified multiple times)
    #[arg(short, long, default_value = ".")]
    pub repo: Vec<PathBuf>,

    /// Output format
    #[arg(short, long, value_enum, default_value = "table")]
    pub format: OutputFormat,

    /// Omit header row (table/csv)
    #[arg(short = 'H', long)]
    pub no_header: bool,

    /// Suppress non-essential output
    #[arg(short, long)]
    pub quiet: bool,

    /// Verbose output (timing, row counts)
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List all available tables
    Tables,

    /// Show table schema(s)
    Schema {
        /// Table name (shows all if omitted)
        table: Option<String>,
    },

    /// Show example queries
    Examples,
}

#[derive(ValueEnum, Clone, Debug, Default)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Jsonl,
    Csv,
}
