//! Incremental on-disk index over Grok Bot history.
//!
//! Two writers feed one cache:
//!
//! * The Grok Bot desktop app's local replicas
//!   (`<grok dir>/sand-client-persistence/*.blob`) — offline, zero-config, but
//!   *truncated client-side windows*.
//! * Optional gateway backfill through the `grokctl` binary — complete, but it
//!   needs a reachable gateway.
//!
//! Because local replicas shrink as the desktop app evicts old entries, this
//! index is **append/upsert-only and never prunes entries**. That is a
//! deliberate divergence from `codex_index`, which prunes threads whose
//! journals disappeared: copying that here would silently destroy history that
//! exists nowhere else.

use crate::Result;
use chrono::{DateTime, Utc};
use data_encoding::BASE32_NOPAD;
use rusqlite::{
    params, Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Bumping this re-projects every entry from its stored `raw_json`.
const SCHEMA_VERSION: i64 = 3;

/// Slice of the desktop app's persistence layer a blob belongs to.
const SLICE_ROSTER: &str = "roster";
const SLICE_TRANSCRIPT: &str = "transcript";
const SLICE_OTHER: &str = "other";

const PERSISTENCE_SUBDIR: &str = "sand-client-persistence";
const SLICE_STORE_DB: &str = "store_db";

/// Inside a Grok Bot sandbox the per-bot SQLite stores are the server of
/// record. The host exposes the same tree under two names.
const IN_BOX_ROOTS: [&str; 2] = ["/home/box/agent-data", "/home/box/sand-data"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GrokSyncStats {
    pub parsed_store_dbs: usize,
    pub parsed_blobs: usize,
    pub parsed_entries: usize,
    pub unchanged_blobs: usize,
    pub skipped_blobs: usize,
}

pub(crate) struct GrokIndex {
    conn: Connection,
    grok_dir: PathBuf,
    cache_path: PathBuf,
}

impl GrokIndex {
    pub(crate) fn open(grok_dir: &Path) -> Result<Self> {
        let cache_root = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("devsql-cache"))
            .join("devsql")
            .join("grok-index");
        let canonical = grok_dir
            .canonicalize()
            .unwrap_or_else(|_| grok_dir.to_path_buf());
        let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
        let cache_path = cache_root.join(format!("{digest:x}.sqlite"));
        Self::open_at(grok_dir, &cache_path)
    }

    pub(crate) fn open_at(grok_dir: &Path, cache_path: &Path) -> Result<Self> {
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent)?;
            set_private_directory_permissions(parent)?;
        }
        let mut conn = match open_cache_connection(cache_path) {
            Ok(conn) => conn,
            Err(error) if is_corrupt_cache_error(&error) => {
                remove_cache_files(cache_path)?;
                open_cache_connection(cache_path)?
            }
            Err(error) => return Err(error),
        };
        let version: i64 = match conn.pragma_query_value(None, "user_version", |row| row.get(0)) {
            Ok(version) => version,
            Err(error) if is_corrupt_sql_error(&error) => {
                drop(conn);
                remove_cache_files(cache_path)?;
                conn = open_cache_connection(cache_path)?;
                0
            }
            Err(error) => return Err(error.into()),
        };
        if version == 0 {
            ensure_wal_mode(&conn)?;
            create_schema(&conn)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version != SCHEMA_VERSION {
            // Never delete-and-rebuild: the cache may hold gateway-backfilled
            // entries that no longer exist in any local replica. Every entry
            // stores its full `raw_json`, so any schema version is rebuildable
            // offline from what is already here.
            migrate_by_reprojection(&mut conn, cache_path)?;
        }
        Ok(Self {
            conn,
            grok_dir: grok_dir.to_path_buf(),
            cache_path: cache_path.to_path_buf(),
        })
    }

    pub(crate) fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    pub(crate) fn persistence_dir(&self) -> PathBuf {
        self.grok_dir.join(PERSISTENCE_SUBDIR)
    }

    /// Ingest local desktop replicas. Missing directory yields zero rows, never
    /// an error — the same invariant `shell_history` applies to absent sources.
    pub(crate) fn sync(&mut self) -> Result<GrokSyncStats> {
        let dir = self.persistence_dir();
        let blobs = if dir.exists() {
            discover_blobs(&dir)?
        } else {
            // In-box there is no desktop app, only per-bot store.db files.
            Vec::new()
        };
        if blobs.is_empty() && discover_store_dbs(&self.grok_dir).is_empty() {
            return Ok(GrokSyncStats::default());
        }

        let mut stats = GrokSyncStats::default();
        self.conn.busy_timeout(Duration::from_millis(100))?;
        let mut tx = match self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
        {
            Ok(tx) => tx,
            // Another devsql process is mid-sync; its work is as good as ours.
            Err(error) if is_busy_sql_error(&error) => return Ok(stats),
            Err(error) => return Err(error.into()),
        };

        // Roster first, so transcript ingest can attach entries to known bots.
        let mut ordered = blobs;
        ordered.sort_by_key(|blob| match blob.slice.as_str() {
            SLICE_ROSTER => 0,
            SLICE_TRANSCRIPT => 1,
            _ => 2,
        });

        for blob in &ordered {
            match sync_blob(&mut tx, blob)? {
                BlobOutcome::Unchanged => stats.unchanged_blobs += 1,
                BlobOutcome::Skipped => stats.skipped_blobs += 1,
                BlobOutcome::Ingested { entries } => {
                    stats.parsed_blobs += 1;
                    stats.parsed_entries += entries;
                }
            }
        }

        for store in discover_store_dbs(&self.grok_dir) {
            match sync_store_db(&mut tx, &store) {
                Ok(StoreOutcome::Unchanged) => stats.unchanged_blobs += 1,
                Ok(StoreOutcome::Ingested { entries }) => {
                    stats.parsed_store_dbs += 1;
                    stats.parsed_entries += entries;
                }
                Err(error) => {
                    stats.skipped_blobs += 1;
                    record_ingest_error(
                        &tx,
                        &store.path.to_string_lossy(),
                        Some(&store.bot_id),
                        None,
                        "store_db_read",
                        &error.to_string(),
                    )?;
                }
            }
        }

        refresh_bot_rollups(&tx)?;
        set_meta(
            &tx,
            "last_sync_completed_ms",
            &Utc::now().timestamp_millis().to_string(),
        )?;
        bump_counter(&tx, "sync_parsed_blobs", stats.parsed_blobs as i64)?;
        tx.commit()?;
        Ok(stats)
    }

    /// Upsert entries fetched from the gateway. Callers own the paging loop;
    /// this is the storage half.
    pub(crate) fn record_gateway_entries(
        &mut self,
        bot_id: &str,
        entries: &[Value],
        source_seq: Option<i64>,
    ) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        ensure_bot_row(&tx, bot_id)?;
        let base = next_source_order(&tx, bot_id)?;
        let mut written = 0usize;
        for (offset, entry) in entries.iter().enumerate() {
            if upsert_entry(
                &tx,
                bot_id,
                entry,
                base + offset as i64,
                "gateway",
                source_seq,
                None,
            )? {
                written += 1;
            }
        }
        refresh_bot_rollups(&tx)?;
        tx.commit()?;
        Ok(written)
    }

    pub(crate) fn mark_gateway_progress(
        &mut self,
        bot_id: &str,
        lowest_seq: Option<i64>,
        complete: bool,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE grok_bots
                SET gateway_backfilled_to_seq =
                      CASE
                        WHEN ?2 IS NULL THEN gateway_backfilled_to_seq
                        WHEN gateway_backfilled_to_seq IS NULL THEN ?2
                        WHEN ?2 < gateway_backfilled_to_seq THEN ?2
                        ELSE gateway_backfilled_to_seq
                      END,
                    gateway_complete = MAX(gateway_complete, ?3),
                    gateway_synced_at = ?4
              WHERE bot_id = ?1",
            params![
                bot_id,
                lowest_seq,
                i64::from(complete),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub(crate) fn record_error(
        &mut self,
        source: &str,
        bot_id: Option<&str>,
        error_kind: &str,
        message: &str,
    ) -> Result<()> {
        record_ingest_error(&self.conn, source, bot_id, None, error_kind, message)
    }

    /// Resolve a bot by exact id, else by case-insensitive name.
    pub(crate) fn resolve_bot(&self, needle: &str) -> Result<Option<(String, String)>> {
        let found = self
            .conn
            .query_row(
                "SELECT bot_id, COALESCE(name, bot_id) FROM grok_bots
                  WHERE bot_id = ?1 OR lower(COALESCE(name, '')) = lower(?1)
                  ORDER BY (bot_id = ?1) DESC
                  LIMIT 1",
                params![needle],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(found)
    }

    pub(crate) fn known_bots(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT bot_id, COALESCE(name, bot_id) FROM grok_bots ORDER BY name")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}

// ---------------------------------------------------------------------------
// Blob discovery and key decoding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct GrokBlob {
    pub path: PathBuf,
    pub logical_key: String,
    pub slice: String,
    pub bot_id: Option<String>,
    pub account_id: Option<String>,
    pub size: u64,
    pub modified_ns: i64,
}

/// Filenames are lowercase, unpadded RFC-4648 base32 of the logical key.
pub(crate) fn decode_blob_key(file_stem: &str) -> Option<String> {
    let upper = file_stem.to_uppercase();
    let bytes = BASE32_NOPAD.decode(upper.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
}

const ACCOUNT_PREFIX: &str = "sand.client.slice.account.";

/// The account id is percent-encoded (`github%7C4741454`) and is not guaranteed
/// to be free of `.`, so match on a known slice suffix rather than splitting on
/// the first separator.
pub(crate) fn classify_key(key: &str) -> (String, Option<String>, Option<String>) {
    let Some(rest) = key.strip_prefix(ACCOUNT_PREFIX) else {
        return (SLICE_OTHER.to_string(), None, None);
    };

    if let Some(idx) = rest.rfind(".transcript.replicas.") {
        let account = &rest[..idx];
        let bot = &rest[idx + ".transcript.replicas.".len()..];
        if !bot.is_empty() {
            return (
                SLICE_TRANSCRIPT.to_string(),
                Some(bot.to_string()),
                Some(account.to_string()),
            );
        }
    }

    if let Some(account) = rest.strip_suffix(".roster.last-roster") {
        return (SLICE_ROSTER.to_string(), None, Some(account.to_string()));
    }

    let account = rest.split('.').next().unwrap_or_default().to_string();
    (SLICE_OTHER.to_string(), None, Some(account))
}

fn discover_blobs(dir: &Path) -> Result<Vec<GrokBlob>> {
    let mut blobs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("blob") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Undecodable names are skipped, not fatal.
        let Some(logical_key) = decode_blob_key(stem) else {
            continue;
        };
        let metadata = entry.metadata()?;
        let (slice, bot_id, account_id) = classify_key(&logical_key);
        blobs.push(GrokBlob {
            path,
            logical_key,
            slice,
            bot_id,
            account_id,
            size: metadata.len(),
            modified_ns: modified_ns(&metadata),
        });
    }
    Ok(blobs)
}

fn modified_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Per-bot SQLite stores (the in-box server of record)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct StoreDb {
    pub path: PathBuf,
    pub bot_id: String,
    pub size: u64,
    pub modified_ns: i64,
}

/// Find `<root>/agents/<bot-uuid>/store.db`.
///
/// Checks the configured directory first, then the two well-known in-box roots,
/// so a bot inspecting itself needs no configuration.
pub(crate) fn discover_store_dbs(grok_dir: &Path) -> Vec<StoreDb> {
    let mut roots = vec![grok_dir.join("agents")];
    roots.extend(
        IN_BOX_ROOTS
            .iter()
            .map(|root| Path::new(root).join("agents")),
    );

    let mut seen = std::collections::HashSet::new();
    let mut stores = Vec::new();
    for root in roots {
        // The two in-box roots are aliases for one tree; key on bot id so a bot
        // is never ingested twice.
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(bot_id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path().join("store.db");
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !seen.insert(bot_id.clone()) {
                continue;
            }
            stores.push(StoreDb {
                path,
                bot_id,
                size: metadata.len(),
                modified_ns: modified_ns(&metadata),
            });
        }
    }
    stores
}

enum StoreOutcome {
    Unchanged,
    Ingested { entries: usize },
}

fn sync_store_db(tx: &mut Transaction<'_>, store: &StoreDb) -> Result<StoreOutcome> {
    let key = store.path.to_string_lossy().into_owned();
    let cached: Option<(i64, i64, Option<i64>)> = tx
        .query_row(
            "SELECT size, modified_ns, last_seq FROM source_blobs WHERE blob_path = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    if let Some((size, modified_ns, _)) = cached {
        if size == store.size as i64 && modified_ns == store.modified_ns {
            return Ok(StoreOutcome::Unchanged);
        }
    }
    let last_seq = cached.and_then(|(_, _, seq)| seq).unwrap_or(-1);

    // `seq` is a monotonic rowid, so resuming from the watermark is exact --
    // no offset arithmetic and no torn-record handling needed.
    let rows = read_transcript_entries(&store.path, last_seq)?;

    ensure_bot_row(tx, &store.bot_id)?;
    let mut written = 0usize;
    let mut highest = last_seq;
    for (seq, entry_id, raw) in rows {
        highest = highest.max(seq);
        let Ok(entry) = serde_json::from_str::<Value>(&raw) else {
            record_ingest_error(
                tx,
                &key,
                Some(&store.bot_id),
                Some(&entry_id),
                "store_db_entry_json",
                &format!("entry {entry_id} at seq {seq} is not valid JSON"),
            )?;
            continue;
        };
        // The stored JSON is the same entry shape the desktop replicas and the
        // gateway return, so one projection serves all three sources.
        if upsert_entry(
            tx,
            &store.bot_id,
            &entry,
            seq,
            "store_db",
            Some(seq),
            Some(&key),
        )? {
            written += 1;
        }
    }

    // In-box there is no roster blob, so identity comes from the store's own
    // key/value table. Roster data, when present, stays authoritative.
    if let Ok(profile) = read_store_kv(&store.path) {
        apply_store_profile(tx, &store.bot_id, &profile)?;
    }

    tx.execute(
        "INSERT INTO source_blobs (
             blob_path, logical_key, slice, bot_id, account_id, schema_version,
             size, modified_ns, persisted_at_ms, last_seq, entry_count, ingested_at)
         VALUES (?1,?2,?3,?4,NULL,NULL,?5,?6,NULL,?7,?8,?9)
         ON CONFLICT(blob_path) DO UPDATE SET
             size = excluded.size,
             modified_ns = excluded.modified_ns,
             last_seq = excluded.last_seq,
             entry_count = source_blobs.entry_count + excluded.entry_count,
             ingested_at = excluded.ingested_at",
        params![
            key,
            format!("store.db:{}", store.bot_id),
            SLICE_STORE_DB,
            store.bot_id,
            store.size as i64,
            store.modified_ns,
            highest,
            written as i64,
            Utc::now().to_rfc3339(),
        ],
    )?;

    Ok(StoreOutcome::Ingested { entries: written })
}

#[derive(Debug, Default)]
struct StoreProfile {
    name: Option<String>,
    origin: Option<String>,
    last_activity_at: Option<String>,
    unread_count: Option<i64>,
}

/// Pull bot identity out of the store's `kv` table.
fn read_store_kv(path: &Path) -> Result<StoreProfile> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;

    let mut stmt = conn.prepare("SELECT key, value FROM kv")?;
    let mut profile = StoreProfile::default();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            "origin" => profile.origin = Some(value),
            "agentProfilePromptSnapshot" => {
                profile.name = serde_json::from_str::<Value>(&value).ok().and_then(|v| {
                    v.get("profileSection")
                        .and_then(Value::as_str)
                        .and_then(profile_title)
                });
            }
            "unreadState" => {
                if let Ok(state) = serde_json::from_str::<Value>(&value) {
                    profile.last_activity_at =
                        epoch_ms_to_rfc3339(state.get("lastActivityAt").and_then(Value::as_i64));
                    profile.unread_count = state.get("unreadCount").and_then(Value::as_i64);
                }
            }
            _ => {}
        }
    }
    Ok(profile)
}

