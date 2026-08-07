//! Read-only macOS Unified Log virtual table.

use rusqlite::vtab::{
    eponymous_only_module, sqlite3_vtab, sqlite3_vtab_cursor, Context, IndexConstraintOp,
    IndexConstraintUsage, IndexInfo, VTab, VTabConfig, VTabConnection, VTabCursor, Values,
};
use rusqlite::{Connection, Error, Result};
use serde_json::Value;
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_LAST: &str = "15m";
const DEFAULT_MAX_ROWS: usize = 50_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const COL_TIMESTAMP: c_int = 0;
const COL_EVENT_TYPE: c_int = 1;
const COL_SUBSYSTEM: c_int = 2;
const COL_CATEGORY: c_int = 3;
const COL_PROCESS: c_int = 4;
const COL_PROCESS_ID: c_int = 5;
const COL_THREAD_ID: c_int = 6;
const COL_SENDER: c_int = 7;
const COL_MESSAGE: c_int = 8;
const COL_LEVEL: c_int = 9;
const COL_ACTIVITY_ID: c_int = 10;
const COL_TRACE_ID: c_int = 11;
const COL_BOOT_UUID: c_int = 12;
const COL_RAW_JSON: c_int = 13;
const COL_PROVENANCE: c_int = 14;
const COL_SOURCE: c_int = 15;
const COL_ARCHIVE_PATH: c_int = 16;
const COL_SOURCE_ORDER: c_int = 17;
const COL_TIMESTAMP_MS: c_int = 18;
const COL_FORMAT_STRING: c_int = 19;
const COL_PROCESS_IMAGE_PATH: c_int = 20;
const COL_SENDER_IMAGE_PATH: c_int = 21;
const COL_PARENT_ACTIVITY_ID: c_int = 22;
const COL_SIGNPOST_ID: c_int = 23;
const COL_SIGNPOST_NAME: c_int = 24;
const COL_SIGNPOST_TYPE: c_int = 25;
const COL_SIGNPOST_SCOPE: c_int = 26;
const COL_USER_ID: c_int = 27;
const COL_MACH_TIMESTAMP: c_int = 28;
const COL_START: c_int = 29;
const COL_END: c_int = 30;
const COL_PREDICATE: c_int = 31;
const COL_ARCHIVE: c_int = 32;
const COL_LEVEL_FILTER: c_int = 33;
const COL_MAX_ROWS: c_int = 34;
const COL_TIMEOUT: c_int = 35;

#[derive(Clone, Debug)]
pub struct MacosLogConfig {
    pub last: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub predicate: Option<String>,
    pub archive: Option<String>,
    pub level: String,
    pub max_rows: usize,
    pub timeout: Duration,
}

#[derive(Clone, Default)]
pub struct MacosLogStats(Arc<Mutex<ScanStats>>);

#[derive(Default)]
struct ScanStats {
    rows: usize,
    truncated_reason: Option<String>,
}

impl MacosLogStats {
    pub fn truncation(&self) -> Option<(usize, String)> {
        let stats = self.0.lock().ok()?;
        stats
            .truncated_reason
            .as_ref()
            .map(|reason| (stats.rows, reason.clone()))
    }

    fn reset(&self) {
        if let Ok(mut stats) = self.0.lock() {
            *stats = ScanStats::default();
        }
    }

    fn record_row(&self, rows: usize) {
        if let Ok(mut stats) = self.0.lock() {
            stats.rows = rows;
        }
    }

    fn truncate(&self, rows: usize, reason: impl Into<String>) {
        if let Ok(mut stats) = self.0.lock() {
            stats.rows = rows;
            stats.truncated_reason = Some(reason.into());
        }
    }
}

#[derive(Clone)]
struct MacosLogModule {
    config: MacosLogConfig,
    stats: MacosLogStats,
}

