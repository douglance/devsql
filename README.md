# DevSQL

Unified SQL interface across AI coding history, Git repositories, and source code.

DevSQL loads data from Claude Code, Codex CLI, Git, and your source tree into an in-memory SQLite database so you can join, filter, and aggregate across all of them with standard SQL.

## Overview

```
~/.claude/    ─┐
~/.codex/     ─┤
.git/         ─┼──▶  SQLite (in-memory)  ──▶  SQL queries / JSON / CSV
src/**/*      ─┘
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
| `devsql recall <terms>` | Load prior work (sessions, commits, prompts) ranked by term-match count then recency |
| `devsql gather <terms>` | Run prior_work, repo_state, code_search, symbols, excerpts, and activity concurrently and return one token-budgeted bundle |
| `devsql work start\|update\|done\|note\|list` | Write structured work events to the durable day log (agents populate; humans read) |
| `devsql today` / `day` / `days` | Cross-project day timeline (Today granular; past days summarized) |

Common options: `--repo` / `-r` (default `.`), `--data-dir` / `-d` (default `~/.claude`). `gather` also takes `--budget` (default `8000` tokens; lowest-ranked rows are dropped round-robin per section, never mid-row, until the bundle fits).

### Workday memory

Agents write short work events so you can answer “what did I do today?” across every project:

```bash
devsql work start "Fix auth token refresh" --project velo --agent codex --body "Investigating 401s"
devsql work done <task-id> --body "Shipped fix; tests green"
devsql today
devsql day yesterday
```

Events live in `~/.devsql/worklog.sqlite` (override with `DEVSQL_HOME`) and are also queryable as SQL tables `work_tasks` and `work_events`.

### MCP server

Run `devsql --mcp` to start the stdio MCP server. It exposes the commands above as tools with typed input and output schemas. History/code tools are read-only; `work start|update|done|note` are write tools for the day log.

## Tables

### AI History

| Table | Source | Description |
|-------|--------|-------------|
| `history` | `~/.claude/history.jsonl` | Claude Code prompts (timestamp, display, project) |
| `transcripts` | `~/.claude/projects/<slug>/**/*.jsonl` (+ legacy `~/.claude/transcripts/*.jsonl`) | Full conversations incl. subagents (type, content, tool_name, session_id, `_project`, `_agent_id`, timestamp, model, `usage_*` token columns) |
| `sessions` | Same files as `transcripts` | One row per session: title, cwd, git_branch, first/last_timestamp, message counts, subagent_count, `total_*_tokens`, pr_url, pr_number |
| `todos` | `~/.claude/todos/*.json` | Task items (content, status) |
| `jhistory` | `~/.codex/history.jsonl` | Codex CLI prompts (session_id, text, display, timestamp) |
| `codex_history` | — | Alias for `jhistory` |
| `tool_calls` | `~/.claude/projects/<slug>/**/*.jsonl` (+ legacy `~/.claude/transcripts/*.jsonl`) | Claude assistant tool calls (tool_name, input_json, target, session_id, `_project`, timestamp) |
| `codex_tool_calls` | `$CODEX_HOME/sessions/**/*.jsonl` (default `~/.codex/sessions/**/*.jsonl`) | Codex function calls (tool_name, arguments_json, cmd, session_id, cwd, timestamp) |
| `work_tasks` | `~/.devsql/worklog.sqlite` | Durable tasks (title, project, status, agent, …) written via `devsql work` |
| `work_events` | `~/.devsql/worklog.sqlite` | Day-timeline events (start/update/done/note) with `local_date` |

### Git

| Table | Description |
|-------|-------------|
| `commits` | id, message, summary, author_name, authored_at, short_id |
| `branches` | name, is_head, commit_id |
| `diffs` | Commit-level stats: commit_id, files_changed, insertions, deletions |
| `diff_files` | Per-file stats: commit_id, path, status (A/D/M/R/C), insertions, deletions |

### Source Code

| Table | Description |
|-------|-------------|
| `source_files` | File inventory: path, name, extension, directory, size_bytes, line_count, modified_at, language |
| `source_lines` | Line content: file_path, line_number, content, is_blank |
| `symbols` | Definitions: file_path, name, kind, line_start, line_end, signature, visibility, parameters, return_type, language |
| `imports`\* | Import statements: file_path, line_number, module, name, alias, kind, is_default, is_wildcard |
| `ast_nodes`\* | Raw AST nodes |

\* Requires the `tree-sitter-ast` feature for full extraction. Without it, `symbols` falls back to regex-based extraction (Rust, TypeScript, JavaScript, Python, Go) and `imports`/`ast_nodes` are empty.

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
```

### Gather a context bundle
```bash
devsql gather "auth token refresh"
devsql gather "auth token refresh" -r /path/to/repo --budget 4000
```

`gather` returns six independently computed sections: prior work, repository state, code search, symbols, excerpts, and recent activity. If one section fails, the remaining sections are still returned and the failed section includes a note.

### File context and impact
```bash
devsql context src/engine.rs
devsql impact src/lib.rs
devsql history src/engine.rs
```

## Notes

- `history.timestamp` is in epoch milliseconds. Use `datetime(timestamp/1000, 'unixepoch')` to convert.
- A custom `DATE()` function normalizes epoch ms, epoch seconds, and ISO strings.
- Tables are loaded lazily — only those referenced in your query are populated.
- The `symbols` table extracts functions, structs, enums, traits, types, classes, interfaces, and more depending on language.

## License

MIT
