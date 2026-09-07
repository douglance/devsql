---
name: devsql-querying
description: Query and analyze developer-local history, macOS Unified Logs, Git, and source code through DevSQL Code Mode. Use when the user asks about prior conversations, shell commands, system logs, productivity, commits, sessions, symbols, file context, or impact analysis.
---

# DevSQL Querying Skill

Use DevSQL Code Mode to query Claude Code and Codex CLI history, shell history, macOS Unified Logs, Git commits, source code, and durable worklogs.

## Primary interface

Prefer the connected DevSQL Code Mode server. Use `codemode_search` to discover typed `devsql.*` methods, then `codemode_execute` to compose them in JavaScript. Poll durable work with `codemode_execution`; approve writes with `codemode_decide` only when the user has authorized them.

Use the direct commands below only when Code Mode is unavailable or when writing a shell script.

## When to Use

- User asks "How many Claude sessions did I have this week?"
- User wants to "Find my longest debugging sessions"
- User asks "Which prompts led to the most commits?"
- User wants productivity analytics or session insights
- User asks about correlating Claude/Codex usage with Git history
- User wants to search for symbols, functions, or classes in the codebase
- User asks "What's in this file?" or needs file context
- User wants to understand what changed between commits
- User asks about imports, dependencies, or impact of a file
- User wants to diagnose recent macOS application or subsystem behavior
- User asks what a Grok Bot said or decided (use `devsql grok search`, not `recall`/`gather`)

## Prerequisites

Ensure devsql is installed:
```bash
brew install douglance/tap/devsql
```

## Agent Tool Commands

For structured queries, prefer these subcommands (they return JSON):

| Command | Use When |
|---------|----------|
| `devsql search "<query>"` | Finding symbols by name (functions, classes, structs). Supports `--kind` filter and `--limit`. |
| `devsql context <file>` | Getting file metadata + all symbols defined in a file. |
| `devsql history <file>` | Showing Git commit history for a specific file with diff stats. |
| `devsql diff <base> <head>` | Comparing two Git refs with file-level and symbol-level change analysis. |
| `devsql impact <file>` | Analyzing a file's exports and finding potential dependents via imports. |
| `devsql grok search "<terms>"` | Searching Grok Bot conversations. Supports `--bot` and `--limit`. |
| `devsql grok status` | Checking Grok Bot index coverage and ingest errors. |
| `devsql grok sync` | Refreshing local replicas, and optionally backfilling full history from the gateway. |

All commands accept `--repo` / `-r` and `--data-dir` / `-d` options.

## Available Tables

### Claude/Codex Tables
| Table | Columns |
|-------|---------|
| `history` | timestamp, display (prompt text), project, pastedContents |
| `jhistory` | session_id, ts, text, display, timestamp |
| `codex_history` | Alias of `jhistory` |
| `transcripts` | type, content, tool_name, session_id, _source_file, _session_id, _project, _agent_id, timestamp, model, usage_input_tokens, usage_output_tokens, usage_cache_read_input_tokens, usage_cache_creation_input_tokens, usage_ephemeral_5m_input_tokens, usage_ephemeral_1h_input_tokens, usage_service_tier |
| `sessions` | session_id, project, cwd, git_branch, version, title, first_timestamp, last_timestamp, user_message_count, assistant_message_count, subagent_count, total_input_tokens, total_output_tokens, total_cache_read_input_tokens, total_cache_creation_input_tokens, pr_url, pr_number |
| `todos` | content, status |

`transcripts` covers `~/.claude/projects/<slug>/**/*.jsonl` (top-level sessions
plus subagent transcripts) and the legacy `~/.claude/transcripts/*.jsonl`.
`sessions` has one aggregated row per session file; `_project` / `project` is
the project slug directory (e.g. `-Users-you-Developer-app`), NULL for legacy
files. `_agent_id` is set only on subagent rows.

