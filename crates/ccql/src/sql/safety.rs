//! Safety guards for SQL write operations
//!
//! Provides protection against accidental data loss:
//! - Automatic backups before modifications
//! - Rejection of DELETE/UPDATE without WHERE clause
//! - Dry-run previews of affected data

use crate::config::Config;
use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Safety guard for write operations
pub struct SafetyGuard {
    config: Config,
    backup_enabled: bool,
}

/// Result of a safety check
#[derive(Debug)]
pub enum SafetyCheckResult {
    /// Query is safe to execute
    Safe,
    /// Query is dangerous and should be rejected
    Dangerous(String),
}

impl SafetyGuard {
    /// Create a new safety guard
    pub fn new(config: Config) -> Self {
        Self {
            config,
            backup_enabled: true,
        }
    }

    /// Disable automatic backups (for testing)
    #[allow(dead_code)]
    pub fn disable_backups(&mut self) {
        self.backup_enabled = false;
    }

    /// Check if a SQL statement is dangerous (DELETE/UPDATE without WHERE)
    pub fn check_query(&self, sql: &str) -> SafetyCheckResult {
        let sql_normalized = normalize_sql(sql);

        if is_delete_without_where(&sql_normalized) {
            return SafetyCheckResult::Dangerous(
                "DELETE without WHERE clause would delete all rows. \
                 Use 'DELETE FROM table WHERE 1=1' if you really want to delete everything."
                    .to_string(),
            );
        }

        if is_update_without_where(&sql_normalized) {
            return SafetyCheckResult::Dangerous(
                "UPDATE without WHERE clause would modify all rows. \
                 Use 'UPDATE table SET ... WHERE 1=1' if you really want to update everything."
                    .to_string(),
            );
        }

        if is_truncate(&sql_normalized) {
            return SafetyCheckResult::Dangerous(
                "TRUNCATE would delete all rows. Use DELETE with explicit WHERE clause instead."
                    .to_string(),
            );
        }

        SafetyCheckResult::Safe
    }

    /// Create backups for tables that will be modified by a write operation
    pub fn backup_table(&self, table_name: &str) -> Result<Option<PathBuf>> {
        if !self.backup_enabled {
            return Ok(None);
        }

        let source_path = self.get_table_path(table_name)?;

        if !source_path.exists() {
            return Ok(None);
        }

        let backup_path = create_backup_path(&source_path);

        fs::copy(&source_path, &backup_path).map_err(|e| {
            Error::BackupFailed(format!(
                "Failed to backup {} to {}: {}",
                source_path.display(),
                backup_path.display(),
                e
            ))
        })?;

        Ok(Some(backup_path))
    }

    /// Restore a table from its backup
    #[allow(dead_code)]
    pub fn restore_from_backup(&self, table_name: &str) -> Result<bool> {
        let source_path = self.get_table_path(table_name)?;
        let backup_path = create_backup_path(&source_path);

        if !backup_path.exists() {
            return Ok(false);
        }

        fs::copy(&backup_path, &source_path).map_err(|e| {
            Error::BackupFailed(format!(
                "Failed to restore {} from {}: {}",
                source_path.display(),
                backup_path.display(),
                e
            ))
        })?;

        Ok(true)
    }

    /// Get the file path for a table
    fn get_table_path(&self, table_name: &str) -> Result<PathBuf> {
        match table_name {
            "history" => Ok(self.config.history_file()),
            "stats" => Ok(self.config.stats_file()),
            // Virtual tables (transcripts, todos) are read-only
            // and handled by CompositeStorage
            _ => {
                // For unknown tables, check if JsonStorage has a file
                let jsonl_path = self.config.data_dir.join(format!("{}.jsonl", table_name));
                if jsonl_path.exists() {
                    return Ok(jsonl_path);
                }
                let json_path = self.config.data_dir.join(format!("{}.json", table_name));
                if json_path.exists() {
                    return Ok(json_path);
                }
                Err(Error::Sql(format!(
                    "Cannot determine file path for table: {}",
                    table_name
                )))
            }
        }
    }
}

