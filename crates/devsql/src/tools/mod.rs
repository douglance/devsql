//! Agent tool commands for deterministic code intelligence queries.
//!
//! Each submodule provides a `build()` function that returns a `CommandDef`
//! for use with the incurs CLI framework.

pub mod context;
pub mod diff;
pub mod gather;
pub mod grok;
pub mod history;
pub mod impact;
pub mod recall;
pub mod schema;
pub mod search;
pub mod semantic_diff;
pub mod shared;

use std::path::PathBuf;

use crate::UnifiedEngine;
use incurs::command::{
    CommandContext, McpAnnotations, McpCommandOptions, TypedContext, TypedResult,
};
use incurs::output::CommandResult;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    pub fn into_typed<T>(self) -> TypedResult<T> {
        TypedResult::error(self.code, self.message)
    }
}

impl From<ToolError> for CommandResult {
    fn from(error: ToolError) -> Self {
        CommandResult::Error {
            code: error.code.to_string(),
            message: error.message,
            retryable: false,
            exit_code: Some(1),
            cta: None,
        }
    }
}

pub fn read_only_mcp() -> McpCommandOptions {
    McpCommandOptions {
        annotations: Some(McpAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Like `read_only_mcp`, but honest that the command reaches the network.
///
/// It reads rather than mutates, so `read_only_hint` still holds; only
/// `open_world_hint` differs.
pub fn network_read_mcp() -> McpCommandOptions {
    McpCommandOptions {
        annotations: Some(McpAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Load every table `sql` names into `engine`. macOS logs load only when
/// `macos_logs` is given, because scanning them is bounded by caller options.
pub fn load_query_tables(
    engine: &mut UnifiedEngine,
    sql: &str,
    macos_logs: Option<crate::providers::macos_logs::MacosLogConfig>,
) -> Result<(), ToolError> {
    fn names(tables: &[String]) -> Vec<&str> {
        tables.iter().map(String::as_str).collect()
    }
    fn load_error(what: &str) -> impl FnOnce(crate::Error) -> ToolError + '_ {
        move |e| ToolError {
            code: "LOAD_ERROR",
            message: format!("Failed to load {what} tables: {e}"),
        }
    }

    let needed = crate::engine::detect_tables(sql);
    engine
        .load_claude_tables(&names(&needed.claude))
        .map_err(load_error("Claude"))?;
    engine
        .load_git_tables(&names(&needed.git))
        .map_err(load_error("Git"))?;
    engine
        .load_code_tables(&names(&needed.code))
        .map_err(load_error("code"))?;
    if !needed.shell.is_empty() {
        let loaded = if needed.shell.iter().any(|table| table == "command_events") {
            engine.load_command_events()
        } else {
            engine.load_shell_history()
        };
        loaded.map_err(load_error("command-history"))?;
    }
    engine
        .load_grok_tables(&names(&needed.grok))
        .map_err(load_error("Grok"))?;
    if let Some(config) = macos_logs.filter(|_| needed.system.iter().any(|t| t == "macos_logs")) {
        engine
            .load_macos_logs(config)
            .map_err(load_error("macOS Unified Log"))?;
    }
    Ok(())
}

/// Join a search command's first term with any further unquoted words, so
/// `recall mvc audit` means the same as `recall "mvc audit"`.
pub(crate) fn search_terms(args: &serde_json::Value, first: &str) -> Option<String> {
    let head = args.get(first).and_then(|v| v.as_str())?;
    let mut words = vec![head.to_string()];
    if let Some(more) = args.get("more").and_then(|v| v.as_array()) {
        words.extend(more.iter().filter_map(|v| v.as_str()).map(String::from));
    }
    Some(words.join(" "))
}

pub fn legacy_context<Args: Serialize, Options: Serialize>(
    ctx: TypedContext<Args, Options, ()>,
) -> Result<CommandContext, ToolError> {
    let args = serde_json::to_value(ctx.args).map_err(|error| ToolError {
        code: "SERIALIZATION_ERROR",
        message: error.to_string(),
    })?;
    let options = serde_json::to_value(ctx.options).map_err(|error| ToolError {
        code: "SERIALIZATION_ERROR",
        message: error.to_string(),
    })?;
    Ok(CommandContext {
        agent: ctx.agent,
        args,
        display_name: ctx.display_name,
        env: serde_json::json!({}),
        globals: ctx.globals,
        options,
        request: ctx.request,
        format: ctx.format,
        format_explicit: ctx.format_explicit,
        name: ctx.name,
        vars: ctx.vars,
        version: ctx.version,
    })
}

pub fn typed_from_result<Output>(result: CommandResult) -> TypedResult<Output>
where
    Output: DeserializeOwned + JsonSchema + Serialize,
{
    match result {
        CommandResult::Ok {
            data,
            cta,
            exit_code,
        } => match serde_json::from_value(data) {
            Ok(data) => TypedResult::Ok {
                data,
                cta,
                exit_code,
            },
            Err(error) => TypedResult::error("SERIALIZATION_ERROR", error.to_string()),
        },
        CommandResult::Error {
            code,
            message,
            retryable,
            exit_code,
            cta,
        } => TypedResult::Error {
            code,
            message,
            retryable,
            exit_code,
            cta,
        },
        CommandResult::Stream(_) | CommandResult::RecordStream(_) => TypedResult::error(
            "UNSUPPORTED_STREAM",
            "Typed DevSQL commands do not return streaming results",
        ),
    }
}

pub fn engine_from_options(
    options: &serde_json::Value,
) -> Result<(UnifiedEngine, PathBuf), ToolError> {
    let repo = options
        .get("repo")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    let data_dir = options.get("data_dir").and_then(|value| value.as_str());
    engine_from_paths(repo, data_dir)
}

/// Create an engine from common options (repo, data_dir).
///
/// Returns `(engine, repo_path)` on success, or a `CommandResult::Error` on failure.
pub fn engine_from_paths(
    repo_str: &str,
    data_dir: Option<&str>,
) -> Result<(UnifiedEngine, PathBuf), ToolError> {
    let repo_path = if repo_str == "." {
        std::env::current_dir().map_err(|e| ToolError {
            code: "PATH_ERROR",
            message: format!("Cannot determine current directory: {e}"),
        })?
    } else {
        PathBuf::from(repo_str)
    };

    let claude_dir = match data_dir {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude"),
    };

    let engine = UnifiedEngine::new(claude_dir, repo_path.clone()).map_err(|e| ToolError {
        code: "ENGINE_ERROR",
        message: format!("Failed to create engine: {e}"),
    })?;

    Ok((engine, repo_path))
}