### Git Tables
| Table | Columns |
|-------|---------|
| `commits` | id, message, summary, author_name, authored_at, short_id |
| `branches` | name, is_head, commit_id |
| `diffs` | commit_id, files_changed, insertions, deletions |
| `diff_files` | commit_id, path, status (A/D/M/R/C), insertions, deletions |

### macOS Unified Log

| Table | Columns |
|-------|---------|
| `macos_logs` | timestamp, event_type, subsystem, category, process, process_id, thread_id, sender, message, level, activity_id, trace_id, boot_uuid, raw_json, provenance, source, archive_path, source_order, timestamp_ms, image/signpost fields |

Pass `log_last`, `log_start`/`log_end`, `log_predicate`, `log_archive`,
`log_level`, `log_max_rows`, and `log_timeout` to `devsql.query`. Prefer a
narrow time window plus process or subsystem filters. Defaults are 15 minutes,
standard level, 50,000 rows, and 30 seconds. The provider streams with bounded
memory, pushes safe filters into macOS `log show`, and returns partial rows with
a warning when a timeout or configured row cap is reached.

### Grok Bot Tables
| Table | Columns |
|-------|---------|
| `grok_bots` | bot_id, name, title, description, is_group, origin, remote_store_path, created_at, updated_at, last_activity_at, newest_entry_id, roster_present, entry_count, first_entry_at, last_entry_at, local_replica_path, gateway_backfilled_to_seq, gateway_complete |
| `grok_entries` | bot_id, entry_id, kind, role, direction, turn_ordinal, timestamp, timestamp_ms, message_type, text, from_agent, to_agent, author, event_type, raw_json, provenance, source_order |
| `grok_messages` | bot_id, bot_name, entry_id, kind, role, direction, message_type, text, timestamp, provenance (entries with real text only) |
| `grok_ingest_errors` | source, error_kind, bot_id, message, occurrences, first_observed_at, observed_at |

Grok Bots are **global**: they have no cwd and no repo, so `--repo` does not
scope them. They are deliberately absent from `recall`, `gather`, and
`command_events` — treat them as an explicit search target.

Two caveats worth knowing when interpreting results:

- Local replicas are truncated windows. `entry_count` below `newest_entry_id`
  means history exists on the gateway that is not indexed yet; run
  `devsql grok sync --bot <name> --full` to backfill. Inside a Grok Bot sandbox
  this does not apply: devsql reads each bot's `store.db` directly and offline.
- `provenance` says where a row came from: `local_replica`, `store_db`,
  `gateway`, or `both` (seen from more than one).
- `roster_present = 0` marks a bot deleted in the Grok UI whose history devsql
  still holds.

### Code Tables (Source Analysis)
| Table | Columns |
|-------|---------|
| `source_files` | path, name, extension, directory, size_bytes, line_count, modified_at, language |
| `source_lines` | file_path, line_number, content, is_blank |
| `symbols` | file_path, name, kind, line_start, line_end, signature, visibility, parameters, return_type, language |
| `imports`\* | file_path, line_number, module, name, alias, kind, is_default, is_wildcard |
| `ast_nodes`\* | (requires tree-sitter-ast feature) |

\* Full extraction requires the `tree-sitter-ast` build feature. Without it, `symbols` uses regex-based extraction and `imports`/`ast_nodes` are empty.

**Supported languages for symbol extraction:** Rust, TypeScript, JavaScript, Python, Go.

**Symbol kinds:** `fn` (Rust), `function` (TypeScript/JavaScript/Python), struct, enum, trait, type, const, static, mod, macro, class, interface (varies by language).

## Approach

1. Understand what the user wants to analyze
2. Choose the right tool:
   - For structured queries about symbols/files/history → use a subcommand
   - For cross-table analytics or custom joins → compose a SQL query
3. Execute with: `devsql "<query>"` or `devsql <subcommand> <args>`
4. Present results with insights

