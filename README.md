# DevSQL

Code Mode for observing AI coding history, shell history, macOS Unified Logs, Git repositories, and source code. Code Mode is the primary agent interface; the direct SQL CLI remains available for humans and scripts.

DevSQL loads data from Claude Code, Codex CLI, shell history, macOS Unified Logs, Git and your source tree into SQLite so you can join, filter, and aggregate across all of them with standard SQL. Most providers load into memory on demand. Unified Logs stream through a bounded virtual table rather than being materialized. Codex rollout journals use a rebuildable incremental cache so compressed conversation history does not need to be reparsed for every query.

The Codex cache path includes its schema version, leaving the unversioned cache used by older releases intact. A new schema rebuilds its index and uses additional disk space; older indexes are not automatically removed.

## Primary agent interface: Code Mode

Run `devsql --mcp` and connect it as an stdio MCP server. Agents see five stable lifecycle tools instead of a wide direct-tool surface:

| Tool | Purpose |
|------|---------|
| `codemode_search` | Discover typed `devsql.*` methods and saved snippets |
| `codemode_execute` | Start JavaScript that can compose multiple DevSQL calls |
| `codemode_execution` | Inspect a durable execution or fetch one artifact |
| `codemode_decide` | Approve or reject a pending write |
| `codemode_cancel` | Cancel a running or paused execution |

For example, an agent can execute:

```js
const recent = await devsql.query({
  query: "SELECT source, timestamp, command FROM shell_history ORDER BY timestamp DESC LIMIT 20"
});
recent
```

Read-only methods run without approval. Use direct `devsql` commands when working interactively in a terminal or writing a shell script.

## Overview

```
~/.claude/       --+
~/.codex/        --+
shell histories  --+
macOS logs       --+--> SQLite --> SQL queries / JSON / CSV
.git/            --+
src/**/*         --+
```

Three standalone tools, one unified interface:

| Tool | Data Source |
|------|------------|
| `ccql` | Claude Code + Codex CLI data (`~/.claude/`, `~/.codex/`) |
| `vcsql` | Git repositories (commits, branches, diffs) |
| `devsql` | All of the above, plus source code analysis |

DevSQL auto-detects which tables your query references and only loads the data it needs.

## Installation

### Homebrew
```bash
brew install douglance/tap/devsql
```

### Claude Code Plugin
```
/plugin marketplace add douglance/devsql
/plugin install devsql@devsql
```

The plugin auto-installs the binary on first session start.