/// The display name is not its own key; it appears as a `Title:` line inside
/// the rendered profile prompt.
fn profile_title(section: &str) -> Option<String> {
    section.lines().find_map(|line| {
        line.strip_prefix("Title:")
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
    })
}

fn apply_store_profile(conn: &Connection, bot_id: &str, profile: &StoreProfile) -> Result<()> {
    conn.execute(
        "UPDATE grok_bots SET
             name = COALESCE(name, ?2),
             origin = COALESCE(origin, ?3),
             last_activity_at = COALESCE(last_activity_at, ?4),
             unread_count = COALESCE(unread_count, ?5)
         WHERE bot_id = ?1",
        params![
            bot_id,
            profile.name,
            profile.origin,
            profile.last_activity_at,
            profile.unread_count
        ],
    )?;
    Ok(())
}

/// Read new transcript rows from a bot's store, read-only.
///
/// The owning bot may be writing concurrently, so this opens read-only with a
/// busy timeout and never takes a write lock.
fn read_transcript_entries(path: &Path, after_seq: i64) -> Result<Vec<(i64, String, String)>> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;

    let mut stmt =
        conn.prepare("SELECT seq, id, entry FROM transcript_entries WHERE seq > ?1 ORDER BY seq")?;
    let rows = stmt
        .query_map(params![after_seq], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

enum BlobOutcome {
    Unchanged,
    Skipped,
    Ingested { entries: usize },
}

fn sync_blob(tx: &mut Transaction<'_>, blob: &GrokBlob) -> Result<BlobOutcome> {
    let key = blob.path.to_string_lossy().into_owned();
    let cached: Option<(i64, i64, Option<i64>)> = tx
        .query_row(
            "SELECT size, modified_ns, persisted_at_ms FROM source_blobs WHERE blob_path = ?1",
            params![key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    if let Some((size, modified_ns, _)) = cached {
        if size == blob.size as i64 && modified_ns == blob.modified_ns {
            return Ok(BlobOutcome::Unchanged);
        }
    }

    let savepoint = tx.savepoint()?;
    let outcome = ingest_blob(&savepoint, blob, cached.and_then(|(_, _, p)| p));
    match outcome {
        Ok(result) => {
            savepoint.commit()?;
            Ok(result)
        }
        Err(error) => {
            // One malformed replica must not abort the rest. Note the watermark
            // is deliberately not advanced, so a torn read retries next run.
            drop(savepoint);
            record_ingest_error(
                tx,
                &key,
                blob.bot_id.as_deref(),
                None,
                "blob_parse",
                &error.to_string(),
            )?;
            Ok(BlobOutcome::Skipped)
        }
    }
}

fn ingest_blob(
    conn: &Connection,
    blob: &GrokBlob,
    cached_persisted_at: Option<i64>,
) -> Result<BlobOutcome> {
    let text = fs::read_to_string(&blob.path)?;
    let root: Value = serde_json::from_str(&text)?;
    let schema_version = root.get("schemaVersion").and_then(Value::as_i64);
    let value = root.get("value").unwrap_or(&Value::Null);
    let persisted_at_ms = value.get("persistedAt").and_then(Value::as_i64);

    // The Electron app rewrites blobs in place, so a same-size rewrite under a
    // coarse mtime is possible. Treat a regressing persistedAt as suspect.
    if let (Some(cached), Some(current)) = (cached_persisted_at, persisted_at_ms) {
        if current < cached {
            record_ingest_error(
                conn,
                &blob.path.to_string_lossy(),
                blob.bot_id.as_deref(),
                None,
                "persisted_at_regression",
                &format!("stored persistedAt {cached} is newer than incoming {current}"),
            )?;
            return Ok(BlobOutcome::Skipped);
        }
    }

    let entries = match blob.slice.as_str() {
        SLICE_ROSTER => {
            ingest_roster(conn, value, blob)?;
            0
        }
        SLICE_TRANSCRIPT => ingest_transcript(conn, value, blob)?,
        _ => 0,
    };

    conn.execute(
        "INSERT INTO source_blobs (
             blob_path, logical_key, slice, bot_id, account_id, schema_version,
             size, modified_ns, persisted_at_ms, entry_count, ingested_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(blob_path) DO UPDATE SET
             logical_key = excluded.logical_key,
             slice = excluded.slice,
             bot_id = excluded.bot_id,
             account_id = excluded.account_id,
             schema_version = excluded.schema_version,
             size = excluded.size,
             modified_ns = excluded.modified_ns,
             persisted_at_ms = excluded.persisted_at_ms,
             entry_count = excluded.entry_count,
             ingested_at = excluded.ingested_at",
        params![
            blob.path.to_string_lossy(),
            blob.logical_key,
            blob.slice,
            blob.bot_id,
            blob.account_id,
            schema_version,
            blob.size as i64,
            blob.modified_ns,
            persisted_at_ms,
            entries as i64,
            Utc::now().to_rfc3339(),
        ],
    )?;

    Ok(BlobOutcome::Ingested { entries })
}

fn ingest_roster(conn: &Connection, value: &Value, blob: &GrokBlob) -> Result<()> {
    let rows = roster_rows(value);
    // A bot dropped from the roster keeps its entries; only the flag flips.
    conn.execute("UPDATE grok_bots SET roster_present = 0", [])?;

    for row in rows {
        let Some(bot_id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        conn.execute(
            "INSERT INTO grok_bots (
                 bot_id, account_id, name, title, description, is_group, member_ids_json,
                 origin, remote_store_path, created_at, updated_at, last_activity_at,
                 last_viewed_at, newest_entry_id, last_message_id, last_entry_kind,
                 last_entry_text, unread_count, has_unread, awaiting_user_response,
                 roster_json, roster_present, local_replica_path)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,1,?22)
             ON CONFLICT(bot_id) DO UPDATE SET
                 account_id = excluded.account_id,
                 name = excluded.name,
                 title = excluded.title,
                 description = excluded.description,
                 is_group = excluded.is_group,
                 member_ids_json = excluded.member_ids_json,
                 origin = excluded.origin,
                 remote_store_path = excluded.remote_store_path,
                 created_at = excluded.created_at,
                 updated_at = excluded.updated_at,
                 last_activity_at = excluded.last_activity_at,
                 last_viewed_at = excluded.last_viewed_at,
                 newest_entry_id = excluded.newest_entry_id,
                 last_message_id = excluded.last_message_id,
                 last_entry_kind = excluded.last_entry_kind,
                 last_entry_text = excluded.last_entry_text,
                 unread_count = excluded.unread_count,
                 has_unread = excluded.has_unread,
                 awaiting_user_response = excluded.awaiting_user_response,
                 roster_json = excluded.roster_json,
                 roster_present = 1",
            params![
                bot_id,
                blob.account_id,
                row.get("name").and_then(Value::as_str),
                row.get("title").and_then(Value::as_str),
                row.get("description").and_then(Value::as_str),
                i64::from(row.get("isGroup").and_then(Value::as_bool).unwrap_or(false)),
                row.get("memberIds").map(ToString::to_string),
                row.get("origin").and_then(Value::as_str),
                row.get("path").and_then(Value::as_str),
                epoch_ms_to_rfc3339(row.get("createdAt").and_then(Value::as_i64)),
                epoch_ms_to_rfc3339(row.get("updatedAt").and_then(Value::as_i64)),
                epoch_ms_to_rfc3339(row.get("lastActivityAt").and_then(Value::as_i64)),
                epoch_ms_to_rfc3339(row.get("lastViewedAt").and_then(Value::as_i64)),
                row.get("newestEntryId").and_then(Value::as_str),
                row.get("lastMessageId").and_then(Value::as_str),
                row.get("lastEntry")
                    .and_then(|e| e.get("kind"))
                    .and_then(Value::as_str),
                row.get("lastEntry")
                    .and_then(|e| e.get("content"))
                    .and_then(Value::as_str),
                row.get("unreadCount").and_then(Value::as_i64),
                row.get("hasUnread").and_then(Value::as_bool).map(i64::from),
                i64::from(row.get("awaitingUserResponse").is_some_and(|v| !v.is_null())),
                row.to_string(),
                blob.path.to_string_lossy(),
            ],
        )?;
    }
    Ok(())
}

/// The roster payload has been seen as `{"rows": [...]}` and as
/// `{"rows": [[...]]}`; accept either.
fn roster_rows(value: &Value) -> Vec<&Value> {
    let Some(rows) = value.get("rows").and_then(Value::as_array) else {
        return Vec::new();
    };
    if let Some(first) = rows.first() {
        if first.is_array() {
            return rows.iter().filter_map(Value::as_array).flatten().collect();
        }
    }
    rows.iter().collect()
}

fn ingest_transcript(conn: &Connection, value: &Value, blob: &GrokBlob) -> Result<usize> {
    let Some(bot_id) = blob.bot_id.as_deref() else {
        return Ok(0);
    };
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    ensure_bot_row(conn, bot_id)?;
    conn.execute(
        "UPDATE grok_bots
            SET local_replica_path = ?2, local_persisted_at = ?3
          WHERE bot_id = ?1",
        params![
            bot_id,
            blob.path.to_string_lossy(),
            epoch_ms_to_rfc3339(value.get("persistedAt").and_then(Value::as_i64)),
        ],
    )?;

    let mut written = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        if upsert_entry(
            conn,
            bot_id,
            entry,
            index as i64,
            "local_replica",
            None,
            Some(&blob.path.to_string_lossy()),
        )? {
            written += 1;
        }
    }
    Ok(written)
}

fn ensure_bot_row(conn: &Connection, bot_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO grok_bots (bot_id, roster_json, roster_present)
         VALUES (?1, '{}', 0)
         ON CONFLICT(bot_id) DO NOTHING",
        params![bot_id],
    )?;
    Ok(())
}

