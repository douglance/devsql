---
name: devsql-querying
description: Query and analyze Claude Code history joined with Git data using SQL. Use when user asks about their Claude conversations, productivity patterns, commit history correlation, session analytics, or wants to explore their coding history with SQL queries.
---

# DevSQL Querying Skill

Query your Claude Code history joined with Git commits to analyze productivity patterns.

## When to Use

- User asks "How many Claude sessions did I have this week?"
- User wants to "Find my longest debugging sessions"
- User asks "Which prompts led to the most commits?"
- User wants productivity analytics or session insights
- User asks about correlating Claude usage with Git history

## Prerequisites

Ensure devsql is installed:
```bash
brew install douglance/tap/devsql
```

## Available Tables

### Claude Code Tables
| Table | Columns |
|-------|---------|
| `history` | timestamp, display (prompt text), project, pastedContents |
| `transcripts` | Full conversation data including tool_use, tool_name |
| `todos` | Todo items tracked in sessions |

### Git Tables
| Table | Columns |
|-------|---------|
| `commits` | id, message, summary, author_name, authored_at, short_id |
| `branches` | name, is_head, commit_id |
| `diffs` | Diff content per commit |
| `blame` | Line-by-line attribution |

## Approach

1. Understand what the user wants to analyze
2. Compose a SQL query joining Claude and Git data as needed
3. Execute with: `devsql "<query>"`
4. Present results with insights

Note: history.timestamp is in milliseconds. Use `datetime(timestamp/1000, 'unixepoch')` to convert.

## Example Queries

```sql
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
```

## Output Formats

- Default: formatted table
- CSV: `devsql -f csv "<query>"`
- JSON: `devsql -f json "<query>"`