### Direct Download
Prebuilt binaries for macOS and Linux are available from [GitHub Releases](https://github.com/douglance/devsql/releases).

### From Source
```bash
git clone https://github.com/douglance/devsql.git
cd devsql && cargo install --path crates/devsql
```

To enable tree-sitter-based AST analysis (richer symbol extraction and import parsing):
```bash
cargo install --path crates/devsql --features tree-sitter-ast
```

## Usage

### SQL Queries

```bash
devsql "<SQL>"                # Default table output
devsql --format json "<SQL>"  # JSON output
devsql --format csv "<SQL>"   # CSV output
devsql query "<SQL>"          # Named form for MCP and scripts
```

### Commands

Structured commands that return JSON, designed for use by AI agents and scripts:

| Command | Description |
|---------|-------------|
| `devsql search <query>` | Find symbols by name across the codebase |
| `devsql context <file>` | File metadata and symbols for a given path |
| `devsql history <file>` | Git commit history for a specific file |
| `devsql diff <base> <head>` | Compare two Git refs with file and symbol-level stats |
| `devsql impact <file>` | Analyze exports and find potential dependents |
| `devsql recall <terms>` | Load prior work (Claude sessions, Codex threads, commits, prompts, shell commands, and agent-issued commands) ranked by term-match count then recency |
| `devsql gather <terms>` | Run prior_work, repo_state, code_search, symbols, excerpts, and activity concurrently and return one token-budgeted bundle |

Common options: `--repo` / `-r` (default `.`), `--data-dir` / `-d` (default `~/.claude`). `gather` also takes `--budget` (default `8000` tokens); it drops the lowest-ranked row of the largest section, never part of a row, until the bundle fits or no rows remain. Each section's `omitted` counts rows removed by this step; it does not count results excluded by search or source limits. The response envelope itself can exceed very small budgets. If one prior-work source fails, results from the other source remain available and the section note identifies the failure.

### Code Mode server

Run `devsql --mcp` to start the primary agent interface described above. Direct commands remain the explicit CLI fallback.

### Shared history over Tailscale

Choose one machine as the shared host. Install this version of DevSQL, Tailscale,
SSH, and rsync on every participating machine. Each client must be able to use
noninteractive SSH to the host, with its host key already trusted. The host must
have `devsql` in `~/.cargo/bin`, `~/.local/bin`, or its SSH command PATH.

On each participating machine, including the host, publish its local history:

```bash
devsql shared sync HOST
```

Replace `HOST` with the host's Tailscale name or full DNS name. Sync again to
refresh the snapshot; it does not run automatically. An unsuccessful transfer
leaves the previous snapshot available. The host stores private SQLite snapshots
in `~/.devsql/shared/machines`. Each snapshot contains Claude, Codex, Grok, and
shell history available on the machine at sync time, including conversation and
tool contents. Protect SSH access to the host accordingly. A snapshot replaces
that machine's previous snapshot, so retain source history or separate backups.

Query from any participating machine:

```bash
devsql shared query 'SELECT _machine, display FROM history LIMIT 10' --host HOST
devsql shared query 'SELECT _machine, modified_at FROM shared_snapshots' --host HOST
```

`_machine` is the Tailscale node ID. `modified_at` is the snapshot file's Unix
timestamp. Only successfully uploaded machines appear; compare this list with
the machines you expect and check timestamps for freshness. Set
`DEVSQL_SHARED_HOST=HOST` in the agent's environment to omit `--host`. Agents
can call `devsql.shared_query({query: "SELECT * FROM shared_snapshots"})` through
Code Mode; its network access may require approval. Queries are read-only;
local Git, source-code, and macOS-log tables remain local. The host must be
online for remote queries and uploads. Each sync rebuilds and transfers the
whole snapshot, and shared queries temporarily copy referenced tables on the
host, so large histories require sufficient disk space and time.

## Tables

### AI History

| Table | Source | Description |
|-------|--------|-------------|
| `history` | `~/.claude/history.jsonl` | Claude Code prompts (timestamp, display, project) |
| `transcripts` | `~/.claude/projects/<slug>/**/*.jsonl` (+ legacy `~/.claude/transcripts/*.jsonl`) | Full conversations incl. subagents (type, content, tool_name, session_id, `_project`, `_agent_id`, timestamp, model, `usage_*` token columns) |
| `sessions` | Same files as `transcripts` | One row per session: title, cwd, git_branch, first/last_timestamp, message counts, subagent_count, `total_*_tokens`, pr_url, pr_number |
| `todos` | `~/.claude/todos/*.json` | Task items (content, status) |
| `jhistory` | `~/.codex/history.jsonl` | Codex CLI prompts (session_id, text, display, timestamp) |
| `codex_history` | - | Alias for `jhistory` |
| `codex_threads` | `$CODEX_HOME/{sessions,archived_sessions}/**/rollout-*.jsonl[.zst]` | One row per Codex thread: lineage, agent identity, originator, cwd, Git branch, state, compression, journal path, timestamps, first user text, and aggregate counts |
| `codex_events` | Same rollout journals | One row per newline-terminated journal record: thread ID, record index, timestamp, record type, payload type, role, call ID, and source path |
| `codex_token_usage` | Same rollout journals | One row per recorded per-response usage event, including input, cached input, output, reasoning output, and total tokens |
| `codex_token_totals` | Same rollout journals | Latest cumulative token-count snapshot per thread that has a total token count |
| `codex_messages` | Same rollout journals | Normalized user and assistant content, including canonical-message status and source provenance |
| `codex_tool_executions` | Same rollout journals | Tool calls paired with outputs by thread and call ID |
| `codex_tool_calls` | Same rollout journals | Backward-compatible tool-call view with source, session, agent, cwd, and timestamp provenance |
| `codex_compactions` | Same rollout journals | Compaction summaries and window metadata |
| `codex_ingest_errors` | DevSQL Codex index | Nonfatal journal read and JSON parsing errors |
| `tool_calls` | `~/.claude/projects/<slug>/**/*.jsonl` (+ legacy `~/.claude/transcripts/*.jsonl`) | Claude assistant tool calls with source, session, subagent, cwd, and timestamp provenance |
| `grok_bots` | `~/Library/Application Support/Grok Bot/sand-client-persistence/*.blob` | One row per Grok Bot: name, roster presence, remote store path, entry counts, first/last entry, local replica and gateway backfill state |
| `grok_entries` | Replicas, in-box `store.db`, and optional gateway backfill | Every transcript entry: kind, role, direction, text, agents, request/batch IDs, `raw_json`, and `provenance` (`local_replica`, `store_db`, `gateway`, or `both`) |
| `grok_messages` | View over `grok_entries` | Entries that carry real text, joined to the bot name |
| `grok_ingest_errors` | DevSQL Grok index | Nonfatal blob read, parse, and gateway errors, deduplicated with an occurrence count |

### Grok Bots

Grok Bots are cloud agents on a Grok Bot Sand gateway. Unlike every other source,
they have **no working directory and no repo** — their only path is a remote
`/home/box/sand-data/agents/<uuid>/store.db`. They are therefore **global**, and
are deliberately excluded from `recall`, `gather`, and `command_events`: reach
them explicitly through `devsql grok` or by querying the `grok_*` tables.

devsql reads three sources into one index, deduplicated on `(bot_id, entry_id)`;
a row seen from more than one is marked `provenance = 'both'`:

| Source | Where | Notes |
|--------|-------|-------|
| Desktop replicas | `~/Library/Application Support/Grok Bot/sand-client-persistence/*.blob` | Offline and zero-config, but **truncated client-side windows** |
| Per-bot stores | `/home/box/sand-data/agents/<uuid>/store.db` | The server of record. Only present **inside a Grok Bot sandbox**, where it is the complete, offline source |
| Gateway | `grokctl bot transcript-tail` | Full history from anywhere, but needs a reachable gateway |

Because replicas are truncated, the index is append-only and never prunes: an
entry the app evicts, or a bot deleted from the roster, stays queryable. Set
`DEVSQL_GROK_DIR` to override the data directory (point it at the app-support
root, not `sand-client-persistence`).

**Running inside a Grok Bot sandbox**, devsql finds `/home/box/sand-data`
automatically and reads every bot's `store.db` directly -- no configuration and
no network. That path also carries entry kinds the desktop replica never shows,
such as `tool-call`. Bot names come from each store's `kv` table when no roster
blob exists.

```bash
devsql grok status                      # coverage, sync state, ingest errors
devsql grok search "standup card"       # global search across all bots
devsql grok search release --bot Terri  # one bot
devsql grok sync --offline true         # refresh local replicas, no network
devsql grok sync --bot Terri --full     # backfill full history via grokctl
```

`grok sync` optionally backfills complete history from the gateway by shelling
out to [`grokctl`](https://github.com/douglance/grokctl) (`--grokctl <path>` or
`DEVSQL_GROKCTL_BIN`). Only read-classified `grokctl` commands are used. If
`grokctl` is missing or the gateway is unreachable, the command still succeeds:
local sources are committed and the result reports `gateway.reachable = false`.

### Shell History

| Table | Source | Description |
|-------|--------|-------------|
| `shell_history` | Atuin, zsh, and bash | Normalized commands with source, source identity/order, timestamp, execution metadata, cwd, session, hostname, and history path |
| `command_events` | `shell_history`, Claude Bash calls, and Codex exec/shell calls | Commands with channel, actor, provenance quality/reason, source identity, session/agent metadata, execution metadata, and source path |

DevSQL reads shell history without modifying it. It excludes Atuin rows marked
deleted, keeps duplicate commands across sources, and treats missing or
unreadable sources as empty. Commands are returned exactly as stored, including
credential-like text.

`command_events` is a source-native union. It does not infer who typed an
Atuin, zsh, or bash command and does not correlate or deduplicate rows across
sources. Those rows use `channel = 'shell'`, `actor = 'unknown'`,
`provenance_quality = 'unattributed'`, and
`provenance_reason = 'unattributed_shell_history'`. Claude Bash calls and Codex
`exec_command`/`shell` calls use `channel = 'agent_tool'`, `actor = 'agent'`,
and `provenance_quality = 'exact'`.

The stable event identity is `(source, session_id, source_id)`. Shell rows use
their native history source and command identity; Claude and Codex rows use the
session/thread ID plus the tool call ID. For Codex commands, that pair maps back
to `codex_tool_executions(thread_id, call_id)`.

The stable `command_events` columns are `source`, `channel`, `actor`,
`provenance_quality`, `provenance_reason`, `source_id`, `source_order`,
`session_id`, `parent_session_id`, `agent_id`, `agent_role`, `originator`,
`tool_name`, `timestamp`, `duration_ms`, `exit_code`, `command`, `cwd`,
`hostname`, and `source_path`. Values are read without redaction.

`exit_code` is nullable. Shell history reports source-native exit status when
available, Claude Bash rows leave it `NULL`, and Codex command rows report it
only when DevSQL recognizes Codex's host wrapper line before the `Final output:`
delimiter.

Source paths are discovered from Atuin's `db_path` setting and the standard
Atuin, zsh, and bash locations. Set `DEVSQL_ATUIN_DB`,
`DEVSQL_ZSH_HISTORY`, or `DEVSQL_BASH_HISTORY` to override a source path.

### macOS Unified Logs

| Table | Source | Description |
|-------|--------|-------------|
| `macos_logs` | macOS live log datastore or a `.logarchive` | Normalized event fields, stable provenance, source ordering, and the original NDJSON record |

Use it through Code Mode:

```js
await devsql.query({
  query: "SELECT timestamp, process, subsystem, category, level, message FROM macos_logs WHERE subsystem = 'com.apple.runningboard' ORDER BY timestamp DESC LIMIT 100",
  log_last: "10m",
  log_level: "info"
})
```

The direct CLI accepts `--log-last`, paired `--log-start`/`--log-end`,
`--log-predicate`, `--log-archive`, `--log-level`, `--log-max-rows`, and
`--log-timeout`. Defaults are the last 15 minutes, standard-level events,
50,000 rows, and 30 seconds. `last` accepts `boot` or a positive value ending
in `s`, `m`, `h`, or `d`.

DevSQL safely pushes timestamp, process, PID, subsystem, category, level, event
type, exact message, and simple `%literal%` message filters into `/usr/bin/log`.
The child process writes through a bounded channel; DevSQL drains stderr and
kills and reaps the process on timeout, SQL `LIMIT`, row cap, query error, or
cursor teardown. A timeout or configured row cap returns the rows already read
with a partial-scan warning. SQL `LIMIT` is treated as an intentional bound.

The stable columns are `timestamp`, `event_type`, `subsystem`, `category`,
`process`, `process_id`, `thread_id`, `sender`, `message`, `level`,
`activity_id`, `trace_id`, `boot_uuid`, `raw_json`, `provenance`, `source`,
`archive_path`, `source_order`, `timestamp_ms`, `format_string`,
`process_image_path`, `sender_image_path`, `parent_activity_id`, signpost
fields, `user_id`, and `mach_timestamp`. The hidden columns `start`, `end`,
`predicate`, `archive`, `level_filter`, `max_rows`, and `timeout` can override
the matching option inside SQL. This provider is read-only, macOS-only, and
does not cache or persist logs. Messages and `raw_json` are returned without
redaction.

### Git

| Table | Description |
|-------|-------------|
| `commits` | id, message, summary, author_name, authored_at, short_id |
| `branches` | name, is_head, commit_id |
| `diffs` | Commit-level stats: commit_id, files_changed, insertions, deletions |
| `diff_files` | Per-file stats: commit_id, path, status (A/D/M/R/C), insertions, deletions |
| `commit_parents`, `tags`, `refs`, `stashes`, `reflog`, `blame`, `config`, `remotes`, `submodules`, `status`, `worktrees`, `hooks`, `notes` | Loaded when a query reads from them (`blame` covers every tracked file, so it is slow on large repos) |

### Source Code

| Table | Description |
|-------|-------------|
| `source_files` | File inventory: path, name, extension, directory, size_bytes, line_count, modified_at, language |
| `source_lines` | Line content: file_path, line_number, content, is_blank |
| `symbols` | Definitions: file_path, name, kind, line_start, line_end, signature, visibility, parameters, return_type, language |
| `imports`\* | Import statements: file_path, line_number, module, name, alias, kind, is_default, is_wildcard |

\* Requires the `tree-sitter-ast` feature; without it a query on `imports` fails and says so, and `symbols` falls back to regex-based extraction (Rust, TypeScript, JavaScript, Python, Go).

Run `devsql schema` to list every table, or `devsql schema <table>` for its exact columns.

## Examples

### Join prompts with commits
```sql
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
```

### Find productive prompts
```sql
SELECT h.display as prompt, COUNT(c.id) as commits_after
FROM history h
JOIN commits c ON date(datetime(h.timestamp/1000, 'unixepoch')) = date(c.authored_at)
GROUP BY h.display
HAVING commits_after > 0
ORDER BY commits_after DESC
LIMIT 20;
```

### Codebase overview by language
```sql
SELECT language, COUNT(*) as files, SUM(line_count) as total_lines
FROM source_files
GROUP BY language
ORDER BY total_lines DESC;
```

### Hottest files (most commits + most symbols)
```sql
SELECT df.path,
  COUNT(DISTINCT df.commit_id) as commits,
  SUM(df.insertions) as lines_added,
  (SELECT COUNT(*) FROM symbols s WHERE s.file_path = df.path) as symbols
FROM diff_files df
GROUP BY df.path
ORDER BY commits DESC
LIMIT 10;
```

### Search symbols
```bash
devsql search "parse"
devsql search "Error" --kind struct
```

### Semantic diff between refs
```bash
devsql diff main~5 HEAD
```

### Recall prior work
```bash
devsql recall "vision simulator mute"
devsql recall "auth token refresh" -r /path/to/repo
devsql recall "older evidence" --all-codex-messages true
```

By default, `recall` and `gather` search the latest 50,000 canonical Codex
messages. `--all-codex-messages true` searches all indexed canonical Codex
messages for prior work and may take substantially longer; other source and
result limits still apply.

A Codex result includes `thread_id` and `record_index`, identifying the matching
message for a targeted follow-up query. Explicit SQL returns stored text without
`recall` or `gather` redaction:

```sql
SELECT role, text, timestamp FROM codex_messages
WHERE thread_id = '<thread-id>' AND record_index = <record-index>;
```

### Query Codex conversation history
```sql
SELECT
  thread.cwd,
  message.role,
  message.text,
  message.timestamp
FROM codex_messages AS message
JOIN codex_threads AS thread
  ON thread.thread_id = message.thread_id
WHERE message.is_canonical = 1
  AND message.text LIKE '%auth callback%'
ORDER BY message.timestamp DESC;
```

### Measure recorded Codex task usage
```sql
SELECT COUNT(*) AS responses, COUNT(input_tokens) AS responses_with_input,
       COUNT(cached_input_tokens) AS responses_with_cache,
       COUNT(total_tokens) AS responses_with_total, SUM(input_tokens) AS input_tokens,
       SUM(cached_input_tokens) AS cached_input_tokens,
       SUM(output_tokens) AS output_tokens, SUM(total_tokens) AS total_tokens
FROM codex_token_usage WHERE thread_id = '<thread-id>';
```

To inspect individual recorded turns in one thread and the budget implied by a
100x reduction, group by the recorded turn ID:

```sql
SELECT turn_id, COUNT(*) AS responses,
       COUNT(input_tokens) AS responses_with_input,
       COUNT(total_tokens) AS responses_with_total,
       SUM(total_tokens) AS total_tokens,
       SUM(total_tokens) / 100.0 AS hundredfold_budget,
       MIN(input_tokens) AS smallest_observed_response_input
FROM codex_token_usage
WHERE thread_id = '<thread-id>' AND turn_id IS NOT NULL
GROUP BY turn_id
ORDER BY MIN(record_index);
```

The smallest observed response is not a lower bound for a different run. If it
already exceeds the hundredfold budget, repeating any of those observed
responses cannot meet that budget; a comparison needs a different execution
and an independent check that it accomplishes the same task. This query excludes
records without turn IDs and does not attribute descendant threads to a turn.

For a task with linked subagents, include the parent and all indexed descendants:

```sql
WITH RECURSIVE thread_tree(thread_id) AS (
  SELECT thread_id FROM codex_threads WHERE thread_id = '<thread-id>'
  UNION
  SELECT child.thread_id FROM codex_threads AS child
  JOIN thread_tree AS parent ON child.parent_thread_id = parent.thread_id
)
SELECT COUNT(DISTINCT tree.thread_id) AS recorded_threads,
       COUNT(usage.record_index) AS recorded_responses,
       COUNT(usage.total_tokens) AS responses_with_total,
       SUM(usage.input_tokens) AS input_tokens,
       SUM(usage.cached_input_tokens) AS cached_input_tokens,
       SUM(usage.output_tokens) AS output_tokens,
       SUM(usage.total_tokens) AS total_tokens,
       (SELECT COUNT(*) FROM thread_tree AS t WHERE NOT EXISTS
         (SELECT 1 FROM codex_token_usage AS u WHERE u.thread_id = t.thread_id))
         AS threads_without_usage
FROM thread_tree AS tree
LEFT JOIN codex_token_usage AS usage ON usage.thread_id = tree.thread_id;
```

This covers only indexed, linked journals. Missing journals, unlinked or external workers, and missing response records are not detectable from these counts; a complete comparison must account for them separately.

The table includes `token_usage_record` entries only; older journals or incomplete runs can lack them. For those, `codex_token_totals` exposes the latest cumulative `token_count` event with a total, if present; it can itself be incomplete and may use different accounting. In some journals, cumulative snapshots omit recorded compaction-response usage, so they can undercount the complete task cost. Do not sum snapshots with per-response records. Missing counters are NULL, not zero, and SQL sums skip NULLs, so check the counts before treating a sum as complete. Reasoning output is part of output, and cached input is part of input: do not add them a second time. Both tables are local and are not included in shared snapshots.

### Gather a context bundle
```bash
devsql gather "auth token refresh"
devsql gather "auth token refresh" -r /path/to/repo --budget 4000
devsql gather "older command" --all-codex-commands true
```

`gather` returns six independently computed sections: prior work, repository state, code search, symbols, excerpts, and recent activity. If one section fails, the remaining sections are still returned and the failed section includes a note. Codex prior-work excerpts in `gather` and `recall` show up to 200 characters around the first literal matching term after redaction.

Activity normally searches the latest 5,000 Codex tool executions. `--all-codex-commands true` searches indexed Codex commands across the full history and may take substantially longer; other source, result, and budget limits still apply. Matching Codex command rows include `thread_id` and `call_record_index`, which identify the execution in `codex_tool_executions` for a targeted follow-up query. This option searches command text and working directory, not tool output.

For non-shell Codex tool calls, activity searches the latest 10,000 calls and
returns a redacted excerpt around the first matching argument term (or the start
of the arguments if only the tool name matches) with the same execution identifiers.
A separate `--all-codex-tools true` option searches all indexed non-shell tool
calls; it can be slow and still applies result and budget limits. It does not
extend the shell-command search.
A match in a script is evidence of submitted script text, not proof that a
nested command ran; inspect the recorded output for its outcome.

### File context and impact
```bash
devsql context src/engine.rs
devsql impact src/lib.rs
devsql history src/engine.rs
```

## Notes

- `history.timestamp` is in epoch milliseconds. Use `datetime(timestamp/1000, 'unixepoch')` to convert.
- A custom `DATE()` function normalizes epoch ms, epoch seconds, and ISO strings.
- Tables are loaded lazily; only those referenced in your query are populated.
- The `symbols` table extracts functions, structs, enums, traits, types, classes, interfaces, and more depending on language.

### Codex journal indexing and privacy

- DevSQL reads canonical Codex journals from `$CODEX_HOME`, falling back to `~/.codex`. It reads active and archived `.jsonl` and `.jsonl.zst` journals; it does not query Codex's catalog, history, goals, memories, logs, credentials, attachments, generated images, or shell snapshots.
- The versioned Codex index lives under the platform cache directory at `devsql/codex-index/<CODEX_HOME-hash>.sqlite` (set `DEVSQL_CACHE_DIR` to move all DevSQL caches; sources inside the system temporary directory cache there). Its first build parses the complete journal corpus. Later loads use file metadata to skip unchanged journals and read only appended records from growing plain journals.
- Base64 images embedded in tool output, such as screenshots returned to the model, are stored as `data:image/...;base64,<N bytes omitted>`. They are unsearchable and were most of the cache's size.
- A journal's own `payload.id` is its thread ID. For subagent journals, `payload.session_id` and `source.subagent.thread_spawn.parent_thread_id` identify the parent and are stored as lineage instead of collapsing the child into the parent.
- The index is rebuildable and removes derived rows after source journals disappear. Schema changes automatically replace the disposable cache.
- DevSQL creates the cache directory with mode `0700` and cache files with mode `0600` on Unix. The cache still contains conversation text, tool arguments, commands, and tool output, so protect it like the source journals.
- Explicit SQL returns indexed content as stored. `recall` and `gather` redact common authorization values, API keys, access and refresh tokens, passwords, cookies, known token prefixes, and sensitive URL values before rendering automatic context.
- DevSQL records encrypted reasoning only as event metadata. It does not decrypt or index reasoning content.

## License

MIT
