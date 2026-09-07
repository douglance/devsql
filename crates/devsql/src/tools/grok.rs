//! `devsql grok` -- inspect and search Grok Bot history.
//!
//! Grok Bots have no working directory and no repo: their only path is a remote
//! `/home/box/sand-data/agents/<uuid>/store.db`. They are therefore global and
//! deliberately stay out of `recall`/`gather` default output — this group is the
//! explicit way to reach them.

use incurs::cli::Cli;
use incurs::command::{CommandDef, Example, TypedContext, TypedResult};
use rusqlite::params;
use serde_json::{json, Value};

use crate::engine::default_grok_data_dir;
use crate::grok_index::GrokIndex;
use crate::providers::grok_gateway::{GatewayError, GrokctlClient};
use crate::redaction::redact_sensitive_text;

use super::{network_read_mcp, read_only_mcp};

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct GrokOptions {
    /// Grok Bot data directory (defaults to the desktop app's app-support root)
    #[incurs(alias = "d")]
    grok_dir: Option<String>,
}

fn open_index(grok_dir: Option<&str>) -> Result<GrokIndex, String> {
    let dir = grok_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_grok_data_dir);
    let mut index = GrokIndex::open(&dir).map_err(|error| error.to_string())?;
    index.sync().map_err(|error| error.to_string())?;
    Ok(index)
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct StatusOutput {
    grok_dir: String,
    persistence_dir_present: bool,
    cache_path: String,
    bots: i64,
    bots_in_roster: i64,
    entries: i64,
    messages: i64,
    /// Blobs parsed by the sync that just ran; 0 means everything was current.
    parsed_blobs: usize,
    unchanged_blobs: usize,
    skipped_blobs: usize,
    ingest_errors: Vec<Value>,
    bot_rows: Vec<Value>,
}

fn status_command() -> CommandDef {
    CommandDef::typed::<(), GrokOptions, (), StatusOutput, _, _>(
        "status",
        |ctx: TypedContext<(), GrokOptions, ()>| async move {
            let dir = ctx
                .options
                .grok_dir
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(default_grok_data_dir);
            let mut index = match GrokIndex::open(&dir) {
                Ok(index) => index,
                Err(error) => return TypedResult::error("GROK_ERROR", error.to_string()),
            };
            let stats = match index.sync() {
                Ok(stats) => stats,
                Err(error) => return TypedResult::error("GROK_ERROR", error.to_string()),
            };
            match build_status(&index, &dir, stats) {
                Ok(output) => TypedResult::ok(output),
                Err(error) => TypedResult::error("GROK_ERROR", error),
            }
        },
    )
    .description("Report Grok Bot index coverage, sync state, and ingest errors")
    .options::<GrokOptions>()
    .mcp(read_only_mcp())
    .done()
}

fn build_status(
    index: &GrokIndex,
    dir: &std::path::Path,
    stats: crate::grok_index::GrokSyncStats,
) -> Result<StatusOutput, String> {
    let conn = index.conn();
    let scalar = |sql: &str| -> Result<i64, String> {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())
    };

    let mut errors = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT source, error_kind, message, occurrences, observed_at
                   FROM grok_ingest_errors ORDER BY observed_at DESC LIMIT 20",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(json!({
                    "source": row.get::<_, String>(0)?,
                    "error_kind": row.get::<_, String>(1)?,
                    "message": row.get::<_, String>(2)?,
                    "occurrences": row.get::<_, i64>(3)?,
                    "observed_at": row.get::<_, String>(4)?,
                }))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            errors.push(row.map_err(|error| error.to_string())?);
        }
    }

    let mut bot_rows = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(name, bot_id), bot_id, entry_count, roster_present,
                        newest_entry_id, gateway_backfilled_to_seq, gateway_complete,
                        last_entry_at
                   FROM grok_bots ORDER BY last_entry_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(json!({
                    "name": row.get::<_, String>(0)?,
                    "bot_id": row.get::<_, String>(1)?,
                    "entries": row.get::<_, i64>(2)?,
                    "in_roster": row.get::<_, i64>(3)? == 1,
                    "newest_entry_id": row.get::<_, Option<String>>(4)?,
                    "gateway_backfilled_to_seq": row.get::<_, Option<i64>>(5)?,
                    "gateway_complete": row.get::<_, i64>(6)? == 1,
                    "last_entry_at": row.get::<_, Option<String>>(7)?,
                }))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            bot_rows.push(row.map_err(|error| error.to_string())?);
        }
    }

    Ok(StatusOutput {
        grok_dir: dir.to_string_lossy().into_owned(),
        persistence_dir_present: index.persistence_dir().exists(),
        cache_path: index.cache_path().to_string_lossy().into_owned(),
        bots: scalar("SELECT COUNT(*) FROM grok_bots")?,
        bots_in_roster: scalar("SELECT COUNT(*) FROM grok_bots WHERE roster_present = 1")?,
        entries: scalar("SELECT COUNT(*) FROM grok_entries")?,
        messages: scalar(
            "SELECT COUNT(*) FROM grok_entries WHERE text IS NOT NULL AND text != ''",
        )?,
        parsed_blobs: stats.parsed_blobs,
        unchanged_blobs: stats.unchanged_blobs,
        skipped_blobs: stats.skipped_blobs,
        ingest_errors: errors,
        bot_rows,
    })
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SearchArgs {
    /// Text to look for in Grok Bot messages
    terms: String,
}

