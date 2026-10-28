//! Commands for machine-labeled shared history snapshots.
use crate::UnifiedEngine;
use incurs::cli::Cli;
use incurs::command::{CommandDef, TypedContext, TypedResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(incurs::Args, Deserialize)]
struct ExportArgs {
    /// Destination file; must not already exist.
    path: String,
    /// Stable machine identifier.
    machine: String,
}
#[derive(incurs::Options, Deserialize)]
struct ExportOptions {
    /// Claude data directory.
    data_dir: Option<String>,
    /// Codex data directory.
    codex_dir: Option<String>,
}
#[derive(JsonSchema, Serialize)]
struct ExportOutput {
    path: String,
    machine: String,
}

fn export_command() -> CommandDef {
    CommandDef::typed::<ExportArgs, ExportOptions, (), ExportOutput, _, _>(
        "export",
        |ctx: TypedContext<ExportArgs, ExportOptions, ()>| async move {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let claude = ctx
                .options
                .data_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"));
            let codex = ctx
                .options
                .codex_dir
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
                .unwrap_or_else(|| home.join(".codex"));
            let path = ctx.args.path.clone();
            let machine = ctx.args.machine.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut engine =
                    UnifiedEngine::new_for_export(claude, std::env::current_dir()?, codex)?;
                crate::shared::export(&mut engine, &machine, std::path::Path::new(&path))?;
                Ok::<_, crate::Error>(ExportOutput { path, machine })
            })
            .await;
            match result {
                Ok(Ok(output)) => TypedResult::ok(output),
                Ok(Err(e)) => TypedResult::error("SHARED_EXPORT", e.to_string()),
                Err(e) => TypedResult::error("SHARED_EXPORT", e.to_string()),
            }
        },
    )
    .description("Export this machine's agent and command history into a private snapshot")
    .args::<ExportArgs>()
    .options::<ExportOptions>()
    .done()
}

#[derive(incurs::Args, Deserialize)]
struct QueryArgs {
    /// SQL to execute over all snapshots.
    query: String,
}
#[derive(incurs::Options, Deserialize)]
struct QueryOptions {
    /// Directory containing machine snapshots (for a local host).
    dir: Option<String>,
    /// Tailscale name of the shared host.
    host: Option<String>,
}
fn query_command() -> CommandDef {
    CommandDef::typed::<QueryArgs, QueryOptions, (), Vec<Value>, _, _>(
        "query",
        |ctx: TypedContext<QueryArgs, QueryOptions, ()>| async move {
            let result = tokio::task::spawn_blocking(move || {
                let host = ctx.options.host.or_else(|| {
                    if ctx.options.dir.is_none() {
                        std::env::var("DEVSQL_SHARED_HOST")
                            .ok()
                            .filter(|v| !v.is_empty())
                    } else {
                        None
                    }
                });
                if let Some(host) = host {
                    let value = crate::shared::remote_query(&host, &ctx.args.query)?;
                    return value.as_array().cloned().ok_or_else(|| {
                        crate::Error::Query("unexpected shared host response".into())
                    });
                }
                let dir = ctx.options.dir.map(PathBuf::from).unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".devsql/shared/machines")
                });
                let mut paths = std::fs::read_dir(dir)?
                    .map(|entry| entry.map(|e| e.path()))
                    .collect::<std::io::Result<Vec<_>>>()?;
                paths.retain(|p| p.extension().is_some_and(|e| e == "sqlite"));
                paths.sort();
                crate::shared::query(&paths, &ctx.args.query)
            })
            .await;
            match result {
                Ok(Ok(rows)) => TypedResult::ok(rows),
                Ok(Err(e)) => TypedResult::error("SHARED_QUERY", e.to_string()),
                Err(e) => TypedResult::error("SHARED_QUERY", e.to_string()),
            }
        },
    )
    .description("Run read-only SQL across machine snapshots; each row has _machine")
    .args::<QueryArgs>()
    .options::<QueryOptions>()
    .mcp(super::network_read_mcp())
    .done()
}
#[derive(incurs::Args, Deserialize)]
struct SyncArgs {
    /// Tailscale name of the shared host.
    host: String,
}
#[derive(incurs::Options, Deserialize)]
struct SyncOptions {
    /// Claude data directory.
    data_dir: Option<String>,
    /// Codex data directory.
    codex_dir: Option<String>,
}
#[derive(JsonSchema, Serialize)]
struct SyncOutput {
    machine: String,
    host: String,
}
fn sync_command() -> CommandDef {
    CommandDef::typed::<SyncArgs, SyncOptions, (), SyncOutput, _, _>(
        "sync",
        |ctx: TypedContext<SyncArgs, SyncOptions, ()>| async move {
            let result = tokio::task::spawn_blocking(move || {
                let status = crate::shared::tailscale_status()?;
                let (_, machine) = crate::shared::tailscale_host(&ctx.args.host, &status)?;
                let home = dirs::home_dir()
                    .ok_or_else(|| crate::Error::Query("home directory unavailable".into()))?;
                let temp_dir = home.join(".devsql/shared/outgoing");
                std::fs::create_dir_all(&temp_dir)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&temp_dir, std::fs::Permissions::from_mode(0o700))?;
                }
            // Hold an OS-released lock through publication so an older concurrent
            // export cannot replace a newer snapshot from this machine.
            let lock = rusqlite::Connection::open(temp_dir.join("sync-lock.sqlite"))?;
            lock.execute_batch("BEGIN IMMEDIATE")?;
                let path = temp_dir.join(format!("{}.{}.sqlite", machine, std::process::id()));
                let claude = ctx
                    .options
                    .data_dir
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".claude"));
                let codex = ctx
                    .options
                    .codex_dir
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
                    .unwrap_or_else(|| home.join(".codex"));
                let mut engine =
                    UnifiedEngine::new_for_export(claude, std::env::current_dir()?, codex)?;
                crate::shared::export(&mut engine, &machine, &path)?;
                let outcome = crate::shared::publish(&ctx.args.host, &path).map(|_| SyncOutput {
                    machine,
                    host: ctx.args.host,
                });
                let _ = std::fs::remove_file(&path);
                outcome
            })
            .await;
            match result {
                Ok(Ok(output)) => TypedResult::ok(output),
                Ok(Err(e)) => TypedResult::error("SHARED_SYNC", e.to_string()),
                Err(e) => TypedResult::error("SHARED_SYNC", e.to_string()),
            }
        },
    )
    .description("Publish this machine's history to a Tailscale host over SSH")
    .args::<SyncArgs>()
    .options::<SyncOptions>()
    .done()
}
pub fn build_group() -> Cli {
    Cli::create("shared")
        .description("Share machine-labeled agent and command history")
        .command("export", export_command())
        .command("sync", sync_command())
        .command("query", query_command())
}
