//! SQL query engine using GlueSQL with JSON/JSONL storage
//!
//! Provides SQL querying capabilities over Claude Code data files.
//! Supports both single-file tables (history, stats) and multi-file
//! virtual tables (transcripts, todos).

mod composite_storage;
mod safety;

use crate::config::Config;
use crate::error::{Error, Result};
use composite_storage::CompositeStorage;
use gluesql::prelude::*;
use safety::{extract_table_name, SafetyCheckResult, SafetyGuard};
use serde_json::Value as JsonValue;

/// SQL query engine wrapping GlueSQL with CompositeStorage
pub struct SqlEngine {
    glue: Glue<CompositeStorage>,
    config: Config,
    write_enabled: bool,
    safety_guard: SafetyGuard,
}

/// Options for SQL execution
#[derive(Debug, Clone, Default)]
pub struct SqlOptions {
    /// Enable write operations (INSERT, UPDATE, DELETE)
    pub write_enabled: bool,
    /// Dry run mode - show what would be modified without actually modifying
    pub dry_run: bool,
}

impl SqlEngine {
    /// Create a new SQL engine pointing at the Claude data directory
    pub fn new(config: Config, options: SqlOptions) -> Result<Self> {
        let storage = CompositeStorage::new(config.clone())
            .map_err(|e| Error::Sql(format!("Failed to initialize storage: {}", e)))?;

        let glue = Glue::new(storage);
        let safety_guard = SafetyGuard::new(config.clone());

        Ok(Self {
            glue,
            config,
            write_enabled: options.write_enabled,
            safety_guard,
        })
    }

    /// Execute a SQL query and return results as JSON
    pub async fn execute(&mut self, sql: &str) -> Result<Vec<JsonValue>> {
        let is_write = is_write_operation(sql);

        // Check for write operations if not enabled
        if !self.write_enabled && is_write {
            return Err(Error::WriteNotAllowed(
                "Write operations require --write flag. Use --dry-run to preview changes.".into(),
            ));
        }

        // Safety checks for write operations
        if is_write {
            // Check for dangerous operations (DELETE/UPDATE without WHERE)
            match self.safety_guard.check_query(sql) {
                SafetyCheckResult::Safe => {}
                SafetyCheckResult::Dangerous(reason) => {
                    return Err(Error::DangerousOperation(reason));
                }
            }

            // Create backup before modifying data
            if let Some(table_name) = extract_table_name(sql) {
                if let Ok(Some(backup_path)) = self.safety_guard.backup_table(&table_name) {
                    eprintln!("Backup created: {}", backup_path.display());
                }
            }
        }

        let payloads = self
            .glue
            .execute(sql)
            .await
            .map_err(|e| Error::Sql(format!("SQL execution error: {}", e)))?;

        let mut results = Vec::new();

        for payload in payloads {
            match payload {
                Payload::Select { labels, rows } => {
                    for row in rows {
                        let mut obj = serde_json::Map::new();
                        for (label, value) in labels.iter().zip(row.iter()) {
                            obj.insert(label.clone(), glue_value_to_json(value));
                        }
                        results.push(JsonValue::Object(obj));
                    }
                }
                Payload::SelectMap(rows) => {
                    for row in rows {
                        let mut obj = serde_json::Map::new();
                        for (key, value) in row {
                            obj.insert(key, glue_value_to_json(&value));
                        }
                        results.push(JsonValue::Object(obj));
                    }
                }
                Payload::Insert(count) => {
                    results.push(serde_json::json!({
                        "operation": "INSERT",
                        "rows_affected": count
                    }));
                }
                Payload::Update(count) => {
                    results.push(serde_json::json!({
                        "operation": "UPDATE",
                        "rows_affected": count
                    }));
                }
                Payload::Delete(count) => {
                    results.push(serde_json::json!({
                        "operation": "DELETE",
                        "rows_affected": count
                    }));
                }
                Payload::Create => {
                    results.push(serde_json::json!({
                        "operation": "CREATE",
                        "success": true
                    }));
                }
                Payload::DropTable(count) => {
                    results.push(serde_json::json!({
                        "operation": "DROP",
                        "tables_dropped": count
                    }));
                }
                Payload::ShowColumns(columns) => {
                    for col in columns {
                        results.push(serde_json::json!({
                            "column_name": col.0,
                            "column_type": format!("{:?}", col.1)
                        }));
                    }
                }
                _ => {
                    // Other payloads (ShowVariable, etc.)
                    results.push(serde_json::json!({
                        "result": "ok"
                    }));
                }
            }
        }

        Ok(results)
    }

