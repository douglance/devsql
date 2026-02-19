//! Composite storage that merges multiple files as virtual tables
//!
//! Provides unified access to:
//! - Single-file tables (history, stats) via JsonStorage
//! - Virtual tables (jhistory/codex_history, transcripts, todos) via custom scanners

use crate::config::Config;
use async_trait::async_trait;
use futures::stream;
use gluesql::core::ast::{ColumnDef, IndexOperator, OrderByExpr};
use gluesql::core::data::{CustomFunction as StructCustomFunction, Schema};
use gluesql::core::error::Error as GlueError;
use gluesql::core::store::{
    AlterTable, CustomFunction, CustomFunctionMut, DataRow, Index, IndexMut, Metadata, RowIter,
    Store, StoreMut, Transaction,
};
use gluesql::prelude::{Key, Result, Value};
use gluesql_json_storage::JsonStorage;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};

/// Storage that combines JsonStorage with virtual multi-file tables
pub struct CompositeStorage {
    json_storage: JsonStorage,
    config: Config,
}

impl CompositeStorage {
    /// Create a new composite storage
    pub fn new(config: Config) -> Result<Self> {
        let json_storage = JsonStorage::new(&config.data_dir)?;
        Ok(Self {
            json_storage,
            config,
        })
    }

    /// Check if a table is a virtual multi-file table
    fn is_virtual_table(&self, table_name: &str) -> bool {
        matches!(
            table_name,
            "jhistory" | "codex_history" | "transcripts" | "todos"
        )
    }

    /// Scan Codex jhistory and return all rows
    fn scan_jhistory(&self) -> Result<Vec<(Key, DataRow)>> {
        let jhistory_file = self.config.jhistory_file();
        if !jhistory_file.exists() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        let file = fs::File::open(&jhistory_file)
            .map_err(|e| GlueError::StorageMsg(format!("Failed to open jhistory file: {}", e)))?;
        let reader = BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(json) = serde_json::from_str::<JsonValue>(&line) {
                if let Some(data_row) = jhistory_json_to_data_row(&json) {
                    rows.push((Key::I64(row_id), data_row));
                    row_id += 1;
                }
            }
        }