impl Default for MacosLogConfig {
    fn default() -> Self {
        Self {
            last: None,
            start: None,
            end: None,
            predicate: None,
            archive: None,
            level: "standard".to_string(),
            max_rows: DEFAULT_MAX_ROWS,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

pub fn register(conn: &Connection, config: MacosLogConfig) -> Result<MacosLogStats> {
    let stats = MacosLogStats::default();
    conn.create_module(
        "macos_logs",
        eponymous_only_module::<MacosLogsTable>(),
        Some(MacosLogModule {
            config,
            stats: stats.clone(),
        }),
    )?;
    Ok(stats)
}

#[repr(C)]
struct MacosLogsTable {
    base: sqlite3_vtab,
    config: MacosLogConfig,
    stats: MacosLogStats,
}

unsafe impl<'vtab> VTab<'vtab> for MacosLogsTable {
    type Aux = MacosLogModule;
    type Cursor = MacosLogsCursor<'vtab>;

    fn connect(
        db: &mut VTabConnection,
        aux: Option<&MacosLogModule>,
        _args: &[&[u8]],
    ) -> Result<(String, Self)> {
        db.config(VTabConfig::DirectOnly)?;
        Ok((
            "CREATE TABLE x(
                timestamp TEXT,
                event_type TEXT,
                subsystem TEXT,
                category TEXT,
                process TEXT,
                process_id INTEGER,
                thread_id INTEGER,
                sender TEXT,
                message TEXT,
                level TEXT,
                activity_id INTEGER,
                trace_id INTEGER,
                boot_uuid TEXT,
                raw_json TEXT,
                provenance TEXT,
                source TEXT,
                archive_path TEXT,
                source_order INTEGER,
                timestamp_ms INTEGER,
                format_string TEXT,
                process_image_path TEXT,
                sender_image_path TEXT,
                parent_activity_id INTEGER,
                signpost_id TEXT,
                signpost_name TEXT,
                signpost_type TEXT,
                signpost_scope TEXT,
                user_id INTEGER,
                mach_timestamp INTEGER,
                start HIDDEN,
                end HIDDEN,
                predicate HIDDEN,
                archive HIDDEN,
                level_filter HIDDEN,
                max_rows HIDDEN,
                timeout HIDDEN
            )"
            .to_string(),
            Self {
                base: sqlite3_vtab::default(),
                config: aux.map(|aux| aux.config.clone()).unwrap_or_default(),
                stats: aux.map(|aux| aux.stats.clone()).unwrap_or_default(),
            },
        ))
    }

    fn best_index(&self, info: &mut IndexInfo) -> Result<()> {
        let mut argv_index = 1;
        let mut keys = Vec::new();
        let has_order_by = info.num_of_order_by() > 0;

        for (constraint, mut usage) in info.constraints_and_usages() {
            if !constraint.is_usable() {
                continue;
            }

            match (constraint.column(), constraint.operator()) {
                (COL_START, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "start", true);
                }
                (COL_END, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "end", true);
                }
                (COL_PREDICATE, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "predicate", true);
                }
                (COL_ARCHIVE, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "archive", true);
                }
                (COL_LEVEL_FILTER, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "level_filter", true);
                }
                (COL_MAX_ROWS, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "max_rows", true);
                }
                (COL_TIMEOUT, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "timeout", true);
                }
                (COL_TIMESTAMP, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GE)
                | (COL_TIMESTAMP, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GT) => {
                    push_arg(
                        &mut usage,
                        &mut argv_index,
                        &mut keys,
                        "timestamp_start",
                        false,
                    );
                }
                (COL_TIMESTAMP, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LE)
                | (COL_TIMESTAMP, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LT) => {
                    push_arg(
                        &mut usage,
                        &mut argv_index,
                        &mut keys,
                        "timestamp_end",
                        false,
                    );
                }
                (COL_PROCESS, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "process", false);
                }
                (COL_PROCESS_ID, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "process_id", false);
                }
                (COL_SUBSYSTEM, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "subsystem", false);
                }
                (COL_CATEGORY, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "category", false);
                }
                (COL_LEVEL, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "log_type", false);
                }
                (COL_EVENT_TYPE, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "event_type", false);
                }
                (COL_MESSAGE, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ) => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "message", false);
                }
                (COL_MESSAGE, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LIKE) => {
                    push_arg(
                        &mut usage,
                        &mut argv_index,
                        &mut keys,
                        "message_like",
                        false,
                    );
                }
                (_, IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LIMIT) if !has_order_by => {
                    push_arg(&mut usage, &mut argv_index, &mut keys, "limit", false);
                }
                _ => {}
            }
        }

        info.set_idx_str(&keys.join(","));
        info.set_estimated_cost(100.0);
        info.set_estimated_rows(DEFAULT_MAX_ROWS as i64);
        Ok(())
    }

    fn open(&'vtab mut self) -> Result<Self::Cursor> {
        Ok(MacosLogsCursor::new(
            self.config.clone(),
            self.stats.clone(),
        ))
    }
}

