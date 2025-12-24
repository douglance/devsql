//! DevSQL - Unified SQL queries across Claude Code + Git data
//!
//! This crate combines ccql (Claude Code data) and vcsql (Git data) into a
//! unified query interface, enabling cross-database joins to analyze
//! developer productivity patterns.

pub mod engine;
pub mod error;

pub use engine::UnifiedEngine;
pub use error::Error;

/// Result type for devsql operations
pub type Result<T> = std::result::Result<T, Error>;
