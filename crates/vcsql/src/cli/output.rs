use crate::cli::OutputFormat;
use crate::error::Result;
use crate::sql::engine::QueryResult;
use serde_json::Value;
use std::io::Write;
use tabled::settings::{Style, Width};

pub fn format_output<W: Write>(
    result: &QueryResult,
    format: &OutputFormat,
    no_header: bool,
    writer: &mut W,
) -> Result<()> {
    match format {
        OutputFormat::Table => format_table(result, no_header, writer),
        OutputFormat::Json => format_json(result, writer),
        OutputFormat::Jsonl => format_jsonl(result, writer),
        OutputFormat::Csv => format_csv(result, no_header, writer),
    }
}

fn format_table<W: Write>(result: &QueryResult, no_header: bool, writer: &mut W) -> Result<()> {
    if result.is_empty() {
        return Ok(());
    }

    let rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(value_to_string).collect())
        .collect();

    let mut builder = tabled::builder::Builder::default();

    if !no_header {
        builder.push_record(&result.columns);
    }

    for row in &rows {
        builder.push_record(row);
    }

    let mut table = builder.build();
    table.with(Style::rounded());

    // Limit column width for readability
    table.with(Width::truncate(80).suffix("..."));

    writeln!(writer, "{}", table)?;

    Ok(())
}

fn format_json<W: Write>(result: &QueryResult, writer: &mut W) -> Result<()> {
    let json_array = result.to_json_array();
    let json_str = serde_json::to_string_pretty(&json_array)?;
    writeln!(writer, "{}", json_str)?;
    Ok(())
}

fn format_jsonl<W: Write>(result: &QueryResult, writer: &mut W) -> Result<()> {
    for row_obj in result.to_json_array() {
        let line = serde_json::to_string(&row_obj)?;
        writeln!(writer, "{}", line)?;
    }
    Ok(())
}

fn format_csv<W: Write>(result: &QueryResult, no_header: bool, writer: &mut W) -> Result<()> {
    let mut csv_writer = csv::Writer::from_writer(writer);

    if !no_header {
        csv_writer.write_record(&result.columns)?;
    }

    for row in &result.rows {
        let string_row: Vec<String> = row.iter().map(value_to_string).collect();
        csv_writer.write_record(&string_row)?;
    }

    csv_writer.flush()?;
    Ok(())
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => serde_json::to_string(arr).unwrap_or_default(),
        Value::Object(obj) => serde_json::to_string(obj).unwrap_or_default(),
    }
}