fn push_arg(
    usage: &mut IndexConstraintUsage<'_>,
    argv_index: &mut c_int,
    keys: &mut Vec<&'static str>,
    key: &'static str,
    omit: bool,
) {
    usage.set_argv_index(*argv_index);
    usage.set_omit(omit);
    *argv_index += 1;
    keys.push(key);
}

#[derive(Clone, Debug)]
struct QueryOptions {
    last: Option<String>,
    start: Option<String>,
    end: Option<String>,
    predicate: Option<String>,
    archive: Option<String>,
    level_filter: Option<String>,
    max_rows: usize,
    timeout: Duration,
    pushed_predicates: Vec<String>,
    query_limit: Option<usize>,
}

impl From<MacosLogConfig> for QueryOptions {
    fn from(config: MacosLogConfig) -> Self {
        Self {
            last: config.last,
            start: config.start,
            end: config.end,
            predicate: config.predicate,
            archive: config.archive,
            level_filter: Some(config.level),
            max_rows: config.max_rows,
            timeout: config.timeout,
            pushed_predicates: Vec::new(),
            query_limit: None,
        }
    }
}

#[derive(Clone, Debug)]
struct LogRow {
    raw_json: String,
    parsed: Option<Value>,
}

impl LogRow {
    fn parse(raw_json: String) -> Result<Option<Self>> {
        if raw_json.trim().is_empty() {
            return Ok(None);
        }
        let parsed: Value = serde_json::from_str(&raw_json)
            .map_err(|error| Error::ModuleError(format!("invalid macOS log NDJSON: {error}")))?;
        if parsed.get("finished").is_some() && parsed.get("count").is_some() {
            return Ok(None);
        }
        if !parsed.is_object() {
            return Err(Error::ModuleError(
                "macOS log NDJSON record was not an object".to_string(),
            ));
        }
        Ok(Some(Self {
            raw_json,
            parsed: Some(parsed),
        }))
    }

    fn text(&self, keys: &[&str]) -> Option<&str> {
        let value = self.parsed.as_ref()?;
        keys.iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
    }

    fn integer(&self, keys: &[&str]) -> Option<i64> {
        let value = self.parsed.as_ref()?;
        keys.iter().find_map(|key| {
            value
                .get(*key)
                .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        })
    }

    fn timestamp_ms(&self) -> Option<i64> {
        let timestamp = self.text(&["timestamp"])?;
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .or_else(|_| chrono::DateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S%.f%z"))
            .ok()
            .map(|value| value.timestamp_millis())
    }
}

#[repr(C)]
struct MacosLogsCursor<'vtab> {
    base: sqlite3_vtab_cursor,
    row_id: i64,
    current: Option<LogRow>,
    stream: Option<LogStream>,
    max_rows: usize,
    emitted_rows: usize,
    deadline: Instant,
    scan_timeout: Duration,
    eof: bool,
    config: MacosLogConfig,
    archive_path: Option<String>,
    stats: MacosLogStats,
    cap_is_truncation: bool,
    phantom: PhantomData<&'vtab MacosLogsTable>,
}

