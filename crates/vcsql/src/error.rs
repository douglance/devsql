//! Error types for vcsql operations.

use thiserror::Error;

/// Errors that can occur when using vcsql.
#[derive(Error, Debug)]
pub enum VcsqlError {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("SQL error: {0}")]
    Sql(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    #[error("Repository not found: {0}")]
    RepoNotFound(String),

    #[error("Table not found: {0}")]
    TableNotFound(String),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

/// A specialized Result type for vcsql operations.
pub type Result<T> = std::result::Result<T, VcsqlError>;
