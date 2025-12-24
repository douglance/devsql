use anyhow::{Context, Result};
use clap::Parser;
use std::io;
use std::time::Instant;

use vcsql::cli::{format_output, Args, Command};
use vcsql::git::GitRepo;
use vcsql::sql::engine::SqlEngine;
use vcsql::sql::schema::{get_table_info, get_tables_by_category, TABLES};

fn main() -> Result<()> {
    let args = Args::parse();

    match &args.command {
        Some(Command::Tables) => show_tables(),
        Some(Command::Schema { table }) => show_schema(table.as_deref()),
        Some(Command::Examples) => show_examples(),
        None => {
            if let Some(sql) = &args.sql {
                execute_query(&args, sql)
            } else {
                eprintln!("No query provided. Use 'vcsql --help' for usage.");
                eprintln!("\nQuick start:");
                eprintln!("  vcsql \"SELECT * FROM commits LIMIT 10\"");
                eprintln!("  vcsql tables");
                eprintln!("  vcsql examples");
                std::process::exit(1);
            }
        }
    }
}

fn execute_query(args: &Args, sql: &str) -> Result<()> {
    let start = Instant::now();

    let mut engine = SqlEngine::new()?;

    for repo_path in &args.repo {
        let mut repo = GitRepo::open(repo_path)
            .with_context(|| format!("Failed to open repository: {}", repo_path.display()))?;

        engine
            .load_tables_for_query(sql, &mut repo)
            .with_context(|| "Failed to load tables")?;
    }

    let result = engine
        .execute(sql)
        .with_context(|| "Query execution failed")?;

    let mut stdout = io::stdout().lock();
    format_output(&result, &args.format, args.no_header, &mut stdout)?;

    if args.verbose {
        let elapsed = start.elapsed();
        eprintln!(
            "\n{} row(s) in {:.3}s",
            result.row_count(),
            elapsed.as_secs_f64()
        );
    }

    Ok(())
}

fn show_tables() -> Result<()> {
    println!("\nAvailable tables:\n");

    let categories = get_tables_by_category();
    let category_order = [
        "CORE",
        "REFERENCES",
        "CHANGES",
        "CONFIGURATION",
        "WORKING DIRECTORY",
        "OPERATIONAL",
        "COMPUTED",
    ];

    for category in category_order {
        if let Some(tables) = categories.get(category) {
            println!("  {}", category);
            for table in tables {
                println!("    {:20} {}", table.name, table.description);
            }
            println!();
        }
    }

    println!("Use 'vcsql schema <table>' for column details.");

    Ok(())
}

fn show_schema(table_name: Option<&str>) -> Result<()> {
    match table_name {
        Some(name) => {
            if let Some(info) = get_table_info(name) {
                print_table_schema(info);
            } else {
                eprintln!("Table '{}' not found.", name);
                eprintln!("\nAvailable tables:");
                for t in TABLES {
                    eprintln!("  {}", t.name);
                }
                std::process::exit(1);
            }
        }
        None => {
            for info in TABLES {
                print_table_schema(info);
                println!();
            }
        }
    }

    Ok(())
}

fn print_table_schema(info: &vcsql::sql::TableInfo) {
    println!("\nTABLE: {}", info.name);
    println!("{}", info.description);
    println!("\nCOLUMNS:");

    for col in info.columns {
        let nullable = if col.nullable { "(nullable)" } else { "" };
        println!(
            "  {:20} {:10} {} {}",
            col.name, col.sql_type, col.description, nullable
        );
    }
}

fn show_examples() -> Result<()> {
    println!(
        r#"
VCSQL EXAMPLE QUERIES
====================

BASIC QUERIES
-------------

  # Recent commits
  vcsql "SELECT short_id, summary, authored_at
         FROM commits
         ORDER BY authored_at DESC
         LIMIT 10"

  # Current branch
  vcsql "SELECT name FROM branches WHERE is_head = 1"

  # All branches with their targets
  vcsql "SELECT name, target_id, is_remote FROM branches"

ANALYTICS
---------

  # Commits by author
  vcsql "SELECT author_name, COUNT(*) as commits
         FROM commits
         GROUP BY author_name
         ORDER BY commits DESC"

  # Commits by day of week
  vcsql "SELECT
           CASE CAST(strftime('%w', authored_at) AS INTEGER)
             WHEN 0 THEN 'Sun' WHEN 1 THEN 'Mon'
             WHEN 2 THEN 'Tue' WHEN 3 THEN 'Wed'
             WHEN 4 THEN 'Thu' WHEN 5 THEN 'Fri' WHEN 6 THEN 'Sat'
           END as day,
           COUNT(*) as commits
         FROM commits
         GROUP BY day"

  # Merge commits
  vcsql "SELECT short_id, summary FROM commits WHERE is_merge = 1"

JOINS
-----

  # Commits with branch info
  vcsql "SELECT c.short_id, c.summary, b.name as branch
         FROM commits c
         JOIN branches b ON b.target_id = c.id"

  # Find merge commits with their parents
  vcsql "SELECT c.summary, p.parent_id, p.parent_index
         FROM commits c
         JOIN commit_parents p ON p.commit_id = c.id
         WHERE c.is_merge = 1
         LIMIT 10"

OUTPUT FORMATS
--------------

  # JSON output
  vcsql -f json "SELECT short_id, summary FROM commits LIMIT 3"

  # CSV output
  vcsql -f csv "SELECT * FROM branches" > branches.csv

  # JSONL for streaming
  vcsql -f jsonl "SELECT * FROM commits"
"#
    );

    Ok(())
}