    /// Get available tables (files in the data directory)
    pub fn list_tables(&self) -> Result<Vec<String>> {
        let mut tables = Vec::new();

        // history.jsonl -> history table
        if self.config.history_file().exists() {
            tables.push("history".to_string());
        }

        // stats-cache.json -> stats table (note: renamed to avoid hyphen in SQL)
        if self.config.stats_file().exists() {
            tables.push("stats".to_string());
        }

        // Virtual multi-file tables
        if self.config.transcripts_dir().exists() {
            tables.push("transcripts".to_string());
        }

        if self.config.todos_dir().exists() {
            tables.push("todos".to_string());
        }

        Ok(tables)
    }
}

/// Check if a SQL statement is a write operation
fn is_write_operation(sql: &str) -> bool {
    let sql_upper = sql.trim().to_uppercase();
    sql_upper.starts_with("INSERT")
        || sql_upper.starts_with("UPDATE")
        || sql_upper.starts_with("DELETE")
        || sql_upper.starts_with("DROP")
        || sql_upper.starts_with("CREATE")
        || sql_upper.starts_with("ALTER")
        || sql_upper.starts_with("TRUNCATE")
}

/// Public wrapper for is_write_operation
pub fn is_write_operation_public(sql: &str) -> bool {
    is_write_operation(sql)
}

/// Convert GlueSQL Value to serde_json Value
fn glue_value_to_json(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::I8(n) => JsonValue::Number((*n).into()),
        Value::I16(n) => JsonValue::Number((*n).into()),
        Value::I32(n) => JsonValue::Number((*n).into()),
        Value::I64(n) => JsonValue::Number((*n).into()),
        Value::I128(n) => {
            // serde_json doesn't support i128 directly, convert to string for large values
            if let Ok(n64) = i64::try_from(*n) {
                JsonValue::Number(n64.into())
            } else {
                JsonValue::String(n.to_string())
            }
        }
        Value::U8(n) => JsonValue::Number((*n).into()),
        Value::U16(n) => JsonValue::Number((*n).into()),
        Value::U32(n) => JsonValue::Number((*n).into()),
        Value::U64(n) => JsonValue::Number((*n).into()),
        Value::U128(n) => {
            if let Ok(n64) = u64::try_from(*n) {
                JsonValue::Number(n64.into())
            } else {
                JsonValue::String(n.to_string())
            }
        }
        Value::F32(n) => serde_json::Number::from_f64(*n as f64)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::F64(n) => serde_json::Number::from_f64(*n)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::Str(s) => JsonValue::String(s.clone()),
        Value::Bytea(bytes) => {
            // Encode bytes as base64
            JsonValue::String(base64_encode(bytes))
        }
        Value::Date(d) => JsonValue::String(d.to_string()),
        Value::Time(t) => JsonValue::String(t.to_string()),
        Value::Timestamp(ts) => JsonValue::String(ts.to_string()),
        Value::Interval(i) => JsonValue::String(format!("{:?}", i)),
        Value::Uuid(u) => JsonValue::String(u.to_string()),
        Value::Map(map) => {
            let obj: serde_json::Map<String, JsonValue> = map
                .iter()
                .map(|(k, v)| (k.clone(), glue_value_to_json(v)))
                .collect();
            JsonValue::Object(obj)
        }
        Value::List(list) => {
            JsonValue::Array(list.iter().map(glue_value_to_json).collect())
        }
        Value::Point(p) => serde_json::json!({
            "x": p.x,
            "y": p.y
        }),
        Value::Decimal(d) => JsonValue::String(d.to_string()),
        Value::Inet(addr) => JsonValue::String(addr.to_string()),
    }
}

/// Simple base64 encoding for bytes
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

        result.push(ALPHABET[b0 >> 2] as char);
        result.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[b2 & 0x3f] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_write_operation() {
        assert!(is_write_operation("INSERT INTO foo VALUES (1)"));
        assert!(is_write_operation("  UPDATE foo SET x = 1"));
        assert!(is_write_operation("DELETE FROM foo"));
        assert!(is_write_operation("DROP TABLE foo"));
        assert!(!is_write_operation("SELECT * FROM foo"));
        assert!(!is_write_operation("  select * from foo"));
    }
}
