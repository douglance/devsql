# DevSQL

Unified SQL interface across AI coding history, Git repositories, and source code.

DevSQL loads data from Claude Code, Codex CLI, shell history, Git, and your source tree into an in-memory SQLite database so you can join, filter, and aggregate across all of them with standard SQL.

## Overview

```
~/.claude/       ─┐
~/.codex/        ─┤
shell histories  ─┤
.git/            ─┼──▶  SQLite (in-memory)  ──▶  SQL queries / JSON / CSV
src/**/*         ─┘
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
devsql -f json "<SQL>"        # JSON output
devsql -f csv "<SQL>"         # CSV output
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
| `devsql recall <terms>` | Load prior work (sessions, commits, prompts, shell commands, and agent-issued commands) ranked by term-match count then recency |
| `devsql gather <terms>` | Run prior_work, repo_state, code_search, symbols, excerpts, and activity concurrently and return one token-budgeted bundle |

Common options: `--repo` / `-r` (default `.`), `--data-dir` / `-d` (default `~/.claude`). `gather` also takes `--budget` (default `8000` tokens; lowest-ranked rows are dropped round-robin per section, never mid-row, until the bundle fits).

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

### Shell History

| Table | Source | Description |
|-------|--------|-------------|
| `shell_history` | Atuin, zsh, and bash | Normalized commands with source, source_id, source_order, timestamp, duration_ms, exit_code, command, cwd, session_id, hostname, and history_path |
| `command_events` | `shell_history`, Claude Bash calls, and Codex exec/shell calls | Commands with explicit channel, actor, provenance quality/reason, source identity, session/agent metadata, tool name, execution metadata, and source path |

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
and `provenance_quality = 'exact'`. Version 1 emits only `agent` and `unknown`;
`human` and `automation` are reserved for future exact sources.

The stable `command_events` columns are `source`, `channel`, `actor`,
`provenance_quality`, `provenance_reason`, `source_id`, `source_order`,
`session_id`, `parent_session_id`, `agent_id`, `agent_role`, `originator`,
`tool_name`, `timestamp`, `duration_ms`, `exit_code`, `command`, `cwd`,
`hostname`, and `source_path`. Values are read without redaction.

Source paths are discovered from Atuin's `db_path` setting and the standard
Atuin, zsh, and bash locations. Set `DEVSQL_ATUIN_DB`,
`DEVSQL_ZSH_HISTORY`, or `DEVSQL_BASH_HISTORY` to override a source path.

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