impl MacosLogsCursor<'_> {
    fn new<'vtab>(config: MacosLogConfig, stats: MacosLogStats) -> MacosLogsCursor<'vtab> {
        MacosLogsCursor {
            base: sqlite3_vtab_cursor::default(),
            row_id: 0,
            current: None,
            stream: None,
            max_rows: DEFAULT_MAX_ROWS,
            emitted_rows: 0,
            deadline: Instant::now() + DEFAULT_TIMEOUT,
            scan_timeout: DEFAULT_TIMEOUT,
            eof: true,
            config,
            archive_path: None,
            stats,
            cap_is_truncation: true,
            phantom: PhantomData,
        }
    }

    fn advance(&mut self) -> Result<()> {
        if self.emitted_rows >= self.max_rows {
            if self.cap_is_truncation {
                self.stats.truncate(
                    self.emitted_rows,
                    format!("row limit {} reached", self.max_rows),
                );
            }
            self.eof = true;
            self.current = None;
            self.stream.take();
            return Ok(());
        }

        let Some(stream) = self.stream.as_mut() else {
            self.eof = true;
            self.current = None;
            return Ok(());
        };

        loop {
            match stream.recv_line(self.deadline)? {
                StreamRead::Line(line) => {
                    let Some(row) = LogRow::parse(line)? else {
                        continue;
                    };
                    self.row_id += 1;
                    self.emitted_rows += 1;
                    self.stats.record_row(self.emitted_rows);
                    self.current = Some(row);
                    self.eof = false;
                    break;
                }
                StreamRead::Done => {
                    self.eof = true;
                    self.current = None;
                    self.stream.take();
                    break;
                }
                StreamRead::TimedOut => {
                    self.stats.truncate(
                        self.emitted_rows,
                        format!("timeout after {} seconds", self.scan_timeout.as_secs()),
                    );
                    self.eof = true;
                    self.current = None;
                    self.stream.take();
                    break;
                }
            }
        }
        Ok(())
    }
}

unsafe impl VTabCursor for MacosLogsCursor<'_> {
    fn filter(&mut self, _idx_num: c_int, idx_str: Option<&str>, args: &Values<'_>) -> Result<()> {
        let options = options_from_filter(self.config.clone(), idx_str.unwrap_or_default(), args)?;
        self.stats.reset();
        self.max_rows = options.max_rows;
        self.cap_is_truncation = options
            .query_limit
            .is_none_or(|query_limit| options.max_rows < query_limit);
        self.emitted_rows = 0;
        self.row_id = 0;
        self.deadline = Instant::now() + options.timeout;
        self.scan_timeout = options.timeout;
        self.archive_path = options.archive.clone();
        self.current = None;
        if self.max_rows == 0 {
            self.stream = None;
            self.eof = true;
            return Ok(());
        }
        self.stream = Some(LogStream::spawn(options)?);
        self.eof = false;
        self.advance()
    }

    fn next(&mut self) -> Result<()> {
        self.advance()
    }

    fn eof(&self) -> bool {
        self.eof
    }

    fn column(&self, ctx: &mut Context, i: c_int) -> Result<()> {
        let Some(row) = self.current.as_ref() else {
            return Ok(());
        };

        match i {
            COL_TIMESTAMP => set_text(ctx, row.text(&["timestamp"])),
            COL_EVENT_TYPE => set_text(ctx, row.text(&["eventType", "event_type"])),
            COL_SUBSYSTEM => set_text(ctx, row.text(&["subsystem"])),
            COL_CATEGORY => set_text(ctx, row.text(&["category"])),
            COL_PROCESS => {
                if let Some(process) = row.text(&["process"]) {
                    ctx.set_result(&process)
                } else {
                    set_basename(ctx, row.text(&["processImagePath"]))
                }
            }
            COL_PROCESS_ID => set_integer(ctx, row.integer(&["processID", "process_id"])),
            COL_THREAD_ID => set_integer(ctx, row.integer(&["threadID", "thread_id"])),
            COL_SENDER => {
                if let Some(sender) = row.text(&["sender"]) {
                    ctx.set_result(&sender)
                } else {
                    set_basename(ctx, row.text(&["senderImagePath"]))
                }
            }
            COL_MESSAGE => set_text(
                ctx,
                row.text(&["eventMessage", "message", "composedMessage"]),
            ),
            COL_LEVEL => set_text(ctx, row.text(&["messageType", "level"])),
            COL_ACTIVITY_ID => {
                set_integer(ctx, row.integer(&["activityIdentifier", "activity_id"]))
            }
            COL_TRACE_ID => set_integer(ctx, row.integer(&["traceID", "trace_id"])),
            COL_BOOT_UUID => set_text(ctx, row.text(&["bootUUID", "boot_uuid"])),
            COL_RAW_JSON => ctx.set_result(&row.raw_json),
            COL_PROVENANCE => ctx.set_result(&"macos_unified_log"),
            COL_SOURCE => ctx.set_result(&if self.archive_path.is_some() {
                "archive"
            } else {
                "live"
            }),
            COL_ARCHIVE_PATH => set_text(ctx, self.archive_path.as_deref()),
            COL_SOURCE_ORDER => ctx.set_result(&(self.row_id - 1)),
            COL_TIMESTAMP_MS => set_integer(ctx, row.timestamp_ms()),
            COL_FORMAT_STRING => set_text(ctx, row.text(&["formatString"])),
            COL_PROCESS_IMAGE_PATH => set_text(ctx, row.text(&["processImagePath"])),
            COL_SENDER_IMAGE_PATH => set_text(ctx, row.text(&["senderImagePath"])),
            COL_PARENT_ACTIVITY_ID => set_integer(ctx, row.integer(&["parentActivityIdentifier"])),
            COL_SIGNPOST_ID => set_identifier(ctx, row.parsed.as_ref(), &["signpostIdentifier"]),
            COL_SIGNPOST_NAME => set_text(ctx, row.text(&["signpostName"])),
            COL_SIGNPOST_TYPE => set_text(ctx, row.text(&["signpostType"])),
            COL_SIGNPOST_SCOPE => set_text(ctx, row.text(&["signpostScope"])),
            COL_USER_ID => set_integer(ctx, row.integer(&["userID"])),
            COL_MACH_TIMESTAMP => set_integer(ctx, row.integer(&["machTimestamp"])),
            _ => Ok(()),
        }
    }

    fn rowid(&self) -> Result<i64> {
        Ok(self.row_id)
    }
}