        Ok(rows)
    }

    /// Scan transcripts directory and return all rows
    fn scan_transcripts(&self) -> Result<Vec<(Key, DataRow)>> {
        let transcripts_dir = self.config.transcripts_dir();
        if !transcripts_dir.exists() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        let entries = fs::read_dir(&transcripts_dir)
            .map_err(|e| GlueError::StorageMsg(format!("Failed to read transcripts dir: {}", e)))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let source_file = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let session_id = source_file
                    .strip_prefix("ses_")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                    .unwrap_or(&source_file)
                    .to_string();

                if let Ok(file) = fs::File::open(&path) {
                    let reader = BufReader::new(file);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Ok(json) = serde_json::from_str::<JsonValue>(&line) {
                            let data_row =
                                json_to_data_row_with_meta(&json, &source_file, &session_id);
                            rows.push((Key::I64(row_id), data_row));
                            row_id += 1;
                        }
                    }
                }
            }
        }

        Ok(rows)
    }

    /// Scan todos directory and return all rows
    fn scan_todos(&self) -> Result<Vec<(Key, DataRow)>> {
        let todos_dir = self.config.todos_dir();
        if !todos_dir.exists() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut row_id: i64 = 0;

        let entries = fs::read_dir(&todos_dir)
            .map_err(|e| GlueError::StorageMsg(format!("Failed to read todos dir: {}", e)))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let source_file = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let (workspace_id, agent_id) = parse_todo_filename(&source_file);

                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(json) = serde_json::from_str::<JsonValue>(&content) {
                        match json {
                            JsonValue::Array(items) => {
                                for item in items {
                                    let data_row = todo_json_to_data_row(
                                        &item,
                                        &source_file,
                                        &workspace_id,
                                        &agent_id,
                                    );
                                    rows.push((Key::I64(row_id), data_row));
                                    row_id += 1;
                                }
                            }
                            JsonValue::Object(_) => {
                                let data_row = todo_json_to_data_row(
                                    &json,
                                    &source_file,
                                    &workspace_id,
                                    &agent_id,
                                );
                                rows.push((Key::I64(row_id), data_row));
                                row_id += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(rows)
    }

    /// Create a virtual schema for jhistory table (schemaless)
    fn jhistory_schema(&self) -> Schema {
        self.codex_history_schema_for("jhistory")
    }

    /// Create a virtual schema for codex_history alias table (schemaless)
    fn codex_history_alias_schema(&self) -> Schema {
        self.codex_history_schema_for("codex_history")
    }

    fn codex_history_schema_for(&self, table_name: &str) -> Schema {
        Schema {
            table_name: table_name.to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table for Codex CLI history.jsonl".to_string()),
        }
    }

    /// Create a virtual schema for transcripts table (schemaless)
    fn transcripts_schema(&self) -> Schema {
        Schema {
            table_name: "transcripts".to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table merging all transcript files".to_string()),
        }
    }

    /// Create a virtual schema for todos table (schemaless)
    fn todos_schema(&self) -> Schema {
        Schema {
            table_name: "todos".to_string(),
            column_defs: None, // Schemaless
            indexes: Vec::new(),
            engine: None,
            foreign_keys: Vec::new(),
            comment: Some("Virtual table merging all todo files".to_string()),
        }
    }
}

/// Convert a JSON object to a DataRow with metadata columns
fn json_to_data_row_with_meta(json: &JsonValue, source_file: &str, session_id: &str) -> DataRow {
    let mut map = HashMap::new();

    map.insert(
        "_source_file".to_string(),
        Value::Str(source_file.to_string()),
    );
    map.insert(
        "_session_id".to_string(),
        Value::Str(session_id.to_string()),
    );

    if let JsonValue::Object(obj) = json {
        for (key, value) in obj {
            map.insert(key.clone(), json_value_to_glue_value(value));
        }
    }

    DataRow::Map(map)
}

/// Convert a todo JSON object to a DataRow
fn todo_json_to_data_row(
    json: &JsonValue,
    source_file: &str,
    workspace_id: &str,
    agent_id: &str,
) -> DataRow {
    let mut map = HashMap::new();

    map.insert(
        "_source_file".to_string(),
        Value::Str(source_file.to_string()),
    );
    map.insert(
        "_workspace_id".to_string(),
        Value::Str(workspace_id.to_string()),
    );
    map.insert("_agent_id".to_string(), Value::Str(agent_id.to_string()));

    if let JsonValue::Object(obj) = json {
        for (key, value) in obj {
            map.insert(key.clone(), json_value_to_glue_value(value));
        }
    }

    DataRow::Map(map)
}

/// Convert a codex jhistory JSON object to a normalized DataRow
fn jhistory_json_to_data_row(json: &JsonValue) -> Option<DataRow> {
    let obj = json.as_object()?;

    let text = obj
        .get("text")
        .or_else(|| obj.get("display"))
        .and_then(json_value_as_string)
        .unwrap_or_default();

    let session_id = obj
        .get("session_id")
        .or_else(|| obj.get("sessionId"))
        .and_then(json_value_as_string)
        .unwrap_or_default();

    let ts_seconds = obj
        .get("ts")
        .and_then(json_value_as_i64)
        .or_else(|| {
            obj.get("timestamp")
                .and_then(json_value_as_i64)
                .map(normalize_ts_seconds)
        })
        .unwrap_or(0);

    let timestamp_millis = ts_seconds.saturating_mul(1000);

    let mut map = HashMap::new();
    map.insert("display".to_string(), Value::Str(text.clone()));
    map.insert("timestamp".to_string(), Value::I64(timestamp_millis));
    map.insert("session_id".to_string(), Value::Str(session_id.clone()));
    map.insert("sessionId".to_string(), Value::Str(session_id));
    map.insert("text".to_string(), Value::Str(text));
    map.insert("ts".to_string(), Value::I64(ts_seconds));

    // Preserve any extra fields from codex output.
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "display" | "timestamp" | "session_id" | "sessionId" | "text" | "ts"
        ) {
            continue;
        }
        map.insert(key.clone(), json_value_to_glue_value(value));
    }

    Some(DataRow::Map(map))
}