fn next_source_order(conn: &Connection, bot_id: &str) -> Result<i64> {
    let max: Option<i64> = conn.query_row(
        "SELECT MAX(source_order) FROM grok_entries WHERE bot_id = ?1",
        params![bot_id],
        |row| row.get(0),
    )?;
    Ok(max.map_or(0, |value| value + 1))
}

/// Upsert one entry. Returns whether a row was written.
///
/// `provenance` becomes `both` once an entry has been seen from a local replica
/// and the gateway — which makes "is my local cache lagging?" a one-line query.
fn upsert_entry(
    conn: &Connection,
    bot_id: &str,
    entry: &Value,
    source_order: i64,
    provenance: &str,
    source_seq: Option<i64>,
    source_path: Option<&str>,
) -> Result<bool> {
    let Some(entry_id) = entry.get("id").and_then(Value::as_str) else {
        return Ok(false);
    };
    let kind = entry
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let projected = project_entry(entry, kind);
    let id_parts = parse_entry_id(entry_id);
    let timestamp_ms = entry
        .get("timestampMs")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    conn.execute(
        "INSERT INTO grok_entries (
             bot_id, entry_id, kind, role, direction, turn_ordinal, turn_token,
             entry_suffix, suffix_index, timestamp, timestamp_ms, message_type,
             text, rich_text_json, request_id, client_nonce, batch_id,
             from_agent, to_agent, author, event_type, is_streaming,
             raw_json, provenance, source_seq, source_order, source_path)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)
         ON CONFLICT(bot_id, entry_id) DO UPDATE SET
             kind = excluded.kind,
             role = excluded.role,
             direction = excluded.direction,
             turn_ordinal = excluded.turn_ordinal,
             turn_token = excluded.turn_token,
             entry_suffix = excluded.entry_suffix,
             suffix_index = excluded.suffix_index,
             timestamp = excluded.timestamp,
             timestamp_ms = excluded.timestamp_ms,
             message_type = excluded.message_type,
             text = excluded.text,
             rich_text_json = excluded.rich_text_json,
             request_id = excluded.request_id,
             client_nonce = excluded.client_nonce,
             batch_id = excluded.batch_id,
             from_agent = excluded.from_agent,
             to_agent = excluded.to_agent,
             author = excluded.author,
             event_type = excluded.event_type,
             is_streaming = excluded.is_streaming,
             raw_json = excluded.raw_json,
             provenance = CASE
                 WHEN grok_entries.provenance != excluded.provenance THEN 'both'
                 ELSE excluded.provenance END,
             source_seq = COALESCE(excluded.source_seq, grok_entries.source_seq),
             source_path = COALESCE(excluded.source_path, grok_entries.source_path)",
        params![
            bot_id,
            entry_id,
            kind,
            projected.role,
            projected.direction,
            id_parts.turn_ordinal,
            id_parts.turn_token,
            id_parts.suffix,
            id_parts.suffix_index,
            epoch_ms_to_rfc3339(Some(timestamp_ms)),
            timestamp_ms,
            projected.message_type,
            projected.text,
            entry.get("richText").and_then(Value::as_str),
            entry.get("requestId").and_then(Value::as_str),
            entry.get("clientNonce").and_then(Value::as_str),
            entry.get("batchId").and_then(Value::as_str),
            agent_label(entry.get("fromAgent")),
            agent_label(entry.get("toAgent")),
            agent_label(entry.get("author")),
            entry
                .get("event")
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str),
            entry.get("isStreaming").and_then(Value::as_bool).map(i64::from),
            entry.to_string(),
            provenance,
            source_seq,
            source_order,
            source_path,
        ],
    )?;
    Ok(true)
}