Note: history.timestamp is in milliseconds. Use `datetime(timestamp/1000, 'unixepoch')` to convert.

## Example Queries

```sql
-- Recent application errors (pass log_last: "10m", log_level: "info")
SELECT timestamp, process, subsystem, category, message
FROM macos_logs
WHERE process = 'ExampleApp' AND message LIKE '%error%'
ORDER BY timestamp DESC
LIMIT 100;

-- Recent prompts
SELECT display as prompt, project
FROM history ORDER BY timestamp DESC LIMIT 10;

-- Prompts this week
SELECT COUNT(*) as prompts
FROM history
WHERE datetime(timestamp/1000, 'unixepoch') > date('now', '-7 days');

-- Correlate prompts with commits
SELECT
  date(c.authored_at) as day,
  COUNT(DISTINCT h.timestamp) as prompts,
  COUNT(DISTINCT c.id) as commits
FROM commits c
LEFT JOIN history h
  ON date(c.authored_at) = date(datetime(h.timestamp/1000, 'unixepoch'))
GROUP BY day
ORDER BY day DESC
LIMIT 14;

-- Which prompts led to commits?
SELECT h.display as prompt, COUNT(c.id) as commits_after
FROM history h
JOIN commits c ON date(datetime(h.timestamp/1000, 'unixepoch')) = date(c.authored_at)
GROUP BY h.display
ORDER BY commits_after DESC
LIMIT 10;

-- Tool usage
SELECT tool_name, COUNT(*) as uses
FROM transcripts
WHERE type = 'tool_use'
GROUP BY tool_name
ORDER BY uses DESC;

-- Find all public Rust functions (Rust emits kind='fn'; JS/TS use 'function')
SELECT name, file_path, line_start, signature
FROM symbols
WHERE kind = 'fn' AND visibility = 'pub'
ORDER BY file_path, line_start;

-- Top sessions by cache-read tokens
SELECT title, project, total_cache_read_input_tokens, last_timestamp
FROM sessions
ORDER BY total_cache_read_input_tokens DESC
LIMIT 10;

-- Daily output tokens by model (flattened usage columns)
SELECT DATE(timestamp) as day, model, SUM(usage_output_tokens) as output_tokens
FROM transcripts
WHERE type = 'assistant' AND usage_output_tokens IS NOT NULL
GROUP BY day, model
ORDER BY day DESC;

-- Codebase overview by language
SELECT language, COUNT(*) as files, SUM(line_count) as total_lines
FROM source_files
GROUP BY language
ORDER BY total_lines DESC;

-- Files with the most symbols
SELECT s.file_path, f.language, COUNT(*) as symbol_count
FROM symbols s
JOIN source_files f ON s.file_path = f.path
GROUP BY s.file_path
ORDER BY symbol_count DESC
LIMIT 10;

-- Most changed files correlated with symbol count
SELECT df.path, COUNT(DISTINCT df.commit_id) as commits,
  SUM(df.insertions) as total_adds,
  (SELECT COUNT(*) FROM symbols s WHERE s.file_path = df.path) as symbols
FROM diff_files df
GROUP BY df.path
ORDER BY commits DESC
LIMIT 10;
```

### Find what a Grok Bot said about a topic
```bash
devsql grok search "release" --bot Terri --limit 10
```

```sql
SELECT bot_name, substr(timestamp, 1, 10) AS day, role, substr(text, 1, 120) AS excerpt
FROM grok_messages
WHERE text LIKE '%deploy%'
ORDER BY timestamp_ms DESC
LIMIT 20;
```

### See which bots need a gateway backfill
```sql
SELECT name, entry_count, newest_entry_id, gateway_complete
FROM grok_bots
WHERE roster_present = 1
ORDER BY entry_count;
```

## Output Formats

- Default: formatted table
- CSV: `devsql -f csv "<query>"`
- JSON: `devsql -f json "<query>"`
