//! Agent tool commands for deterministic code intelligence queries.
//!
//! Each submodule provides a `build()` function that returns a `CommandDef`
//! for use with the incurs CLI framework.

pub mod context;
pub mod diff;
pub mod gather;
pub mod history;
pub mod impact;
pub mod recall;
pub mod search;
pub mod semantic_diff;

use std::path::PathBuf;

use crate::UnifiedEngine;
use incurs::output::CommandResult;

/// Create an engine from common options (repo, data_dir).
///
/// Returns `(engine, repo_path)` on success, or a `CommandResult::Error` on failure.
pub fn engine_from_options(
    options: &serde_json::Value,
) -> Result<(UnifiedEngine, PathBuf), CommandResult> {
    let repo_str = options
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let repo_path = if repo_str == "." {
        std::env::current_dir().map_err(|e| CommandResult::Error {
            code: "PATH_ERROR".into(),
            message: format!("Cannot determine current directory: {e}"),
            retryable: false,
            exit_code: Some(1),
            cta: None,
        })?
    } else {
        PathBuf::from(repo_str)
    };

    let claude_dir = match options.get("data_dir").and_then(|v| v.as_str()) {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude"),
    };

    let engine = UnifiedEngine::new(claude_dir, repo_path.clone()).map_err(|e| {
        CommandResult::Error {
            code: "ENGINE_ERROR".into(),
            message: format!("Failed to create engine: {e}"),
            retryable: false,
            exit_code: Some(1),
            cta: None,
        }
    })?;

    Ok((engine, repo_path))
}