/// Extract table name from a SQL statement
pub fn extract_table_name(sql: &str) -> Option<String> {
    let sql_normalized = normalize_sql(sql);

    // DELETE FROM table_name
    if let Some(pos) = sql_normalized.find("DELETE FROM ") {
        let rest = &sql_normalized[pos + 12..];
        return extract_identifier(rest);
    }

    // UPDATE table_name SET
    if let Some(pos) = sql_normalized.find("UPDATE ") {
        let rest = &sql_normalized[pos + 7..];
        return extract_identifier(rest);
    }

    // INSERT INTO table_name
    if let Some(pos) = sql_normalized.find("INSERT INTO ") {
        let rest = &sql_normalized[pos + 12..];
        return extract_identifier(rest);
    }

    // TRUNCATE table_name
    if let Some(pos) = sql_normalized.find("TRUNCATE ") {
        let rest = &sql_normalized[pos + 9..];
        return extract_identifier(rest);
    }

    None
}

/// Extract an identifier (table name) from the start of a string
fn extract_identifier(s: &str) -> Option<String> {
    let s = s.trim();
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if end > 0 {
        Some(s[..end].to_lowercase())
    } else {
        None
    }
}

/// Normalize SQL for pattern matching
fn normalize_sql(sql: &str) -> String {
    // Convert to uppercase and collapse whitespace
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// Check if SQL is a DELETE without WHERE
fn is_delete_without_where(sql_normalized: &str) -> bool {
    if !sql_normalized.starts_with("DELETE ") {
        return false;
    }

    !sql_normalized.contains(" WHERE ")
}

/// Check if SQL is an UPDATE without WHERE
fn is_update_without_where(sql_normalized: &str) -> bool {
    if !sql_normalized.starts_with("UPDATE ") {
        return false;
    }

    !sql_normalized.contains(" WHERE ")
}

/// Check if SQL is a TRUNCATE statement
fn is_truncate(sql_normalized: &str) -> bool {
    sql_normalized.starts_with("TRUNCATE ")
}

/// Create a backup path for a file
fn create_backup_path(original: &Path) -> PathBuf {
    let mut backup = original.to_path_buf();
    let extension = backup
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    let new_extension = if extension.is_empty() {
        "bak".to_string()
    } else {
        format!("{}.bak", extension)
    };

    backup.set_extension(new_extension);
    backup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_delete_without_where() {
        let sql = normalize_sql("DELETE FROM history");
        assert!(is_delete_without_where(&sql));

        let sql = normalize_sql("DELETE FROM history WHERE id = 1");
        assert!(!is_delete_without_where(&sql));

        let sql = normalize_sql("  delete from history  ");
        assert!(is_delete_without_where(&sql));
    }

    #[test]
    fn test_is_update_without_where() {
        let sql = normalize_sql("UPDATE history SET status = 'done'");
        assert!(is_update_without_where(&sql));

        let sql = normalize_sql("UPDATE history SET status = 'done' WHERE id = 1");
        assert!(!is_update_without_where(&sql));
    }

    #[test]
    fn test_extract_table_name() {
        assert_eq!(
            extract_table_name("DELETE FROM history WHERE id = 1"),
            Some("history".to_string())
        );
        assert_eq!(
            extract_table_name("UPDATE todos SET status = 'done'"),
            Some("todos".to_string())
        );
        assert_eq!(
            extract_table_name("INSERT INTO history (col) VALUES (1)"),
            Some("history".to_string())
        );
        assert_eq!(extract_table_name("SELECT * FROM foo"), None);
    }

    #[test]
    fn test_create_backup_path() {
        let path = PathBuf::from("/data/history.jsonl");
        let backup = create_backup_path(&path);
        assert_eq!(backup, PathBuf::from("/data/history.jsonl.bak"));

        let path = PathBuf::from("/data/stats.json");
        let backup = create_backup_path(&path);
        assert_eq!(backup, PathBuf::from("/data/stats.json.bak"));
    }
}
