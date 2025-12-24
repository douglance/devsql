//! Error types for devsql

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("SQL error: {0}")]
    Sql(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ccql error: {0}")]
    Ccql(String),

    #[error("vcsql error: {0}")]
    Vcsql(String),

    #[error("Query error: {0}")]
    Query(String),
}