fn set_text(ctx: &mut Context, value: Option<&str>) -> Result<()> {
    match value {
        Some(value) => ctx.set_result(&value),
        None => Ok(()),
    }
}

fn set_integer(ctx: &mut Context, value: Option<i64>) -> Result<()> {
    match value {
        Some(value) => ctx.set_result(&value),
        None => Ok(()),
    }
}

fn set_basename(ctx: &mut Context, value: Option<&str>) -> Result<()> {
    set_text(
        ctx,
        value.and_then(|path| std::path::Path::new(path).file_name()?.to_str()),
    )
}

fn set_identifier(ctx: &mut Context, value: Option<&Value>, keys: &[&str]) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    let Some(value) = keys.iter().find_map(|key| value.get(*key)) else {
        return Ok(());
    };
    if let Some(text) = value.as_str() {
        ctx.set_result(&text)
    } else if let Some(number) = value.as_u64() {
        ctx.set_result(&number.to_string())
    } else if let Some(number) = value.as_i64() {
        ctx.set_result(&number.to_string())
    } else {
        Ok(())
    }
}

fn options_from_filter(
    config: MacosLogConfig,
    idx_str: &str,
    args: &Values<'_>,
) -> Result<QueryOptions> {
    let mut options = QueryOptions::from(config);
    for (idx, key) in idx_str.split(',').filter(|key| !key.is_empty()).enumerate() {
        match key {
            "start" => options.start = Some(args.get(idx)?),
            "end" => options.end = Some(args.get(idx)?),
            "predicate" => options.predicate = Some(args.get(idx)?),
            "archive" => options.archive = Some(args.get(idx)?),
            "level_filter" => options.level_filter = Some(args.get(idx)?),
            "max_rows" => {
                let value = integer_arg(args, idx)?;
                options.max_rows = usize::try_from(value).unwrap_or_default();
            }
            "limit" => {
                let value = integer_arg(args, idx)?;
                if value >= 0 {
                    options.max_rows = options.max_rows.min(value as usize);
                    options.query_limit = Some(value as usize);
                }
            }
            "timeout" => {
                let value = integer_arg(args, idx)?;
                options.timeout = Duration::from_secs(u64::try_from(value).unwrap_or_default());
            }
            "timestamp_start" if options.start.is_none() => {
                options.start = Some(args.get(idx)?);
            }
            "timestamp_end" if options.end.is_none() => {
                options.end = Some(args.get(idx)?);
            }
            "process" => push_string_predicate(&mut options, "process", args.get(idx)?),
            "process_id" => {
                let value = integer_arg(args, idx)?;
                options
                    .pushed_predicates
                    .push(format!("processIdentifier == {value}"));
            }
            "subsystem" => push_string_predicate(&mut options, "subsystem", args.get(idx)?),
            "category" => push_string_predicate(&mut options, "category", args.get(idx)?),
            "log_type" => push_string_predicate(&mut options, "logType", args.get(idx)?),
            "event_type" => push_string_predicate(&mut options, "type", args.get(idx)?),
            "message" => push_string_predicate(&mut options, "composedMessage", args.get(idx)?),
            "message_like" => {
                let pattern: String = args.get(idx)?;
                if let Some(literal) = contains_literal(&pattern) {
                    options.pushed_predicates.push(format!(
                        "composedMessage CONTAINS {}",
                        predicate_string(literal)
                    ));
                }
            }
            _ => {}
        }
    }
    validate_options(&options)?;
    Ok(options)
}

