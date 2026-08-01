//! DevSQL - Unified SQL queries across developer-local data
//!
//! This crate combines AI coding history, Git, source code, and shell history
//! in a unified query interface.

mod codex_index;
pub mod engine;
pub mod error;
pub mod providers;
mod redaction;
pub mod tools;
pub mod worklog;

pub use engine::UnifiedEngine;
pub use error::Error;

/// Result type for devsql operations
pub type Result<T> = std::result::Result<T, Error>;