fn normalize_ts_seconds(raw_ts: i64) -> i64 {
    // Convert epoch milliseconds into seconds when needed.
    if raw_ts > 10_000_000_000 {
        raw_ts / 1000
    } else {
        raw_ts
    }
}

fn json_value_as_i64(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        JsonValue::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn json_value_as_string(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Null => None,
        other => Some(other.to_string()),
    }
}

/// Parse todo filename to extract workspace_id and agent_id
fn parse_todo_filename(filename: &str) -> (String, String) {
    let name = filename.strip_suffix(".json").unwrap_or(filename);

    if let Some(idx) = name.find("-agent-") {
        let workspace_id = name[..idx].to_string();
        let agent_id = name[idx + 7..].to_string();
        (workspace_id, agent_id)
    } else {
        (name.to_string(), "unknown".to_string())
    }
}

/// Convert serde_json Value to GlueSQL Value
fn json_value_to_glue_value(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::I64(i)
            } else if let Some(f) = n.as_f64() {
                Value::F64(f)
            } else {
                Value::Str(n.to_string())
            }
        }
        JsonValue::String(s) => Value::Str(s.clone()),
        JsonValue::Array(arr) => Value::List(arr.iter().map(json_value_to_glue_value).collect()),
        JsonValue::Object(obj) => {
            let map: HashMap<String, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_value_to_glue_value(v)))
                .collect();
            Value::Map(map)
        }
    }
}

/// Convert a vector of rows to a RowIter (pinned boxed stream)
fn rows_to_iter(rows: Vec<(Key, DataRow)>) -> RowIter<'static> {
    let stream = stream::iter(rows.into_iter().map(Ok));
    Box::pin(stream)
}

// Implement the Store trait
#[async_trait(?Send)]
impl Store for CompositeStorage {
    async fn fetch_schema(&self, table_name: &str) -> Result<Option<Schema>> {
        match table_name {
            "jhistory" => Ok(Some(self.jhistory_schema())),
            "codex_history" => Ok(Some(self.codex_history_alias_schema())),
            "transcripts" => Ok(Some(self.transcripts_schema())),
            "todos" => Ok(Some(self.todos_schema())),
            _ => self.json_storage.fetch_schema(table_name).await,
        }
    }

    async fn fetch_all_schemas(&self) -> Result<Vec<Schema>> {
        let mut schemas = self.json_storage.fetch_all_schemas().await?;

        if self.config.jhistory_file().exists() {
            schemas.push(self.jhistory_schema());
            schemas.push(self.codex_history_alias_schema());
        }
        if self.config.transcripts_dir().exists() {
            schemas.push(self.transcripts_schema());
        }
        if self.config.todos_dir().exists() {
            schemas.push(self.todos_schema());
        }

        Ok(schemas)
    }

    async fn fetch_data(&self, table_name: &str, key: &Key) -> Result<Option<DataRow>> {
        if self.is_virtual_table(table_name) {
            let rows = match table_name {
                "jhistory" | "codex_history" => self.scan_jhistory()?,
                "transcripts" => self.scan_transcripts()?,
                "todos" => self.scan_todos()?,
                _ => return Ok(None),
            };

            for (k, row) in rows {
                if &k == key {
                    return Ok(Some(row));
                }
            }
            Ok(None)
        } else {
            self.json_storage.fetch_data(table_name, key).await
        }
    }

