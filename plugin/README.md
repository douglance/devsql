# DevSQL Claude Code Plugin

Use DevSQL Code Mode to query Claude Code history, Codex CLI history, shell history, Git data, source code, and durable worklogs. Code Mode is the primary agent interface; slash commands and the direct CLI are fallbacks.

## Installation

```
/plugin marketplace add douglance/devsql
/plugin install devsql@devsql
```

Restart Claude Code to load the plugin.

The plugin auto-installs the `devsql` binary via Homebrew on first session start.

## Usage

### Code Mode

Connect `devsql --mcp` as an stdio MCP server. Agents use the five `codemode_*` lifecycle tools to discover and compose typed `devsql.*` methods. Read-only methods run automatically; worklog writes require approval.

### Slash Command

```
/devsql:query SELECT * FROM history LIMIT 10
/devsql:query SELECT * FROM symbols WHERE kind = 'fn' LIMIT 10
```

### Agent Tool Commands

```
devsql search "parse"            # Find symbols by name
devsql context src/engine.rs     # File metadata + symbols
devsql history src/engine.rs     # Git history for a file
devsql diff main~5 HEAD          # Compare refs (file + symbol level)
devsql impact src/lib.rs         # Exports and dependents
```

### Natural Language

Just ask Claude about your history or codebase:
- "How many Claude sessions did I have this week?"
- "Which prompts led to the most commits?"
- "Find all structs in the project"
- "What changed between these two commits at the symbol level?"

Claude will automatically use devsql to answer.

## Available Tables

### Claude Code
- `history` — Your prompts
- `transcripts` — Full conversations incl. subagents (type, content, tool_name, session_id, `_project`, `_agent_id`, timestamp, model, `usage_*` token columns)
- `sessions` — One row per session (title, project, message counts, subagent_count, `total_*_tokens`, pr_url, pr_number)
- `todos` — Todo items (content, status)

### Codex CLI
- `jhistory` — Prompt history from `~/.codex/history.jsonl`
- `codex_history` — Alias of `jhistory`

### Git
- `commits` — Commit history (id, message, summary, author_name, authored_at, short_id)
- `branches` — Branch info (name, is_head, commit_id)
- `diffs` — Commit-level diff stats (commit_id, files_changed, insertions, deletions)
- `diff_files` — Per-file diff stats (commit_id, path, status, insertions, deletions)

### Code (Source Analysis)
- `source_files` — File inventory (path, name, extension, language, size_bytes, line_count)
- `source_lines` — Line-level content (file_path, line_number, content, is_blank)
- `symbols` — Functions, classes, structs, etc. (name, kind, file_path, line_start, signature, visibility)
- `imports`\* — Import statements (file_path, module, name, alias, kind)
- `ast_nodes`\* — Raw AST nodes

\* Full extraction requires the `tree-sitter-ast` build feature. Without it, `symbols` uses regex-based extraction and `imports`/`ast_nodes` are empty.

## Examples

```sql
-- Recent prompts
SELECT display as prompt, project
FROM history ORDER BY timestamp DESC LIMIT 10

-- Commits correlated with Claude sessions
SELECT date(c.authored_at) as day, COUNT(*) as commits
FROM commits c
JOIN history h ON date(c.authored_at) = date(datetime(h.timestamp/1000, 'unixepoch'))
GROUP BY day

-- Most active days
SELECT date(datetime(timestamp/1000, 'unixepoch')) as day, COUNT(*) as prompts
FROM history
GROUP BY day ORDER BY prompts DESC LIMIT 7

-- Which prompts led to commits?
SELECT h.display as prompt, COUNT(c.id) as commits_after
FROM history h
JOIN commits c ON date(datetime(h.timestamp/1000, 'unixepoch')) = date(c.authored_at)
GROUP BY h.display
ORDER BY commits_after DESC LIMIT 10

-- Find all public Rust functions (Rust emits kind='fn'; JS/TS use 'function')
SELECT name, file_path, line_start, signature
FROM symbols
WHERE kind = 'fn' AND visibility = 'pub'
ORDER BY file_path, line_start

-- Largest files by line count
SELECT path, language, line_count, size_bytes
FROM source_files
ORDER BY line_count DESC LIMIT 10

-- Blank line ratio per file
SELECT file_path,
  COUNT(*) as total_lines,
  SUM(is_blank) as blank_lines,
  ROUND(100.0 * SUM(is_blank) / COUNT(*), 1) as blank_pct
FROM source_lines
GROUP BY file_path
ORDER BY blank_pct DESC LIMIT 10
```

## License

MIT