#[derive(Clone, Debug, Default, incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct SearchOptions {
    /// Grok Bot data directory (defaults to the desktop app's app-support root)
    #[incurs(alias = "d")]
    grok_dir: Option<String>,
    /// Restrict to one bot, by id or exact name
    #[incurs(alias = "b")]
    bot: Option<String>,
    /// Maximum number of matches
    #[incurs(alias = "n", default = 25)]
    limit: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct SearchOutput {
    terms: String,
    bot: Option<String>,
    /// Grok Bots have no repo or cwd, so results are never project-scoped.
    scope: &'static str,
    total: usize,
    matches: Vec<Value>,
}

fn search_command() -> CommandDef {
    CommandDef::typed::<SearchArgs, SearchOptions, (), SearchOutput, _, _>(
        "search",
        |ctx: TypedContext<SearchArgs, SearchOptions, ()>| async move {
            let index = match open_index(ctx.options.grok_dir.as_deref()) {
                Ok(index) => index,
                Err(error) => return TypedResult::error("GROK_ERROR", error),
            };

            let bot_filter = match ctx.options.bot.as_deref() {
                Some(needle) => match index.resolve_bot(needle) {
                    Ok(Some((bot_id, _))) => Some(bot_id),
                    Ok(None) => {
                        return TypedResult::error(
                            "UNKNOWN_BOT",
                            format!("No Grok Bot matches {needle:?}"),
                        )
                    }
                    Err(error) => return TypedResult::error("GROK_ERROR", error.to_string()),
                },
                None => None,
            };

            match run_search(
                &index,
                &ctx.args.terms,
                bot_filter.as_deref(),
                ctx.options.limit,
            ) {
                Ok(matches) => TypedResult::ok(SearchOutput {
                    terms: ctx.args.terms,
                    bot: ctx.options.bot,
                    scope: "global",
                    total: matches.len(),
                    matches,
                }),
                Err(error) => TypedResult::error("GROK_ERROR", error),
            }
        },
    )
    .description("Search Grok Bot messages (global: bots have no repo or cwd)")
    .args::<SearchArgs>()
    .options::<SearchOptions>()
    .examples(vec![
        Example {
            command: "\"standup card\"".to_string(),
            description: Some("Find Grok Bot messages mentioning a standup card".to_string()),
        },
        Example {
            command: "release --bot Terri".to_string(),
            description: Some("Search only Terri's transcript".to_string()),
        },
    ])
    .mcp(read_only_mcp())
    .done()
}

fn run_search(
    index: &GrokIndex,
    terms: &str,
    bot_id: Option<&str>,
    limit: i64,
) -> Result<Vec<Value>, String> {
    let pattern = format!("%{terms}%");
    let sql = "SELECT COALESCE(bot.name, entry.bot_id) AS bot_name,
                      entry.bot_id, entry.entry_id, entry.kind, entry.role,
                      entry.direction, entry.text, entry.timestamp
                 FROM grok_entries AS entry
                 LEFT JOIN grok_bots AS bot ON bot.bot_id = entry.bot_id
                WHERE entry.text IS NOT NULL AND entry.text != ''
                  AND entry.text LIKE ?1 COLLATE NOCASE
                  AND (?2 IS NULL OR entry.bot_id = ?2)
                ORDER BY entry.timestamp_ms DESC
                LIMIT ?3";

    let conn = index.conn();
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![pattern, bot_id, limit], |row| {
            let text: String = row.get(6)?;
            Ok(json!({
                "bot_name": row.get::<_, String>(0)?,
                "bot_id": row.get::<_, String>(1)?,
                "entry_id": row.get::<_, String>(2)?,
                "kind": row.get::<_, String>(3)?,
                "role": row.get::<_, Option<String>>(4)?,
                "direction": row.get::<_, Option<String>>(5)?,
                // Redaction happens at the surface, never at rest: the index
                // stays lossless and re-projectable.
                "excerpt": redact_sensitive_text(&excerpt(&text)),
                "timestamp": row.get::<_, Option<String>>(7)?,
            }))
        })
        .map_err(|error| error.to_string())?;

    let mut matches = Vec::new();
    for row in rows {
        matches.push(row.map_err(|error| error.to_string())?);
    }
    Ok(matches)
}