fn push_string_predicate(options: &mut QueryOptions, field: &str, value: String) {
    options
        .pushed_predicates
        .push(format!("{field} == {}", predicate_string(&value)));
}

fn predicate_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

fn contains_literal(pattern: &str) -> Option<&str> {
    let literal = pattern.strip_prefix('%')?.strip_suffix('%')?;
    (!literal.is_empty() && !literal.contains(['%', '_', '*', '?'])).then_some(literal)
}

fn validate_options(options: &QueryOptions) -> Result<()> {
    if options.last.is_some() && options.start.is_some() {
        return Err(Error::ModuleError(
            "macos_logs accepts either last or start/end, not both".to_string(),
        ));
    }
    if options.start.is_some() != options.end.is_some() {
        return Err(Error::ModuleError(
            "macos_logs requires start and end together".to_string(),
        ));
    }
    if !((1..=1_000_000).contains(&options.max_rows)
        || options.max_rows == 0 && options.query_limit == Some(0))
    {
        return Err(Error::ModuleError(
            "macos_logs max_rows must be between 1 and 1000000".to_string(),
        ));
    }
    if !(Duration::from_secs(1)..=Duration::from_secs(300)).contains(&options.timeout) {
        return Err(Error::ModuleError(
            "macos_logs timeout must be between 1 and 300 seconds".to_string(),
        ));
    }
    if let Some(last) = options.last.as_deref() {
        let valid = last == "boot"
            || last
                .strip_suffix(['s', 'm', 'h', 'd'])
                .and_then(|number| number.parse::<u64>().ok())
                .is_some_and(|number| number > 0);
        if !valid {
            return Err(Error::ModuleError(
                "macos_logs last must be boot or a positive number followed by s, m, h, or d"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn integer_arg(args: &Values<'_>, idx: usize) -> Result<i64> {
    args.get(idx).or_else(|_| {
        let value: String = args.get(idx)?;
        value
            .parse()
            .map_err(|_| Error::ModuleError("macos_logs expected integer option".to_string()))
    })
}

enum StreamEvent {
    Line(String),
    StdoutDone,
}

enum StreamRead {
    Line(String),
    Done,
    TimedOut,
}

struct LogStream {
    receiver: Option<Receiver<StreamEvent>>,
    child: Child,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr: Arc<Mutex<String>>,
}

impl LogStream {
    fn spawn(options: QueryOptions) -> Result<Self> {
        if !platform_supported() {
            return Err(Error::ModuleError(
                "macos_logs is only supported on macOS".to_string(),
            ));
        }

        let mut command = Command::new(log_binary());
        command.arg("show").arg("--style").arg("ndjson");
        if let Some(archive) = options.archive.as_deref() {
            command.arg("--archive").arg(archive);
        }
        match (options.start.as_deref(), options.end.as_deref()) {
            (None, None) => {
                command
                    .arg("--last")
                    .arg(options.last.as_deref().unwrap_or(DEFAULT_LAST));
            }
            (start, end) => {
                if let Some(start) = start {
                    command.arg("--start").arg(start);
                }
                if let Some(end) = end {
                    command.arg("--end").arg(end);
                }
            }
        }
        let mut predicates = options.pushed_predicates;
        if let Some(predicate) = options.predicate {
            predicates.insert(0, format!("({predicate})"));
        }
        if !predicates.is_empty() {
            command.arg("--predicate").arg(
                predicates
                    .into_iter()
                    .map(|item| format!("({item})"))
                    .collect::<Vec<_>>()
                    .join(" AND "),
            );
        }
        if let Some(level) = options.level_filter.as_deref() {
            match level.to_ascii_lowercase().as_str() {
                "debug" => {
                    command.arg("--info");
                    command.arg("--debug");
                }
                "info" => {
                    command.arg("--info");
                }
                "default" | "standard" => {}
                _ => {
                    return Err(Error::ModuleError(format!(
                        "unsupported macos_logs level_filter `{level}`; use standard, info, or debug"
                    )));
                }
            }
        }

        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                Error::ModuleError(format!("failed to run macOS log command: {error}"))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::ModuleError("failed to capture macOS log stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::ModuleError("failed to capture macOS log stderr".to_string()))?;
        let (sender, receiver) = sync_channel(128);
        let stdout_thread = Some(spawn_stdout_reader(stdout, sender));
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let stderr_thread = Some(spawn_stderr_reader(stderr, Arc::clone(&stderr_buf)));

        Ok(Self {
            receiver: Some(receiver),
            child,
            stdout_thread,
            stderr_thread,
            stderr: stderr_buf,
        })
    }

    fn recv_line(&mut self, deadline: Instant) -> Result<StreamRead> {
        let now = Instant::now();
        if now >= deadline {
            self.kill_and_wait();
            return Ok(StreamRead::TimedOut);
        }
        let timeout = deadline.saturating_duration_since(now);
        let Some(receiver) = self.receiver.as_ref() else {
            return Ok(StreamRead::Done);
        };
        match receiver.recv_timeout(timeout) {
            Ok(StreamEvent::Line(line)) => Ok(StreamRead::Line(line)),
            Ok(StreamEvent::StdoutDone) => {
                let status = self.wait_child();
                self.join_threads();
                if let Some(status) = status {
                    if !status.success() {
                        let stderr = self.stderr.lock().map(|s| s.clone()).unwrap_or_default();
                        return Err(Error::ModuleError(format!(
                            "macOS log command failed with {status}: {stderr}"
                        )));
                    }
                }
                Ok(StreamRead::Done)
            }
            Err(RecvTimeoutError::Timeout) => {
                self.kill_and_wait();
                Ok(StreamRead::TimedOut)
            }
            Err(RecvTimeoutError::Disconnected) => Ok(StreamRead::Done),
        }
    }

    fn kill_and_wait(&mut self) {
        let _ = self.receiver.take();
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.join_threads();
    }

    fn wait_child(&mut self) -> Option<std::process::ExitStatus> {
        self.child.wait().ok()
    }

    fn join_threads(&mut self) {
        let _ = self.receiver.take();
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn spawn_stdout_reader(
    stdout: impl std::io::Read + Send + 'static,
    sender: SyncSender<StreamEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if sender.send(StreamEvent::Line(line)).is_err() {
                return;
            }
        }
        let _ = sender.send(StreamEvent::StdoutDone);
    })
}

fn spawn_stderr_reader(
    stderr: impl std::io::Read + Send + 'static,
    output: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        use std::io::Read;
        let mut reader = std::io::BufReader::new(stderr);
        let mut buffer = String::new();
        let _ = reader.read_to_string(&mut buffer);
        if buffer.len() > 4096 {
            buffer.truncate(4096);
        }
        if let Ok(mut output) = output.lock() {
            *output = buffer;
        }
    })
}

fn platform_supported() -> bool {
    cfg!(target_os = "macos") || std::env::var_os("DEVSQL_MACOS_LOG_BIN").is_some()
}

fn log_binary() -> String {
    std::env::var("DEVSQL_MACOS_LOG_BIN").unwrap_or_else(|_| "/usr/bin/log".to_string())
}
