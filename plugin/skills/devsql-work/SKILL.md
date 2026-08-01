---
name: devsql-work
description: Write and read cross-project workday memory via devsql work/today/day. Use when starting or finishing non-trivial work, logging progress, or answering what was done today/yesterday across projects.
---

# DevSQL workday memory

Durable cross-project day log. Agents write structured events; humans read a Flavio-style day timeline.

Storage: `~/.devsql/worklog.sqlite` (override with `DEVSQL_HOME`).

## When to write

- Starting non-trivial work → `devsql work start`
- Meaningful progress → `devsql work update`
- Finishing → `devsql work done`
- Quick observation without a task → `devsql work note`

Prefer **outcome** language a human would want in a day log. Do not dump tool traces.

## Commands

```bash
# Start (returns task.id — keep it for update/done)
devsql work start "Title" --project myproj --agent codex --body "Why / first step"

# Progress
devsql work update <task-id> --body "What changed" --status doing

# Complete
devsql work done <task-id> --body "Outcome"

# Freeform note
devsql work note "Standup" --body "…" --project myproj

# List open tasks
devsql work list --status doing
```

Options: `--project`, `--cwd`, `--agent`, `--session-id`, `--body`, `--title`.

## Day views (read)

```bash
devsql today                    # granular feed for today (all projects)
devsql today --project openbw
devsql day yesterday            # summary bullets
devsql day 2026-07-25 --detail  # full feed for a past day
devsql days --limit 14          # day index with counts
```

JSON: add `--format json`. The `markdown` field has the human timeline.

## SQL tables

```sql
SELECT * FROM work_events WHERE local_date = date('now') ORDER BY ts DESC;
SELECT * FROM work_tasks WHERE status = 'doing';
```

## Decision rule

| Need | Command |
|------|---------|
| Prior code/conversation context for this task | `devsql gather` / `recall` |
| Record work humans care about across projects | `devsql work start/update/done` |
| "What did we do today/yesterday?" | `devsql today` / `day` |
