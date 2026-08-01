---
description: Execute SQL queries against Claude Code, Codex CLI, Git, and source code data. Usage: /devsql:query <SQL>
---

# DevSQL Query

Execute a direct SQL query against developer-local data. This slash command is a convenience fallback; agents should prefer the DevSQL Code Mode server (`devsql --mcp`) when it is connected.

## Prerequisites

Install devsql first:
```bash
brew install douglance/tap/devsql
```

## Execution

Run the user's SQL query using devsql:

```bash
devsql "$ARGUMENTS"
```

For agent tool commands, use the subcommands directly:

```bash
devsql search "parse"            # Find symbols by name
devsql context src/engine.rs     # File metadata + symbols
devsql history src/engine.rs     # Git history for a file
devsql diff main~5 HEAD          # Compare refs (file + symbol level)
devsql impact src/lib.rs         # Exports and dependents
```

## Available Tables

### Claude + Codex Tables
- `history` — Claude prompt history (`~/.claude/history.jsonl`)
- `jhistory` — Codex prompt history (`~/.codex/history.jsonl`, or `$CODEX_HOME/history.jsonl`)
- `codex_history` — Alias of `jhistory`
- `transcripts` — Claude transcript logs from `~/.claude/projects/<slug>/**/*.jsonl` incl. subagents, plus legacy `~/.claude/transcripts/*.jsonl` (type, content, tool_name, session_id, `_project`, `_agent_id`, timestamp, model, `usage_input_tokens`, `usage_output_tokens`, `usage_cache_read_input_tokens`, `usage_cache_creation_input_tokens`, `usage_service_tier`, …)
- `sessions` — One row per Claude session (session_id, project, cwd, git_branch, version, title, first/last_timestamp, user/assistant_message_count, subagent_count, `total_*_tokens`, pr_url, pr_number)
- `todos` — Claude todo items (content, status)

### Git Tables (from current repo)
- `commits` — Git commit history (id, message, summary, author_name, authored_at, short_id)
- `branches` — Branch information (name, is_head, commit_id)
- `diffs` — Commit-level diff stats (commit_id, files_changed, insertions, deletions)
- `diff_files` — Per-file diff stats (commit_id, path, status, insertions, deletions)

### Code Tables (source analysis of current repo)
- `source_files` — File inventory (path, name, extension, directory, size_bytes, line_count, modified_at, language)
- `source_lines` — Line-level content (file_path, line_number, content, is_blank)
- `symbols` — Functions, classes, structs, traits, etc. (file_path, name, kind, line_start, line_end, signature, visibility, parameters, return_type, language)
- `imports` — Import/use statements (file_path, line_number, module, name, alias, kind, is_default, is_wildcard). Requires `tree-sitter-ast` feature for full extraction.
- `ast_nodes` — Raw AST nodes. Requires `tree-sitter-ast` feature.

## Example Queries

```sql
-- Recent Claude prompts
SELECT display, project
FROM history
ORDER BY timestamp DESC
LIMIT 10;

-- Recent Codex prompts
SELECT datetime(timestamp/1000, 'unixepoch') AS time, display
FROM jhistory
ORDER BY timestamp DESC
LIMIT 10;

-- Commits correlated with Codex prompt activity
SELECT date(c.authored_at) AS day, COUNT(*) AS commits, COUNT(j.session_id) AS codex_prompts
FROM commits c
LEFT JOIN jhistory j ON date(c.authored_at) = date(datetime(j.timestamp/1000, 'unixepoch'))
GROUP BY day
ORDER BY day DESC;

-- Find all public Rust functions (Rust emits kind='fn'; JS/TS use 'function')
SELECT name, file_path, line_start, signature
FROM symbols
WHERE kind = 'fn' AND visibility = 'pub'
ORDER BY file_path, line_start;

-- Top sessions by cache-read tokens
SELECT title, project, total_cache_read_input_tokens
FROM sessions
ORDER BY total_cache_read_input_tokens DESC
LIMIT 10;

-- Files with the most symbols
SELECT s.file_path, f.language, COUNT(*) as symbol_count
FROM symbols s
JOIN source_files f ON s.file_path = f.path
GROUP BY s.file_path
ORDER BY symbol_count DESC
LIMIT 10;

-- Join prompts with code changes: which files were you working on?
SELECT df.path, COUNT(DISTINCT c.id) as commits, COUNT(DISTINCT h.timestamp) as prompts
FROM diff_files df
JOIN commits c ON df.commit_id = c.id
LEFT JOIN history h ON date(c.authored_at) = date(datetime(h.timestamp/1000, 'unixepoch'))
GROUP BY df.path
ORDER BY commits DESC
LIMIT 10;
```

## Output

Display results in a formatted table. For large results, suggest the user pipe to csv:
```bash
devsql -f csv "SELECT ..." > output.csv
```