    async fn scan_data(&self, table_name: &str) -> Result<RowIter<'_>> {
        if self.is_virtual_table(table_name) {
            let rows = match table_name {
                "jhistory" | "codex_history" => self.scan_jhistory()?,
                "transcripts" => self.scan_transcripts()?,
                "todos" => self.scan_todos()?,
                _ => Vec::new(),
            };

            Ok(rows_to_iter(rows))
        } else {
            self.json_storage.scan_data(table_name).await
        }
    }
}

// Implement StoreMut (delegate to JsonStorage for non-virtual tables)
#[async_trait(?Send)]
impl StoreMut for CompositeStorage {
    async fn insert_schema(&mut self, schema: &Schema) -> Result<()> {
        if self.is_virtual_table(&schema.table_name) {
            Err(GlueError::StorageMsg(
                "Cannot create schema for virtual table".to_string(),
            ))
        } else {
            self.json_storage.insert_schema(schema).await
        }
    }

    async fn delete_schema(&mut self, table_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot delete virtual table schema".to_string(),
            ))
        } else {
            self.json_storage.delete_schema(table_name).await
        }
    }

    async fn append_data(&mut self, table_name: &str, rows: Vec<DataRow>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.append_data(table_name, rows).await
        }
    }

    async fn insert_data(&mut self, table_name: &str, rows: Vec<(Key, DataRow)>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.insert_data(table_name, rows).await
        }
    }

    async fn delete_data(&mut self, table_name: &str, keys: Vec<Key>) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Write operations on virtual multi-file tables not yet supported".to_string(),
            ))
        } else {
            self.json_storage.delete_data(table_name, keys).await
        }
    }
}

// Implement Metadata (delegate to JsonStorage)
#[async_trait(?Send)]
impl Metadata for CompositeStorage {}

// Implement Index (delegate to JsonStorage)
#[async_trait(?Send)]
impl Index for CompositeStorage {
    async fn scan_indexed_data(
        &self,
        table_name: &str,
        index_name: &str,
        asc: Option<bool>,
        cmp_value: Option<(&IndexOperator, Value)>,
    ) -> Result<RowIter<'_>> {
        if self.is_virtual_table(table_name) {
            // Virtual tables don't support indexes, fall back to full scan
            self.scan_data(table_name).await
        } else {
            self.json_storage
                .scan_indexed_data(table_name, index_name, asc, cmp_value)
                .await
        }
    }
}

// Implement IndexMut (delegate to JsonStorage)
#[async_trait(?Send)]
impl IndexMut for CompositeStorage {
    async fn create_index(
        &mut self,
        table_name: &str,
        index_name: &str,
        column: &OrderByExpr,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot create index on virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .create_index(table_name, index_name, column)
                .await
        }
    }

    async fn drop_index(&mut self, table_name: &str, index_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot drop index on virtual table".to_string(),
            ))
        } else {
            self.json_storage.drop_index(table_name, index_name).await
        }
    }
}

// Implement AlterTable (delegate to JsonStorage)
#[async_trait(?Send)]
impl AlterTable for CompositeStorage {
    async fn rename_schema(&mut self, table_name: &str, new_table_name: &str) -> Result<()> {
        if self.is_virtual_table(table_name) || self.is_virtual_table(new_table_name) {
            Err(GlueError::StorageMsg(
                "Cannot rename virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .rename_schema(table_name, new_table_name)
                .await
        }
    }

    async fn rename_column(
        &mut self,
        table_name: &str,
        old_column_name: &str,
        new_column_name: &str,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .rename_column(table_name, old_column_name, new_column_name)
                .await
        }
    }

    async fn add_column(&mut self, table_name: &str, column_def: &ColumnDef) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage.add_column(table_name, column_def).await
        }
    }

    async fn drop_column(
        &mut self,
        table_name: &str,
        column_name: &str,
        if_exists: bool,
    ) -> Result<()> {
        if self.is_virtual_table(table_name) {
            Err(GlueError::StorageMsg(
                "Cannot alter virtual table".to_string(),
            ))
        } else {
            self.json_storage
                .drop_column(table_name, column_name, if_exists)
                .await
        }
    }
}