struct ProjectedEntry {
    role: Option<&'static str>,
    direction: Option<&'static str>,
    message_type: Option<String>,
    text: Option<String>,
}

/// Text projection stays conservative on purpose: a widget serialized into
/// `text` would poison LIKE-based search with UI chrome. Unknown kinds still
/// produce a row — `raw_json` keeps them recoverable.
fn project_entry(entry: &Value, kind: &str) -> ProjectedEntry {
    match kind {
        "message" => {
            let role = entry.get("role").and_then(Value::as_str);
            let (role, direction) = match role {
                Some("user") => (Some("user"), Some("inbound")),
                Some("assistant") => (Some("assistant"), Some("outbound")),
                _ => (None, Some("system")),
            };
            ProjectedEntry {
                role,
                direction,
                message_type: None,
                text: entry
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        }
        "send-message" => {
            let message = entry.get("message");
            let message_type = message
                .and_then(|m| m.get("type"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let text = if message_type.as_deref() == Some("text") {
                message
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            } else {
                entry
                    .get("boxInstruction")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            ProjectedEntry {
                role: Some("assistant"),
                direction: Some("outbound"),
                message_type,
                text,
            }
        }
        "user-attachment" => ProjectedEntry {
            role: Some("user"),
            direction: Some("inbound"),
            message_type: Some("attachment".to_string()),
            text: entry
                .get("file_name")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        _ => ProjectedEntry {
            role: None,
            direction: Some("system"),
            message_type: None,
            text: None,
        },
    }
}

fn agent_label(value: Option<&Value>) -> Option<String> {
    let value = value?;
    value
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))
        .map(str::to_owned)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EntryIdParts {
    pub turn_ordinal: Option<i64>,
    pub turn_token: Option<String>,
    pub suffix: Option<String>,
    pub suffix_index: Option<i64>,
}

/// Entry ids look like `t10u`, `t121s3`, `tbs0`, `event-<uuid>`, `fb-<uuid>`.
/// The turn token is not always numeric, so `turn_ordinal` is best-effort and
/// ordering falls back to `source_order`.
pub(crate) fn parse_entry_id(entry_id: &str) -> EntryIdParts {
    let Some(rest) = entry_id.strip_prefix('t') else {
        return EntryIdParts::default();
    };
    if rest.is_empty() || entry_id.contains('-') {
        return EntryIdParts::default();
    }

    // Parse from the right: trailing digits are the suffix index, the
    // alphabetic run before them is the suffix, and whatever precedes it is the
    // turn token — which is not always numeric (`tbs0`).
    let digits_start = rest
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + c_len(rest, i));
    let (head, index_text) = rest.split_at(digits_start);
    let head = if head.is_empty() { rest } else { head };
    let index_text = if head == rest { "" } else { index_text };

    let alpha_start = head
        .rfind(|c: char| !c.is_ascii_alphabetic())
        .map_or(0, |i| i + c_len(head, i));
    let (turn, suffix) = head.split_at(alpha_start);

    // An all-alphabetic head means the turn token itself is a letter: keep the
    // first character as the turn and the remainder as the suffix.
    let (turn, suffix) = if turn.is_empty() && suffix.len() > 1 {
        suffix.split_at(1)
    } else {
        (turn, suffix)
    };

    EntryIdParts {
        turn_ordinal: turn.parse::<i64>().ok(),
        turn_token: (!turn.is_empty()).then(|| turn.to_string()),
        suffix: (!suffix.is_empty()).then(|| suffix.to_string()),
        suffix_index: index_text.parse::<i64>().ok(),
    }
}

fn c_len(s: &str, byte_index: usize) -> usize {
    s[byte_index..].chars().next().map_or(1, char::len_utf8)
}

fn refresh_bot_rollups(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE grok_bots SET
             entry_count = COALESCE((
                 SELECT COUNT(*) FROM grok_entries WHERE grok_entries.bot_id = grok_bots.bot_id
             ), 0),
             first_entry_at = (
                 SELECT MIN(timestamp) FROM grok_entries
                  WHERE grok_entries.bot_id = grok_bots.bot_id AND timestamp IS NOT NULL
             ),
             last_entry_at = (
                 SELECT MAX(timestamp) FROM grok_entries
                  WHERE grok_entries.bot_id = grok_bots.bot_id AND timestamp IS NOT NULL
             )",
        [],
    )?;
    Ok(())
}

fn record_ingest_error(
    conn: &Connection,
    source: &str,
    bot_id: Option<&str>,
    entry_id: Option<&str>,
    error_kind: &str,
    message: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO grok_ingest_errors (
             source, error_kind, bot_id, entry_id, message,
             occurrences, first_observed_at, observed_at)
         VALUES (?1,?2,?3,?4,?5,1,?6,?6)
         ON CONFLICT(source, error_kind) DO UPDATE SET
             bot_id = excluded.bot_id,
             entry_id = excluded.entry_id,
             message = excluded.message,
             occurrences = grok_ingest_errors.occurrences + 1,
             observed_at = excluded.observed_at",
        params![source, error_kind, bot_id, entry_id, message, now],
    )?;
    Ok(())
}

fn epoch_ms_to_rfc3339(ms: Option<i64>) -> Option<String> {
    let ms = ms?;
    DateTime::<Utc>::from_timestamp_millis(ms).map(|dt| dt.to_rfc3339())
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO index_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn bump_counter(conn: &Connection, key: &str, delta: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO index_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET
             value = CAST(CAST(index_meta.value AS INTEGER) + ?2 AS TEXT)",
        params![key, delta.to_string()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Schema and migration
// ---------------------------------------------------------------------------

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS grok_bots (
            bot_id TEXT PRIMARY KEY,
            account_id TEXT,
            name TEXT,
            title TEXT,
            description TEXT,
            is_group INTEGER NOT NULL DEFAULT 0,
            member_ids_json TEXT,
            origin TEXT,
            remote_store_path TEXT,
            created_at TEXT,
            updated_at TEXT,
            last_activity_at TEXT,
            last_viewed_at TEXT,
            newest_entry_id TEXT,
            last_message_id TEXT,
            last_entry_kind TEXT,
            last_entry_text TEXT,
            unread_count INTEGER,
            has_unread INTEGER,
            awaiting_user_response INTEGER,
            roster_json TEXT NOT NULL DEFAULT '{}',
            roster_present INTEGER NOT NULL DEFAULT 1,
            entry_count INTEGER NOT NULL DEFAULT 0,
            first_entry_at TEXT,
            last_entry_at TEXT,
            local_replica_path TEXT,
            local_persisted_at TEXT,
            gateway_backfilled_to_seq INTEGER,
            gateway_complete INTEGER NOT NULL DEFAULT 0,
            gateway_synced_at TEXT
        );

        CREATE TABLE IF NOT EXISTS grok_entries (
            bot_id TEXT NOT NULL REFERENCES grok_bots(bot_id) ON DELETE CASCADE,
            entry_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            role TEXT,
            direction TEXT,
            turn_ordinal INTEGER,
            turn_token TEXT,
            entry_suffix TEXT,
            suffix_index INTEGER,
            timestamp TEXT,
            timestamp_ms INTEGER NOT NULL,
            message_type TEXT,
            text TEXT,
            rich_text_json TEXT,
            request_id TEXT,
            client_nonce TEXT,
            batch_id TEXT,
            from_agent TEXT,
            to_agent TEXT,
            author TEXT,
            event_type TEXT,
            is_streaming INTEGER,
            raw_json TEXT NOT NULL,
            provenance TEXT NOT NULL,
            source_seq INTEGER,
            source_order INTEGER NOT NULL,
            source_path TEXT,
            PRIMARY KEY (bot_id, entry_id)
        );

        -- Keyed rather than append-only: a permanently broken blob is retried on
        -- every query, and an unbounded error log would be the result.
        CREATE TABLE IF NOT EXISTS grok_ingest_errors (
            source TEXT NOT NULL,
            error_kind TEXT NOT NULL,
            bot_id TEXT,
            entry_id TEXT,
            message TEXT NOT NULL,
            occurrences INTEGER NOT NULL DEFAULT 1,
            first_observed_at TEXT NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY (source, error_kind)
        );

        CREATE TABLE IF NOT EXISTS source_blobs (
            blob_path TEXT PRIMARY KEY,
            logical_key TEXT NOT NULL,
            slice TEXT NOT NULL,
            bot_id TEXT,
            account_id TEXT,
            schema_version INTEGER,
            size INTEGER NOT NULL,
            modified_ns INTEGER NOT NULL,
            persisted_at_ms INTEGER,
            last_seq INTEGER,
            entry_count INTEGER NOT NULL DEFAULT 0,
            ingested_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_grok_entries_bot_time
            ON grok_entries(bot_id, timestamp_ms);
        CREATE INDEX IF NOT EXISTS idx_grok_entries_kind
            ON grok_entries(kind);
        CREATE INDEX IF NOT EXISTS idx_grok_entries_timestamp
            ON grok_entries(timestamp);
        ",
    )?;
    Ok(())
}

/// Rebuild the schema and re-project every entry from its stored `raw_json`.
///
/// This is why `raw_json` is mandatory on every row: it makes each future
/// schema version rebuildable offline, without re-reading blobs that may have
/// since been truncated or a gateway that may be unreachable.
fn migrate_by_reprojection(conn: &mut Connection, cache_path: &Path) -> Result<()> {
    // Probe first so a pre-v1 or damaged cache falls back to a clean rebuild
    // without holding a borrow across the drop.
    if conn
        .prepare("SELECT bot_id, raw_json, provenance, source_seq, source_order FROM grok_entries")
        .is_err()
    {
        return rebuild_from_scratch(cache_path);
    }

    #[allow(clippy::type_complexity)]
    let preserved: Vec<(String, String, String, Option<i64>, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT bot_id, raw_json, provenance, source_seq, source_order FROM grok_entries",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    let bots: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT bot_id, roster_json FROM grok_bots")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    conn.execute_batch(
        "DROP TABLE IF EXISTS grok_entries;
         DROP TABLE IF EXISTS grok_bots;
         DROP TABLE IF EXISTS grok_ingest_errors;
         DROP TABLE IF EXISTS source_blobs;
         DROP TABLE IF EXISTS index_meta;",
    )?;
    create_schema(conn)?;

    let tx = conn.transaction()?;
    for (bot_id, roster_json) in bots {
        tx.execute(
            "INSERT INTO grok_bots (bot_id, roster_json, roster_present)
             VALUES (?1, ?2, 0)
             ON CONFLICT(bot_id) DO NOTHING",
            params![bot_id, roster_json],
        )?;
    }
    for (bot_id, raw_json, provenance, source_seq, source_order) in preserved {
        ensure_bot_row(&tx, &bot_id)?;
        let Ok(entry) = serde_json::from_str::<Value>(&raw_json) else {
            continue;
        };
        upsert_entry(
            &tx,
            &bot_id,
            &entry,
            source_order,
            &provenance,
            source_seq,
            None,
        )?;
    }
    refresh_bot_rollups(&tx)?;
    tx.commit()?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn rebuild_from_scratch(cache_path: &Path) -> Result<()> {
    remove_cache_files(cache_path)?;
    let conn = open_cache_connection(cache_path)?;
    ensure_wal_mode(&conn)?;
    create_schema(&conn)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cache plumbing (mirrors codex_index)
// ---------------------------------------------------------------------------

fn open_cache_connection(cache_path: &Path) -> Result<Connection> {
    let conn = Connection::open(cache_path)?;
    set_private_file_permissions(cache_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(30))?;
    set_cache_sidecar_permissions(cache_path)?;
    Ok(conn)
}

fn ensure_wal_mode(conn: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        let result = (|| -> rusqlite::Result<()> {
            let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
            if !mode.eq_ignore_ascii_case("wal") {
                conn.pragma_update(None, "journal_mode", "WAL")?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if is_busy_sql_error(&error) && started.elapsed() < Duration::from_secs(30) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn is_busy_sql_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn is_corrupt_cache_error(error: &crate::Error) -> bool {
    matches!(error, crate::Error::Sql(error) if is_corrupt_sql_error(error))
}

fn is_corrupt_sql_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

fn remove_cache_files(cache_path: &Path) -> Result<()> {
    for path in [
        cache_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", cache_path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", cache_path.to_string_lossy())),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn set_cache_sidecar_permissions(cache_path: &Path) -> Result<()> {
    for path in [
        PathBuf::from(format!("{}-wal", cache_path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", cache_path.to_string_lossy())),
    ] {
        if path.exists() {
            set_private_file_permissions(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop app's naming scheme, kept here so the round-trip test does
    /// not require production code that nothing else calls.
    fn encode_blob_key(key: &str) -> String {
        BASE32_NOPAD.encode(key.as_bytes()).to_lowercase()
    }

    #[test]
    fn decodes_a_real_blob_filename() {
        let name = "onqw4zbomnwgszlooqxhg3djmnss4yldmnxxk3tufztws5diovrckn2dgq3timjugu2c45dsmfxhgy3snfyhiltsmvygy2ldmfzs4mdbha2ggnrwguwwiytbgywtimbxguwweytegiwtczrxhfrtaolbmu4tony";
        let key = decode_blob_key(name).expect("decodes");
        assert!(key.starts_with("sand.client.slice.account."));
        assert!(key.contains(".transcript.replicas."));
    }

    #[test]
    fn encode_and_decode_round_trip() {
        let key = "sand.client.slice.account.github%7C4741454.roster.last-roster";
        assert_eq!(decode_blob_key(&encode_blob_key(key)).as_deref(), Some(key));
    }

    #[test]
    fn classifies_transcript_and_roster_keys() {
        let (slice, bot, account) = classify_key(
            "sand.client.slice.account.github%7C4741454.transcript.replicas.69884c90-a572-4638-9b6d-5753ea878c88",
        );
        assert_eq!(slice, SLICE_TRANSCRIPT);
        assert_eq!(bot.as_deref(), Some("69884c90-a572-4638-9b6d-5753ea878c88"));
        assert_eq!(account.as_deref(), Some("github%7C4741454"));

        let (slice, bot, account) =
            classify_key("sand.client.slice.account.github%7C4741454.roster.last-roster");
        assert_eq!(slice, SLICE_ROSTER);
        assert!(bot.is_none());
        assert_eq!(account.as_deref(), Some("github%7C4741454"));

        let (slice, _, _) = classify_key("sand.client.slice.ui-layout");
        assert_eq!(slice, SLICE_OTHER);
    }

    #[test]
    fn rejects_undecodable_filenames() {
        assert!(decode_blob_key("not-base32!!").is_none());
    }

    #[test]
    fn parses_ordinal_entry_ids() {
        let parts = parse_entry_id("t121s3");
        assert_eq!(parts.turn_ordinal, Some(121));
        assert_eq!(parts.turn_token.as_deref(), Some("121"));
        assert_eq!(parts.suffix.as_deref(), Some("s"));
        assert_eq!(parts.suffix_index, Some(3));

        let parts = parse_entry_id("t10u");
        assert_eq!(parts.turn_ordinal, Some(10));
        assert_eq!(parts.suffix.as_deref(), Some("u"));
        assert_eq!(parts.suffix_index, None);
    }

    #[test]
    fn tolerates_non_numeric_and_uuid_entry_ids() {
        let parts = parse_entry_id("tbs0");
        assert_eq!(parts.turn_ordinal, None);
        assert_eq!(parts.turn_token.as_deref(), Some("b"));
        assert_eq!(parts.suffix.as_deref(), Some("s"));

        let parts = parse_entry_id("event-15e92eb2-0000-0000-0000-000000000000");
        assert_eq!(parts, EntryIdParts::default());

        let parts = parse_entry_id("fb-15e92eb2-0000-0000-0000-000000000000");
        assert_eq!(parts, EntryIdParts::default());
    }

    #[test]
    fn projects_text_only_for_text_shaped_entries() {
        let widget = serde_json::json!({
            "kind": "send-message",
            "message": {"type": "widget", "content": {"nested": true}}
        });
        assert!(project_entry(&widget, "send-message").text.is_none());

        let text = serde_json::json!({
            "kind": "send-message",
            "message": {"type": "text", "content": "hello"}
        });
        assert_eq!(
            project_entry(&text, "send-message").text.as_deref(),
            Some("hello")
        );

        let unknown = serde_json::json!({"kind": "brand-new-kind"});
        let projected = project_entry(&unknown, "brand-new-kind");
        assert!(projected.text.is_none());
        assert_eq!(projected.direction, Some("system"));
    }
}
