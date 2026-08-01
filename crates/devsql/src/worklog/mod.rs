//! Durable cross-project work memory.
//!
//! Stored in `~/.devsql/worklog.sqlite` (override with `DEVSQL_HOME`).
//! Agents write via `devsql work …`; humans read via `devsql today` / `day`.

use crate::{Error, Result};
use chrono::{Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: i32 = 1;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Resolve the DevSQL home directory (`DEVSQL_HOME` or `~/.devsql`).
pub fn home_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("DEVSQL_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".devsql")
}

/// Path to the durable worklog database.
pub fn db_path() -> PathBuf {
    home_dir().join("worklog.sqlite")
}

// ---------------------------------------------------------------------------
// IDs / time
// ---------------------------------------------------------------------------

fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = Utc::now().timestamp_micros() as u64;
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{t:016x}{c:04x}")
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn today_local() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkTask {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub session_id: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkEvent {
    pub id: String,
    pub task_id: Option<String>,
    pub ts: String,
    pub local_date: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    pub meta_json: Option<String>,
    /// Current task status when the event is linked to a task (for day views).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DayStats {
    pub date: String,
    pub updates: usize,
    pub tasks: usize,
    pub doing: usize,
    pub done: usize,
    pub blocked: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DayView {
    pub date: String,
    pub weekday: String,
    pub stats: DayStats,
    /// Full event feed (newest first).
    pub events: Vec<WorkEvent>,
    /// Rule-based summary bullets for past days.
    pub summary: Vec<String>,
    pub detail: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StartInput {
    pub title: String,
    pub body: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateInput {
    pub task_id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DoneInput {
    pub task_id: String,
    pub title: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NoteInput {
    pub title: String,
    pub body: Option<String>,
    pub task_id: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct Worklog {
    conn: Connection,
}

impl Worklog {
    /// Open (or create) the durable worklog at the default path.
    pub fn open() -> Result<Self> {
        Self::open_at(&db_path())
    }

    /// Open (or create) the durable worklog at an explicit path.
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS work_tasks (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                project     TEXT,
                cwd         TEXT,
                agent       TEXT,
                status      TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                session_id  TEXT,
                source      TEXT NOT NULL DEFAULT 'agent'
            );

            CREATE TABLE IF NOT EXISTS work_events (
                id          TEXT PRIMARY KEY,
                task_id     TEXT,
                ts          TEXT NOT NULL,
                local_date  TEXT NOT NULL,
                kind        TEXT NOT NULL,
                title       TEXT NOT NULL,
                body        TEXT,
                project     TEXT,
                cwd         TEXT,
                agent       TEXT,
                session_id  TEXT,
                meta_json   TEXT,
                FOREIGN KEY (task_id) REFERENCES work_tasks(id)
            );

            CREATE INDEX IF NOT EXISTS idx_work_events_ts ON work_events(ts);
            CREATE INDEX IF NOT EXISTS idx_work_events_local_date ON work_events(local_date);
            CREATE INDEX IF NOT EXISTS idx_work_events_task ON work_events(task_id);
            CREATE INDEX IF NOT EXISTS idx_work_events_project ON work_events(project);
            CREATE INDEX IF NOT EXISTS idx_work_tasks_status ON work_tasks(status);
            "#,
        )?;

        let version: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'version'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if version.is_none() {
            self.conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }

        Ok(())
    }

    // ---- writes ----

    pub fn start(&self, input: StartInput) -> Result<(WorkTask, WorkEvent)> {
        let id = new_id();
        let ts = now_iso();
        let project = normalize_project(input.project.as_deref(), input.cwd.as_deref());
        let agent = input.agent.or_else(default_agent);
        let cwd = input.cwd.or_else(current_cwd);

        let task = WorkTask {
            id: id.clone(),
            title: input.title.clone(),
            project: project.clone(),
            cwd: cwd.clone(),
            agent: agent.clone(),
            status: "doing".into(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
            session_id: input.session_id.clone(),
            source: "agent".into(),
        };

        self.conn.execute(
            "INSERT INTO work_tasks
             (id, title, project, cwd, agent, status, created_at, updated_at, session_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id,
                task.title,
                task.project,
                task.cwd,
                task.agent,
                task.status,
                task.created_at,
                task.updated_at,
                task.session_id,
                task.source,
            ],
        )?;

        let event = self.insert_event(
            Some(&task.id),
            "start",
            &input.title,
            input.body.as_deref(),
            project.as_deref(),
            cwd.as_deref(),
            agent.as_deref(),
            input.session_id.as_deref(),
            None,
        )?;

        Ok((task, event))
    }

    pub fn update(&self, input: UpdateInput) -> Result<(WorkTask, WorkEvent)> {
        let mut task = self
            .get_task(&input.task_id)?
            .ok_or_else(|| Error::Query(format!("Unknown task: {}", input.task_id)))?;

        let ts = now_iso();
        if let Some(title) = &input.title {
            task.title = title.clone();
        }
        if let Some(status) = &input.status {
            validate_status(status)?;
            task.status = status.clone();
        }
        task.updated_at = ts;

        self.conn.execute(
            "UPDATE work_tasks SET title = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
            params![task.title, task.status, task.updated_at, task.id],
        )?;

        let kind = if task.status == "blocked" {
            "block"
        } else {
            "update"
        };
        let title = input.title.clone().unwrap_or_else(|| task.title.clone());

        let event = self.insert_event(
            Some(&task.id),
            kind,
            &title,
            input.body.as_deref(),
            task.project.as_deref(),
            task.cwd.as_deref(),
            task.agent.as_deref(),
            task.session_id.as_deref(),
            None,
        )?;

        Ok((task, event))
    }

    pub fn done(&self, input: DoneInput) -> Result<(WorkTask, WorkEvent)> {
        let mut task = self
            .get_task(&input.task_id)?
            .ok_or_else(|| Error::Query(format!("Unknown task: {}", input.task_id)))?;

        let ts = now_iso();
        if let Some(title) = &input.title {
            task.title = title.clone();
        }
        task.status = "done".into();
        task.updated_at = ts;

        self.conn.execute(
            "UPDATE work_tasks SET title = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
            params![task.title, task.status, task.updated_at, task.id],
        )?;

        let title = input.title.clone().unwrap_or_else(|| task.title.clone());

        let event = self.insert_event(
            Some(&task.id),
            "done",
            &title,
            input.body.as_deref(),
            task.project.as_deref(),
            task.cwd.as_deref(),
            task.agent.as_deref(),
            task.session_id.as_deref(),
            None,
        )?;

        Ok((task, event))
    }

    pub fn note(&self, input: NoteInput) -> Result<WorkEvent> {
        let (project, cwd, agent, session_id) = if let Some(tid) = &input.task_id {
            let task = self
                .get_task(tid)?
                .ok_or_else(|| Error::Query(format!("Unknown task: {tid}")))?;
            (
                input.project.or(task.project),
                input.cwd.or(task.cwd),
                input.agent.or(task.agent),
                input.session_id.or(task.session_id),
            )
        } else {
            (
                normalize_project(input.project.as_deref(), input.cwd.as_deref()),
                input.cwd.or_else(current_cwd),
                input.agent.or_else(default_agent),
                input.session_id,
            )
        };

        self.insert_event(
            input.task_id.as_deref(),
            "note",
            &input.title,
            input.body.as_deref(),
            project.as_deref(),
            cwd.as_deref(),
            agent.as_deref(),
            session_id.as_deref(),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_event(
        &self,
        task_id: Option<&str>,
        kind: &str,
        title: &str,
        body: Option<&str>,
        project: Option<&str>,
        cwd: Option<&str>,
        agent: Option<&str>,
        session_id: Option<&str>,
        meta_json: Option<&str>,
    ) -> Result<WorkEvent> {
        let id = new_id();
        let ts = now_iso();
        let local_date = today_local();
        self.conn.execute(
            "INSERT INTO work_events
             (id, task_id, ts, local_date, kind, title, body, project, cwd, agent, session_id, meta_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                task_id,
                ts,
                local_date,
                kind,
                title,
                body,
                project,
                cwd,
                agent,
                session_id,
                meta_json,
            ],
        )?;

        Ok(WorkEvent {
            id,
            task_id: task_id.map(|s| s.to_string()),
            ts,
            local_date,
            kind: kind.into(),
            title: title.into(),
            body: body.map(|s| s.to_string()),
            project: project.map(|s| s.to_string()),
            cwd: cwd.map(|s| s.to_string()),
            agent: agent.map(|s| s.to_string()),
            session_id: session_id.map(|s| s.to_string()),
            meta_json: meta_json.map(|s| s.to_string()),
            task_status: None,
        })
    }

    // ---- reads ----

    pub fn get_task(&self, id: &str) -> Result<Option<WorkTask>> {
        self.conn
            .query_row(
                "SELECT id, title, project, cwd, agent, status, created_at, updated_at,
                        session_id, source
                 FROM work_tasks WHERE id = ?1",
                params![id],
                row_to_task,
            )
            .optional()
            .map_err(Error::from)
    }

    pub fn list_tasks(
        &self,
        status: Option<&str>,
        project: Option<&str>,
        limit: i64,
    ) -> Result<Vec<WorkTask>> {
        let mut out = Vec::new();
        match (status, project) {
            (Some(s), Some(p)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, project, cwd, agent, status, created_at, updated_at,
                            session_id, source
                     FROM work_tasks WHERE status = ?1 AND project = ?2
                     ORDER BY updated_at DESC LIMIT ?3",
                )?;
                for row in stmt.query_map(params![s, p, limit], row_to_task)? {
                    out.push(row?);
                }
            }
            (Some(s), None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, project, cwd, agent, status, created_at, updated_at,
                            session_id, source
                     FROM work_tasks WHERE status = ?1
                     ORDER BY updated_at DESC LIMIT ?2",
                )?;
                for row in stmt.query_map(params![s, limit], row_to_task)? {
                    out.push(row?);
                }
            }
            (None, Some(p)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, project, cwd, agent, status, created_at, updated_at,
                            session_id, source
                     FROM work_tasks WHERE project = ?1
                     ORDER BY updated_at DESC LIMIT ?2",
                )?;
                for row in stmt.query_map(params![p, limit], row_to_task)? {
                    out.push(row?);
                }
            }
            (None, None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, project, cwd, agent, status, created_at, updated_at,
                            session_id, source
                     FROM work_tasks
                     ORDER BY updated_at DESC LIMIT ?1",
                )?;
                for row in stmt.query_map(params![limit], row_to_task)? {
                    out.push(row?);
                }
            }
        }
        Ok(out)
    }

    pub fn events_for_day(
        &self,
        date: &str,
        project: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Vec<WorkEvent>> {
        let mut out = Vec::new();
        match (project, agent) {
            (Some(p), Some(a)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT e.id, e.task_id, e.ts, e.local_date, e.kind, e.title, e.body,
                            e.project, e.cwd, e.agent, e.session_id, e.meta_json, t.status
                     FROM work_events e
                     LEFT JOIN work_tasks t ON e.task_id = t.id
                     WHERE e.local_date = ?1 AND e.project = ?2 AND e.agent = ?3
                     ORDER BY e.ts DESC",
                )?;
                for row in stmt.query_map(params![date, p, a], row_to_event)? {
                    out.push(row?);
                }
            }
            (Some(p), None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT e.id, e.task_id, e.ts, e.local_date, e.kind, e.title, e.body,
                            e.project, e.cwd, e.agent, e.session_id, e.meta_json, t.status
                     FROM work_events e
                     LEFT JOIN work_tasks t ON e.task_id = t.id
                     WHERE e.local_date = ?1 AND e.project = ?2
                     ORDER BY e.ts DESC",
                )?;
                for row in stmt.query_map(params![date, p], row_to_event)? {
                    out.push(row?);
                }
            }
            (None, Some(a)) => {
                let mut stmt = self.conn.prepare(
                    "SELECT e.id, e.task_id, e.ts, e.local_date, e.kind, e.title, e.body,
                            e.project, e.cwd, e.agent, e.session_id, e.meta_json, t.status
                     FROM work_events e
                     LEFT JOIN work_tasks t ON e.task_id = t.id
                     WHERE e.local_date = ?1 AND e.agent = ?2
                     ORDER BY e.ts DESC",
                )?;
                for row in stmt.query_map(params![date, a], row_to_event)? {
                    out.push(row?);
                }
            }
            (None, None) => {
                let mut stmt = self.conn.prepare(
                    "SELECT e.id, e.task_id, e.ts, e.local_date, e.kind, e.title, e.body,
                            e.project, e.cwd, e.agent, e.session_id, e.meta_json, t.status
                     FROM work_events e
                     LEFT JOIN work_tasks t ON e.task_id = t.id
                     WHERE e.local_date = ?1
                     ORDER BY e.ts DESC",
                )?;
                for row in stmt.query_map(params![date], row_to_event)? {
                    out.push(row?);
                }
            }
        }
        Ok(out)
    }

    pub fn day_stats(
        &self,
        date: &str,
        project: Option<&str>,
        agent: Option<&str>,
    ) -> Result<DayStats> {
        let events = self.events_for_day(date, project, agent)?;
        let updates = events.len();

        let mut task_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for e in &events {
            if let Some(tid) = &e.task_id {
                task_ids.insert(tid.clone());
            }
        }
        let tasks = task_ids.len();

        let mut doing = 0usize;
        let mut done = 0usize;
        let mut blocked = 0usize;
        for tid in &task_ids {
            if let Some(task) = self.get_task(tid)? {
                match task.status.as_str() {
                    "doing" => doing += 1,
                    "done" => done += 1,
                    "blocked" => blocked += 1,
                    _ => {}
                }
            }
        }

        Ok(DayStats {
            date: date.into(),
            updates,
            tasks,
            doing,
            done,
            blocked,
        })
    }

    pub fn day_view(
        &self,
        date: &str,
        detail: bool,
        project: Option<&str>,
        agent: Option<&str>,
    ) -> Result<DayView> {
        let events = self.events_for_day(date, project, agent)?;
        let stats = self.day_stats(date, project, agent)?;
        let weekday = weekday_name(date);
        let is_today = date == today_local();
        let summary = if detail || is_today {
            Vec::new()
        } else {
            summarize_events(&events)
        };

        Ok(DayView {
            date: date.into(),
            weekday,
            stats,
            events,
            summary,
            detail: detail || is_today,
        })
    }

    pub fn list_days(&self, limit: i64, project: Option<&str>) -> Result<Vec<DayStats>> {
        let days: Vec<String> = if let Some(p) = project {
            let mut stmt = self.conn.prepare(
                "SELECT local_date FROM work_events WHERE project = ?1
                 GROUP BY local_date ORDER BY local_date DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![p, limit], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT local_date FROM work_events
                 GROUP BY local_date ORDER BY local_date DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut out = Vec::new();
        for day in days {
            out.push(self.day_stats(&day, project, None)?);
        }
        Ok(out)
    }

    /// Copy work tables into an in-memory connection for ad-hoc SQL.
    pub fn materialize_into(&self, conn: &Connection, tables: &[&str]) -> Result<()> {
        for table in tables {
            match *table {
                "work_tasks" => {
                    conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS work_tasks (
                            id TEXT, title TEXT, project TEXT, cwd TEXT, agent TEXT,
                            status TEXT, created_at TEXT, updated_at TEXT,
                            session_id TEXT, source TEXT
                        );
                        DELETE FROM work_tasks;",
                    )?;
                    let mut stmt = self.conn.prepare(
                        "SELECT id, title, project, cwd, agent, status, created_at,
                                updated_at, session_id, source FROM work_tasks",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, String>(9)?,
                        ))
                    })?;
                    for row in rows {
                        let r = row?;
                        conn.execute(
                            "INSERT INTO work_tasks
                             (id, title, project, cwd, agent, status, created_at, updated_at, session_id, source)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                            params![r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9],
                        )?;
                    }
                }
                "work_events" => {
                    conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS work_events (
                            id TEXT, task_id TEXT, ts TEXT, local_date TEXT, kind TEXT,
                            title TEXT, body TEXT, project TEXT, cwd TEXT, agent TEXT,
                            session_id TEXT, meta_json TEXT
                        );
                        DELETE FROM work_events;",
                    )?;
                    let mut stmt = self.conn.prepare(
                        "SELECT id, task_id, ts, local_date, kind, title, body, project,
                                cwd, agent, session_id, meta_json FROM work_events",
                    )?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<String>>(9)?,
                            row.get::<_, Option<String>>(10)?,
                            row.get::<_, Option<String>>(11)?,
                        ))
                    })?;
                    for row in rows {
                        let r = row?;
                        conn.execute(
                            "INSERT INTO work_events
                             (id, task_id, ts, local_date, kind, title, body, project, cwd, agent, session_id, meta_json)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                            params![
                                r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9, r.10, r.11
                            ],
                        )?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkTask> {
    Ok(WorkTask {
        id: row.get(0)?,
        title: row.get(1)?,
        project: row.get(2)?,
        cwd: row.get(3)?,
        agent: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        session_id: row.get(8)?,
        source: row.get(9)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkEvent> {
    Ok(WorkEvent {
        id: row.get(0)?,
        task_id: row.get(1)?,
        ts: row.get(2)?,
        local_date: row.get(3)?,
        kind: row.get(4)?,
        title: row.get(5)?,
        body: row.get(6)?,
        project: row.get(7)?,
        cwd: row.get(8)?,
        agent: row.get(9)?,
        session_id: row.get(10)?,
        meta_json: row.get(11)?,
        task_status: row.get(12).ok().flatten(),
    })
}

fn validate_status(status: &str) -> Result<()> {
    match status {
        "doing" | "done" | "blocked" | "dropped" => Ok(()),
        other => Err(Error::Query(format!(
            "Invalid status '{other}'. Use: doing, done, blocked, dropped"
        ))),
    }
}

fn default_agent() -> Option<String> {
    std::env::var("DEVSQL_AGENT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if std::env::var_os("CLAUDECODE").is_some() || std::env::var_os("CLAUDE_CODE").is_some()
            {
                Some("claude".into())
            } else if std::env::var_os("CODEX_HOME").is_some() {
                Some("codex".into())
            } else {
                None
            }
        })
}

fn current_cwd() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
}

/// Prefer explicit project; else basename of cwd.
pub fn normalize_project(project: Option<&str>, cwd: Option<&str>) -> Option<String> {
    if let Some(p) = project {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    cwd.and_then(|c| {
        Path::new(c)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
    })
}

/// Parse `today`, `yesterday`, or `YYYY-MM-DD` into a local calendar date string.
pub fn parse_day(spec: Option<&str>) -> Result<String> {
    let today = Local::now().date_naive();
    match spec.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("today") => Ok(today.format("%Y-%m-%d").to_string()),
        Some("yesterday") => Ok((today - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string()),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| d.format("%Y-%m-%d").to_string())
            .map_err(|_| {
                Error::Query(format!(
                    "Invalid date '{s}'. Use YYYY-MM-DD, today, or yesterday"
                ))
            }),
    }
}

fn weekday_name(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%A").to_string())
        .unwrap_or_default()
}

/// Cluster events by project into short bullets for past-day summaries.
fn summarize_events(events: &[WorkEvent]) -> Vec<String> {
    if events.is_empty() {
        return vec!["No worklog events.".into()];
    }

    let mut by_project: std::collections::BTreeMap<String, Vec<&WorkEvent>> =
        std::collections::BTreeMap::new();
    for e in events {
        let key = e.project.clone().unwrap_or_else(|| "(no project)".into());
        by_project.entry(key).or_default().push(e);
    }

    let mut bullets = Vec::new();
    for (project, evs) in by_project {
        let mut titles: Vec<String> = Vec::new();
        for e in evs {
            if !titles.iter().any(|t| t == &e.title) {
                titles.push(e.title.clone());
            }
            if titles.len() >= 4 {
                break;
            }
        }
        bullets.push(format!("**{project}**: {}", titles.join("; ")));
    }
    bullets
}

/// Format a day view as human-readable markdown (Flavio-style feed).
pub fn format_day_markdown(view: &DayView) -> String {
    let mut out = String::new();
    let header_date = NaiveDate::parse_from_str(&view.date, "%Y-%m-%d")
        .map(|d| d.format("%A, %-d %B %Y").to_string())
        .unwrap_or_else(|_| view.date.clone());

    out.push_str(&format!("# {header_date}\n\n"));
    out.push_str(&format!(
        "{} updates across {} tasks. {} Doing, {} Done{}.\n\n",
        view.stats.updates,
        view.stats.tasks,
        view.stats.doing,
        view.stats.done,
        if view.stats.blocked > 0 {
            format!(", {} Blocked", view.stats.blocked)
        } else {
            String::new()
        }
    ));

    let show_feed = view.detail;

    if !show_feed && !view.summary.is_empty() {
        for bullet in &view.summary {
            out.push_str(&format!("- {bullet}\n"));
        }
        out.push_str("\n_Use `--detail` for the full event feed._\n");
        return out;
    }

    for e in &view.events {
        let time = format_local_time(&e.ts);
        let status = e
            .task_status
            .as_deref()
            .map(|s| {
                let label = match s {
                    "doing" => "Doing",
                    "done" => "Done",
                    "blocked" => "Blocked",
                    "dropped" => "Dropped",
                    other => other,
                };
                format!("  {label}")
            })
            .unwrap_or_default();

        out.push_str(&format!("{time}  **{}**{status}\n", e.title));
        if let Some(body) = &e.body {
            if !body.is_empty() {
                out.push_str(&format!("        {body}\n"));
            }
        }
        let mut meta = Vec::new();
        if let Some(p) = &e.project {
            meta.push(p.clone());
        }
        if let Some(a) = &e.agent {
            meta.push(a.clone());
        }
        if !meta.is_empty() {
            out.push_str(&format!("        {}\n", meta.join(" · ")));
        }
        out.push('\n');
    }

    if view.events.is_empty() {
        out.push_str("_No worklog events for this day._\n");
    }

    out
}

fn format_local_time(iso: &str) -> String {
    parse_datetime(iso)
        .map(|dt| dt.with_timezone(&Local).format("%-I:%M %p").to_string())
        .unwrap_or_else(|| iso.chars().take(16).collect())
}

fn parse_datetime(iso: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|n| n.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_worklog() -> (TempDir, Worklog) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("worklog.sqlite");
        let wl = Worklog::open_at(&path).unwrap();
        (dir, wl)
    }

    #[test]
    fn start_update_done_round_trip() {
        let (_dir, wl) = temp_worklog();
        let (task, event) = wl
            .start(StartInput {
                title: "Fix auth".into(),
                body: Some("Started investigating token refresh".into()),
                project: Some("velo".into()),
                cwd: Some("/Users/x/velo".into()),
                agent: Some("codex".into()),
                session_id: None,
            })
            .unwrap();

        assert_eq!(task.status, "doing");
        assert_eq!(event.kind, "start");
        assert_eq!(event.project.as_deref(), Some("velo"));

        let (_task, ev2) = wl
            .update(UpdateInput {
                task_id: task.id.clone(),
                title: Some("Fix auth token path".into()),
                body: Some("Found the bad middleware".into()),
                status: None,
            })
            .unwrap();
        assert_eq!(ev2.kind, "update");

        let (task3, ev3) = wl
            .done(DoneInput {
                task_id: task.id.clone(),
                title: None,
                body: Some("Shipped fix".into()),
            })
            .unwrap();
        assert_eq!(task3.status, "done");
        assert_eq!(ev3.kind, "done");

        let today = parse_day(Some("today")).unwrap();
        let view = wl.day_view(&today, true, None, None).unwrap();
        assert_eq!(view.stats.updates, 3);
        assert_eq!(view.stats.tasks, 1);
        assert_eq!(view.stats.done, 1);
        assert_eq!(view.stats.doing, 0);
    }

    #[test]
    fn cross_project_today() {
        let (_dir, wl) = temp_worklog();
        wl.start(StartInput {
            title: "A".into(),
            project: Some("proj-a".into()),
            agent: Some("claude".into()),
            ..Default::default()
        })
        .unwrap();
        wl.start(StartInput {
            title: "B".into(),
            project: Some("proj-b".into()),
            agent: Some("codex".into()),
            ..Default::default()
        })
        .unwrap();

        let today = parse_day(None).unwrap();
        let all = wl.day_view(&today, true, None, None).unwrap();
        assert_eq!(all.stats.updates, 2);

        let filtered = wl.day_view(&today, true, Some("proj-a"), None).unwrap();
        assert_eq!(filtered.stats.updates, 1);
        assert_eq!(filtered.events[0].title, "A");
    }

    #[test]
    fn project_from_cwd_basename() {
        assert_eq!(
            normalize_project(None, Some("/Users/x/Developer/lv/openbw")),
            Some("openbw".into())
        );
        assert_eq!(
            normalize_project(Some("explicit"), Some("/Users/x/other")),
            Some("explicit".into())
        );
    }

    #[test]
    fn parse_day_variants() {
        let today = parse_day(Some("today")).unwrap();
        assert_eq!(
            today,
            Local::now().date_naive().format("%Y-%m-%d").to_string()
        );
        assert_eq!(parse_day(Some("2026-07-01")).unwrap(), "2026-07-01");
        assert!(parse_day(Some("not-a-date")).is_err());
    }
}
