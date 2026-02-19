---
description: Execute SQL queries against Claude Code, Codex CLI, and Git data. Usage: /devsql:query <SQL>
---

# DevSQL Query

Execute SQL queries against your Claude Code and Codex CLI history joined with Git commit data.

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

## Available Tables

### Claude + Codex Tables
- `history` - Claude prompt history (`~/.claude/history.jsonl`)
- `jhistory` - Codex prompt history (`~/.codex/history.jsonl`, or `$CODEX_HOME/history.jsonl`)
- `codex_history` - Alias of `jhistory`
- `transcripts` - Claude transcript logs
- `todos` - Claude todo items

### Git Tables (from current repo)
- `commits` - Git commit history
- `branches` - Branch information
- `diffs` - Aggregate diff stats
- `diff_files` - Per-file diff stats

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
```

## Output

Display results in a formatted table. For large results, suggest the user pipe to csv:
```bash
devsql -f csv "SELECT ..." > output.csv
```