const EXCERPT_LIMIT: usize = 320;

fn excerpt(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= EXCERPT_LIMIT {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(EXCERPT_LIMIT).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
#[serde(default)]
struct SyncOptions {
    /// Grok Bot data directory (defaults to the desktop app's app-support root)
    #[incurs(alias = "d")]
    grok_dir: Option<String>,
    /// Restrict the backfill to these bots, by id or exact name
    #[incurs(alias = "b")]
    bot: Vec<String>,
    /// Maximum transcript pages to fetch per bot
    #[incurs(default = 50)]
    max_pages: i64,
    /// Entries requested per page
    #[incurs(default = 200)]
    page_size: i64,
    /// Page to the beginning instead of stopping at already-known entries
    full: bool,
    /// Ingest local replicas only; do not contact the gateway
    offline: bool,
    /// Per-grokctl-call timeout in seconds
    #[incurs(default = 60)]
    timeout: u64,
    /// Explicit path to the grokctl binary
    grokctl: Option<String>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            grok_dir: None,
            bot: Vec::new(),
            max_pages: 50,
            page_size: 200,
            full: false,
            offline: false,
            timeout: 60,
            grokctl: None,
        }
    }
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct SyncOutput {
    local: LocalSyncSummary,
    gateway: GatewaySummary,
    bots: Vec<Value>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct LocalSyncSummary {
    parsed_blobs: usize,
    unchanged_blobs: usize,
    skipped_blobs: usize,
    entries: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct GatewaySummary {
    reachable: bool,
    reason: Option<String>,
    entries_written: usize,
}

fn sync_command() -> CommandDef {
    CommandDef::typed::<(), SyncOptions, (), SyncOutput, _, _>(
        "sync",
        |ctx: TypedContext<(), SyncOptions, ()>| async move {
            match run_sync(ctx.options) {
                Ok(output) => TypedResult::ok(output),
                Err(error) => TypedResult::error("GROK_ERROR", error),
            }
        },
    )
    .description(
        "Ingest Grok Bot local replicas, then optionally backfill full history \
         from the gateway via grokctl",
    )
    .options::<SyncOptions>()
    .examples(vec![
        Example {
            command: "--offline".to_string(),
            description: Some("Refresh from local replicas only, offline".to_string()),
        },
        Example {
            command: "--bot Terri --full".to_string(),
            description: Some("Backfill one bot's complete history".to_string()),
        },
    ])
    .mcp(network_read_mcp())
    .done()
}

fn run_sync(options: SyncOptions) -> Result<SyncOutput, String> {
    let dir = options
        .grok_dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_grok_data_dir);
    let mut index = GrokIndex::open(&dir).map_err(|error| error.to_string())?;
    let stats = index.sync().map_err(|error| error.to_string())?;

    let local = LocalSyncSummary {
        parsed_blobs: stats.parsed_blobs,
        unchanged_blobs: stats.unchanged_blobs,
        skipped_blobs: stats.skipped_blobs,
        entries: count_entries(&index)?,
    };

    if options.offline {
        return Ok(SyncOutput {
            local,
            gateway: GatewaySummary {
                reachable: false,
                reason: Some("--offline".to_string()),
                entries_written: 0,
            },
            bots: Vec::new(),
        });
    }

    // A missing or failing grokctl is a note, never a failure: local replicas
    // are already committed and remain queryable.
    let client = match GrokctlClient::discover(
        options.grokctl.as_deref(),
        std::time::Duration::from_secs(options.timeout),
    ) {
        Ok(client) => client,
        Err(error) => {
            let reason = error.to_string();
            let _ = index.record_error("gateway", None, "gateway_unavailable", &reason);
            return Ok(SyncOutput {
                local,
                gateway: GatewaySummary {
                    reachable: false,
                    reason: Some(reason),
                    entries_written: 0,
                },
                bots: Vec::new(),
            });
        }
    };

    let targets = resolve_targets(&index, &options.bot)?;

    let mut bots = Vec::new();
    let mut written_total = 0usize;
    let mut reachable = true;
    let mut reason = None;

    for (bot_id, name) in targets {
        match backfill_bot(&mut index, &client, &bot_id, &options) {
            Ok(summary) => {
                written_total += summary.written;
                bots.push(json!({
                    "bot_id": bot_id,
                    "name": name,
                    "entries_written": summary.written,
                    "pages": summary.pages,
                    "lowest_seq": summary.lowest_seq,
                    "complete": summary.complete,
                }));
            }
            Err(error) => {
                let message = error.to_string();
                if matches!(
                    error,
                    GatewayError::BinaryNotFound(_) | GatewayError::Spawn(_)
                ) {
                    reachable = false;
                    reason.get_or_insert(message.clone());
                }
                let _ = index.record_error(
                    &format!("gateway:{bot_id}"),
                    Some(&bot_id),
                    "gateway_page",
                    &message,
                );
                bots.push(json!({
                    "bot_id": bot_id,
                    "name": name,
                    "entries_written": 0,
                    "note": message,
                }));
            }
        }
    }

    Ok(SyncOutput {
        local: LocalSyncSummary {
            entries: count_entries(&index)?,
            ..local
        },
        gateway: GatewaySummary {
            reachable,
            reason,
            entries_written: written_total,
        },
        bots,
    })
}

fn count_entries(index: &GrokIndex) -> Result<i64, String> {
    index
        .conn()
        .query_row("SELECT COUNT(*) FROM grok_entries", [], |row| row.get(0))
        .map_err(|error| error.to_string())
}

fn resolve_targets(
    index: &GrokIndex,
    requested: &[String],
) -> Result<Vec<(String, String)>, String> {
    if requested.is_empty() {
        return index.known_bots().map_err(|error| error.to_string());
    }
    let mut targets = Vec::new();
    for needle in requested {
        match index.resolve_bot(needle).map_err(|e| e.to_string())? {
            Some(found) => targets.push(found),
            None => return Err(format!("No Grok Bot matches {needle:?}")),
        }
    }
    Ok(targets)
}

struct BackfillSummary {
    written: usize,
    pages: usize,
    lowest_seq: Option<i64>,
    complete: bool,
}

/// Page backward through a bot's transcript.
///
/// Stops when the gateway offers no further cursor, when the cursor stops
/// decreasing (an infinite-loop guard), at `--max-pages`, or — unless `--full`
/// — as soon as a page contains only entries already indexed.
fn backfill_bot(
    index: &mut GrokIndex,
    client: &GrokctlClient,
    bot_id: &str,
    options: &SyncOptions,
) -> Result<BackfillSummary, GatewayError> {
    let mut before_seq: Option<i64> = None;
    let mut lowest_seq: Option<i64> = None;
    let mut written = 0usize;
    let mut pages = 0usize;
    let mut complete = false;

    while pages < options.max_pages.max(1) as usize {
        let page = client.transcript_page(bot_id, options.page_size, before_seq)?;
        pages += 1;

        if page.entries.is_empty() {
            complete = true;
            break;
        }

        let new_rows = index
            .record_gateway_entries(bot_id, &page.entries, page.next_before_seq)
            .map_err(|error| GatewayError::Parse(error.to_string()))?;
        written += new_rows;

        let Some(next) = page.next_before_seq else {
            complete = true;
            break;
        };
        // A cursor that fails to decrease would page forever.
        if before_seq.is_some_and(|current| next >= current) {
            break;
        }
        lowest_seq = Some(next);
        before_seq = Some(next);

        if !options.full && new_rows == 0 {
            // Caught up with what is already indexed.
            break;
        }
    }

    index
        .mark_gateway_progress(bot_id, lowest_seq, complete)
        .map_err(|error| GatewayError::Parse(error.to_string()))?;

    Ok(BackfillSummary {
        written,
        pages,
        lowest_seq,
        complete,
    })
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build_group() -> Cli {
    Cli::create("grok")
        .description(
            "Inspect and search Grok Bot history.\n\
             Bots are global: they have no repo or cwd, so they are an explicit \
             search target rather than part of recall/gather output.",
        )
        .command("status", status_command())
        .command("sync", sync_command())
        .command("search", search_command())
}
