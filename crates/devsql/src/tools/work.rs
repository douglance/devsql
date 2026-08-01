//! `devsql work` — agents write structured work events to the durable worklog.

use incurs::cli::Cli;
use incurs::command::{CommandDef, Example, McpAnnotations, McpCommandOptions, TypedContext};
use incurs::output::CommandResult;

use crate::worklog::{
    DoneInput, NoteInput, StartInput, UpdateInput, WorkEvent, WorkTask, Worklog,
};
use super::typed_from_result;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn open_worklog() -> Result<Worklog, CommandResult> {
    Worklog::open().map_err(|e| CommandResult::Error {
        code: "WORKLOG_ERROR".into(),
        message: format!("Failed to open worklog: {e}"),
        retryable: false,
        exit_code: Some(1),
        cta: None,
    })
}
fn tool_err(code: &str, message: String) -> CommandResult {
    CommandResult::Error {
        code: code.into(),
        message,
        retryable: false,
        exit_code: Some(1),
        cta: None,
    }
}

fn write_mcp() -> McpCommandOptions {
    McpCommandOptions {
        annotations: Some(McpAnnotations {
            read_only_hint: Some(false),
            destructive_hint: Some(false),
            idempotent_hint: Some(false),
            open_world_hint: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn read_mcp() -> McpCommandOptions {
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

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct StartArgs {
    /// Short human title for the work
    title: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct StartOptions {
    /// Longer narrative body
    body: Option<String>,
    /// Project name (defaults to basename of cwd)
    project: Option<String>,
    /// Working directory
    cwd: Option<String>,
    /// Agent name (claude, codex, grok, cursor, …)
    agent: Option<String>,
    /// Optional session id from Claude/Codex
    session_id: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct StartOutput {
    task: WorkTask,
    event: WorkEvent,
}

async fn run_start(ctx: TypedContext<StartArgs, StartOptions, ()>) -> incurs::command::TypedResult<StartOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.start(StartInput {
        title: ctx.args.title,
        body: ctx.options.body,
        project: ctx.options.project,
        cwd: ctx.options.cwd,
        agent: ctx.options.agent,
        session_id: ctx.options.session_id,
    }) {
        Ok((task, event)) => incurs::command::TypedResult::ok(StartOutput { task, event }),
        Err(e) => typed_from_result(tool_err("WORKLOG_ERROR", e.to_string())),
    }
}

fn start_cmd() -> CommandDef {
    CommandDef::typed::<StartArgs, StartOptions, (), StartOutput, _, _>("start", run_start)
        .description("Start a work task and emit a start event (agents: call when beginning non-trivial work)")
        .examples(vec![Example {
            command: r#""Fix auth token refresh" --project velo --agent codex --body "Investigating 401s""#
                .into(),
            description: Some("Start a task with project and body".into()),
        }])
        .mcp(write_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct UpdateArgs {
    /// Task id from work start
    task: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct UpdateOptions {
    /// New title (optional)
    title: Option<String>,
    /// Progress narrative
    body: Option<String>,
    /// Status: doing | blocked | dropped
    status: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct UpdateOutput {
    task: WorkTask,
    event: WorkEvent,
}

async fn run_update(
    ctx: TypedContext<UpdateArgs, UpdateOptions, ()>,
) -> incurs::command::TypedResult<UpdateOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.update(UpdateInput {
        task_id: ctx.args.task,
        title: ctx.options.title,
        body: ctx.options.body,
        status: ctx.options.status,
    }) {
        Ok((task, event)) => incurs::command::TypedResult::ok(UpdateOutput { task, event }),
        Err(e) => typed_from_result(tool_err("WORKLOG_ERROR", e.to_string())),
    }
}

fn update_cmd() -> CommandDef {
    CommandDef::typed::<UpdateArgs, UpdateOptions, (), UpdateOutput, _, _>("update", run_update)
        .description("Record progress on a work task")
        .examples(vec![Example {
            command: r#"abc123 --body "Found the bad middleware" --status doing"#.into(),
            description: Some("Update a task with a progress note".into()),
        }])
        .mcp(write_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// done
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DoneArgs {
    /// Task id from work start
    task: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct DoneOptions {
    /// Optional final title
    title: Option<String>,
    /// Outcome narrative
    body: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct DoneOutput {
    task: WorkTask,
    event: WorkEvent,
}

async fn run_done(ctx: TypedContext<DoneArgs, DoneOptions, ()>) -> incurs::command::TypedResult<DoneOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.done(DoneInput {
        task_id: ctx.args.task,
        title: ctx.options.title,
        body: ctx.options.body,
    }) {
        Ok((task, event)) => incurs::command::TypedResult::ok(DoneOutput { task, event }),
        Err(e) => typed_from_result(tool_err("WORKLOG_ERROR", e.to_string())),
    }
}

fn done_cmd() -> CommandDef {
    CommandDef::typed::<DoneArgs, DoneOptions, (), DoneOutput, _, _>("done", run_done)
        .description("Mark a work task done and emit a done event")
        .examples(vec![Example {
            command: r#"abc123 --body "Shipped fix; tests green""#.into(),
            description: Some("Complete a task".into()),
        }])
        .mcp(write_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// note
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct NoteArgs {
    /// Short note title
    title: String,
}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct NoteOptions {
    /// Note body
    body: Option<String>,
    /// Optional task to attach the note to
    task: Option<String>,
    /// Project name
    project: Option<String>,
    /// Working directory
    cwd: Option<String>,
    /// Agent name
    agent: Option<String>,
    /// Optional session id from Claude/Codex
    session_id: Option<String>,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct NoteOutput {
    event: WorkEvent,
}

async fn run_note(ctx: TypedContext<NoteArgs, NoteOptions, ()>) -> incurs::command::TypedResult<NoteOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.note(NoteInput {
        title: ctx.args.title,
        body: ctx.options.body,
        task_id: ctx.options.task,
        project: ctx.options.project,
        cwd: ctx.options.cwd,
        agent: ctx.options.agent,
        session_id: ctx.options.session_id,
    }) {
        Ok(event) => incurs::command::TypedResult::ok(NoteOutput { event }),
        Err(e) => typed_from_result(tool_err("WORKLOG_ERROR", e.to_string())),
    }
}

fn note_cmd() -> CommandDef {
    CommandDef::typed::<NoteArgs, NoteOptions, (), NoteOutput, _, _>("note", run_note)
        .description("Add a freeform worklog note (optionally attached to a task)")
        .examples(vec![Example {
            command: r#""Standup prep" --body "Three PRs ready for review" --project tmppr"#.into(),
            description: Some("Standalone note".into()),
        }])
        .mcp(write_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct ListArgs {}

#[derive(incurs::Options, serde::Deserialize, serde::Serialize)]
#[allow(dead_code)]
struct ListOptions {
    /// Filter by status (doing, done, blocked, dropped)
    status: Option<String>,
    /// Filter by project
    project: Option<String>,
    /// Max tasks to return
    #[incurs(alias = "n", default = 20)]
    limit: i64,
}

#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
struct ListOutput {
    total: usize,
    tasks: Vec<WorkTask>,
}

async fn run_list(ctx: TypedContext<ListArgs, ListOptions, ()>) -> incurs::command::TypedResult<ListOutput> {
    let wl = match open_worklog() {
        Ok(w) => w,
        Err(e) => return typed_from_result(e),
    };
    match wl.list_tasks(
        ctx.options.status.as_deref(),
        ctx.options.project.as_deref(),
        ctx.options.limit,
    ) {
        Ok(tasks) => {
            let total = tasks.len();
            incurs::command::TypedResult::ok(ListOutput { total, tasks })
        }
        Err(e) => typed_from_result(tool_err("WORKLOG_ERROR", e.to_string())),
    }
}

fn list_cmd() -> CommandDef {
    CommandDef::typed::<ListArgs, ListOptions, (), ListOutput, _, _>("list", run_list)
        .description("List work tasks (default: most recently updated)")
        .examples(vec![Example {
            command: "--status doing".into(),
            description: Some("List open tasks".into()),
        }])
        .mcp(read_mcp())
        .done()
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

/// Build the `work` command group (`devsql work start|update|done|note|list`).
pub fn build_group() -> Cli {
    Cli::create("work")
        .description(
            "Write structured work events for the day timeline.\n\
             Agents: start non-trivial work, update on progress, done on completion.",
        )
        .command("start", start_cmd())
        .command("update", update_cmd())
        .command("done", done_cmd())
        .command("note", note_cmd())
        .command("list", list_cmd())
}
