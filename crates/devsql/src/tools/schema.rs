//! `devsql schema` -- list the tables a query can use, or one table's columns.

use incurs::command::{CommandDef, Example, TypedContext, TypedResult};
use serde_json::{json, Value};

use super::{engine_from_paths, load_query_tables, read_only_mcp};
use crate::query_help::{explain_error, table_columns, SchemaRequest, TABLES};

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SchemaArgs {
    /// Table to describe; omit to list every table
    table: Option<String>,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SchemaOptions {
    /// Git repository path
    #[incurs(alias = "r", default = ".")]
    repo: String,
    /// Claude data directory (defaults to ~/.claude)
    #[incurs(alias = "d")]
    data_dir: Option<String>,
}

/// Answer a schema question. Listing tables reads nothing; describing one
/// loads that table so its columns are the real ones, not a declared copy.
pub fn answer(
    request: SchemaRequest,
    repo: &str,
    data_dir: Option<&str>,
) -> TypedResult<Vec<Value>> {
    let table = match request {
        SchemaRequest::ListTables => {
            return TypedResult::ok(
                TABLES
                    .iter()
                    .map(|t| json!({ "table": t.name, "source": t.source, "about": t.about }))
                    .collect(),
            );
        }
        SchemaRequest::Describe(table) => table,
    };
    let (mut engine, _) = match engine_from_paths(repo, data_dir) {
        Ok(value) => value,
        Err(error) => return error.into_typed(),
    };
    let probe = format!("SELECT * FROM {table}");
    let macos = Some(crate::providers::macos_logs::MacosLogConfig::default());
    if let Err(error) = load_query_tables(&mut engine, &probe, macos) {
        return error.into_typed();
    }
    let columns = table_columns(engine.conn(), &table);
    if columns.is_empty() {
        let missing = format!("no such table: {table}");
        let message = explain_error(engine.conn(), &probe, &missing).unwrap_or(missing);
        return TypedResult::error("UNKNOWN_TABLE", message);
    }
    TypedResult::ok(
        columns
            .into_iter()
            .map(|(column, kind)| json!({ "table": table, "column": column, "type": kind }))
            .collect(),
    )
}

async fn run(ctx: TypedContext<SchemaArgs, SchemaOptions, ()>) -> TypedResult<Vec<Value>> {
    let request = match ctx.args.table {
        Some(table) => SchemaRequest::Describe(table),
        None => SchemaRequest::ListTables,
    };
    let repo = ctx.options.repo;
    let data_dir = ctx.options.data_dir;
    tokio::task::spawn_blocking(move || answer(request, &repo, data_dir.as_deref()))
        .await
        .unwrap_or_else(|error| TypedResult::error("SCHEMA_TASK_ERROR", error.to_string()))
}

pub fn build() -> CommandDef {
    CommandDef::typed::<SchemaArgs, SchemaOptions, (), Vec<Value>, _, _>("schema", run)
        .description("List queryable tables, or one table's columns")
        .examples(vec![
            Example {
                command: String::new(),
                description: Some("List every table".into()),
            },
            Example {
                command: "commits".into(),
                description: Some("Columns of commits".into()),
            },
        ])
        .mcp(read_only_mcp())
        .done()
}