// Implement Transaction (delegate to JsonStorage)
#[async_trait(?Send)]
impl Transaction for CompositeStorage {
    async fn begin(&mut self, autocommit: bool) -> Result<bool> {
        self.json_storage.begin(autocommit).await
    }

    async fn rollback(&mut self) -> Result<()> {
        self.json_storage.rollback().await
    }

    async fn commit(&mut self) -> Result<()> {
        self.json_storage.commit().await
    }
}

// Implement CustomFunction (delegate to JsonStorage)
#[async_trait(?Send)]
impl CustomFunction for CompositeStorage {
    async fn fetch_function(&self, func_name: &str) -> Result<Option<&StructCustomFunction>> {
        self.json_storage.fetch_function(func_name).await
    }

    async fn fetch_all_functions(&self) -> Result<Vec<&StructCustomFunction>> {
        self.json_storage.fetch_all_functions().await
    }
}

// Implement CustomFunctionMut (delegate to JsonStorage)
#[async_trait(?Send)]
impl CustomFunctionMut for CompositeStorage {
    async fn insert_function(&mut self, func: StructCustomFunction) -> Result<()> {
        self.json_storage.insert_function(func).await
    }

    async fn delete_function(&mut self, func_name: &str) -> Result<()> {
        self.json_storage.delete_function(func_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_todo_filename() {
        let (workspace, agent) = parse_todo_filename("abc123-agent-def456.json");
        assert_eq!(workspace, "abc123");
        assert_eq!(agent, "def456");

        let (workspace, agent) = parse_todo_filename("simple.json");
        assert_eq!(workspace, "simple");
        assert_eq!(agent, "unknown");
    }

    #[test]
    fn test_json_value_to_glue_value() {
        assert_eq!(
            json_value_to_glue_value(&JsonValue::String("test".to_string())),
            Value::Str("test".to_string())
        );
        assert_eq!(
            json_value_to_glue_value(&JsonValue::Bool(true)),
            Value::Bool(true)
        );
        assert_eq!(
            json_value_to_glue_value(&serde_json::json!(42)),
            Value::I64(42)
        );
    }

    #[test]
    fn test_jhistory_json_to_data_row() {
        let json = serde_json::json!({
            "session_id": "abc123",
            "ts": 1754402102,
            "text": "hello codex"
        });

        let Some(DataRow::Map(map)) = jhistory_json_to_data_row(&json) else {
            panic!("expected jhistory row");
        };

        assert_eq!(
            map.get("display"),
            Some(&Value::Str("hello codex".to_string()))
        );
        assert_eq!(map.get("ts"), Some(&Value::I64(1754402102)));
        assert_eq!(map.get("timestamp"), Some(&Value::I64(1_754_402_102_000)));
    }

    #[test]
    fn test_jhistory_json_to_data_row_with_string_numbers() {
        let json = serde_json::json!({
            "session_id": "abc123",
            "ts": "1754402102",
            "text": "hello codex"
        });

        let Some(DataRow::Map(map)) = jhistory_json_to_data_row(&json) else {
            panic!("expected jhistory row");
        };

        assert_eq!(map.get("ts"), Some(&Value::I64(1_754_402_102)));
        assert_eq!(map.get("timestamp"), Some(&Value::I64(1_754_402_102_000)));
    }

    #[test]
    fn test_normalize_ts_seconds() {
        assert_eq!(normalize_ts_seconds(1_754_402_102), 1_754_402_102);
        assert_eq!(normalize_ts_seconds(1_754_402_102_000), 1_754_402_102);
    }
}
