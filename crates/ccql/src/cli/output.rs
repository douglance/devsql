use crate::error::Result;
use comfy_table::{presets::UTF8_FULL_CONDENSED, ContentArrangement, Table};
use serde::Serialize;
use std::io::Write;

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum OutputFormat {
    Json,
    #[default]
    Table,
    Raw,
    Jsonl,
}

pub struct OutputWriter<W: Write> {
    writer: W,
    format: OutputFormat,
}

impl<W: Write> OutputWriter<W> {
    pub fn new(writer: W, format: OutputFormat) -> Self {
        Self { writer, format }
    }

    pub fn write_json<T: Serialize>(&mut self, data: &T) -> Result<()> {
        match self.format {
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(data)?;
                writeln!(self.writer, "{}", json)?;
            }
            OutputFormat::Raw | OutputFormat::Jsonl => {
                let json = serde_json::to_string(data)?;
                writeln!(self.writer, "{}", json)?;
            }
            OutputFormat::Table => {
                let json = serde_json::to_string_pretty(data)?;
                writeln!(self.writer, "{}", json)?;
            }
        }
        Ok(())
    }

    pub fn write_table(&mut self, table: Table) -> Result<()> {
        writeln!(self.writer, "{}", table)?;
        Ok(())
    }

    pub fn writeln(&mut self, text: &str) -> Result<()> {
        writeln!(self.writer, "{}", text)?;
        Ok(())
    }
}

pub fn create_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s.chars().take(max_len - 3).collect::<String>())
    } else {
        s.to_string()
    }
}

pub fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
