//! `devsql today` / `day` / `days` — human day timeline views over the worklog.

use incurs::command::{CommandDef, Example, TypedContext};
use incurs::output::CommandResult;
use serde_json::Value;

use crate::worklog::{
    format_day_markdown, parse_day, DayStats, DayView, Worklog,
};
use super::{read_only_mcp, typed_from_result};

fn open_worklog() -> Result<Worklog, CommandResult> {
    Worklog::open().map_err(|e| CommandResult::Error {
        code: "WORKLOG_ERROR".into(),
        message: format!("Failed to open worklog: {e}"),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    })
}
fn tool_err(message: String) -> CommandResult {
    CommandResult::Error {
        code: "WORKLOG_ERROR".into(),
        message,
        retryable: false,
        exit_code: Some(1),
        cta: None,
    }
}

// ---------------------------------------------------------------------------
// today
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct TodayArgs {}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct TodayOptions {
    /// Filter by project
    project: Option<String>,
    /// Filter by agent
    agent: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct DayOutput {
    date: String,
    weekday: String,
    stats: DayStats,
    events: Vec<Value>,
    summary: Vec<String>,
    detail: bool,
    /// Markdown rendering for human-readable default display
    markdown: String,
}

fn day_to_output(view: DayView) -> DayOutput {
    let markdown = format_day_markdown(&view);
    DayOutput {
        date: view.date,
        weekday: view.weekday,
        stats: view.stats,
        events: view
            .events
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect(),
        summary: view.summary,
        detail: view.detail,
        markdown,
    }
}

async fn run_today(
    ctx: TypedContext<TodayArgs, TodayOptions, ()>,
) -> incurs::command::TypedResult<DayOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    let date = match parse_day(Some("today")) {
        Ok(d) => d,
        Err(e) => return typed_from_result(tool_err(e.to_string())),
    };
    match wl.day_view(
        &date,
        true, // today always granular
        ctx.options.project.as_deref(),
        ctx.options.agent.as_deref(),
    ) {
        Ok(view) => incurs::command::TypedResult::ok(day_to_output(view)),
        Err(e) => typed_from_result(tool_err(e.to_string())),
    }
}

pub fn build_today() -> CommandDef {
    CommandDef::typed::<TodayArgs, TodayOptions, (), DayOutput, _, _>("today", run_today)
        .description(
            "Show today's worklog timeline across all projects (granular event feed)",
        )
        .examples(vec![
            Example {
                command: "".into(),
                description: Some("Full cross-project today feed".into()),
            },
            Example {
                command: "--project openbw".into(),
                description: Some("Today, filtered to one project".into()),
            },
        ])
        .mcp(read_only_mcp())
        .done()
}
// ---------------------------------------------------------------------------
// day
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DayArgs {
    /// Date: YYYY-MM-DD, today, or yesterday (default: today)
    date: Option<String>,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DayOptions {
    /// Show full event feed instead of summary (past days)
    #[serde(default)]
    detail: bool,
    /// Filter by project
    project: Option<String>,
    /// Filter by agent
    agent: Option<String>,
}

async fn run_day(
    ctx: TypedContext<DayArgs, DayOptions, ()>,
) -> incurs::command::TypedResult<DayOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    let date = match parse_day(ctx.args.date.as_deref()) {
        Ok(d) => d,
        Err(e) => return typed_from_result(tool_err(e.to_string())),
    };
    match wl.day_view(
        &date,
        ctx.options.detail,
        ctx.options.project.as_deref(),
        ctx.options.agent.as_deref(),
    ) {
        Ok(view) => incurs::command::TypedResult::ok(day_to_output(view)),
        Err(e) => typed_from_result(tool_err(e.to_string())),
    }
}

pub fn build_day() -> CommandDef {
    CommandDef::typed::<DayArgs, DayOptions, (), DayOutput, _, _>("day", run_day)
        .description(
            "Show a day view: today is granular; past days summarize unless --detail",
        )
        .examples(vec![
            Example {
                command: "yesterday".into(),
                description: Some("Summarized view of yesterday".into()),
            },
            Example {
                command: "2026-07-25 --detail".into(),
                description: Some("Full event feed for a past day".into()),
            },
        ])
        .mcp(read_only_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// days
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DaysArgs {}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DaysOptions {
    /// How many days to list
    #[incurs(alias = "n", default = 14)]
    limit: i64,
    /// Filter by project
    project: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct DaysOutput {
    total: usize,
    days: Vec<DayStats>,
}

async fn run_days(
    ctx: TypedContext<DaysArgs, DaysOptions, ()>,
) -> incurs::command::TypedResult<DaysOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.list_days(ctx.options.limit, ctx.options.project.as_deref()) {
        Ok(days) => {
            let total = days.len();
            incurs::command::TypedResult::ok(DaysOutput { total, days })
        }
        Err(e) => typed_from_result(tool_err(e.to_string())),
    }
}

pub fn build_days() -> CommandDef {
    CommandDef::typed::<DaysArgs, DaysOptions, (), DaysOutput, _, _>("days", run_days)
        .description("List recent days that have worklog activity with counts")
        .examples(vec![Example {
            command: "--limit 7".into(),
            description: Some("Last week of activity".into()),
        }])
        .mcp(read_only_mcp())
        .done()
}
