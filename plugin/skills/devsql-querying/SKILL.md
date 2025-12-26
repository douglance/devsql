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
| Table | Description |
|-------|-------------|
| `conversations` | Chat sessions (id, timestamp, project, duration) |
| `messages` | Messages in conversations (role, content, timestamp) |
| `todos` | Todo items tracked in sessions |
| `projects` | Project contexts and settings |

### Git Tables
| Table | Description |
|-------|-------------|
| `commits` | Commit history (hash, message, author, timestamp) |
| `branches` | Branch information |
| `files` | Files changed per commit |
| `blame` | Line-by-line attribution |
| `hooks` | Git hooks configuration |

## Approach

1. Understand what the user wants to analyze
2. Compose a SQL query joining Claude and Git data as needed
3. Execute with: `devsql "<query>"`
4. Present results with insights

## Example Queries

```sql
-- Sessions this week
SELECT COUNT(*) as sessions, SUM(duration) as total_minutes
FROM conversations
WHERE timestamp > date('now', '-7 days');

-- Correlate sessions with commits
SELECT
  date(c.timestamp) as day,
  COUNT(DISTINCT conv.id) as sessions,
  COUNT(DISTINCT c.hash) as commits
FROM commits c
LEFT JOIN conversations conv
  ON date(c.timestamp) = date(conv.timestamp)
GROUP BY day
ORDER BY day DESC
LIMIT 14;

-- Most productive prompt patterns
SELECT
  substr(m.content, 1, 100) as prompt_start,
  COUNT(DISTINCT c.hash) as resulting_commits
FROM messages m
JOIN conversations conv ON m.conversation_id = conv.id
JOIN commits c ON date(conv.timestamp) = date(c.timestamp)
WHERE m.role = 'user'
GROUP BY prompt_start
ORDER BY resulting_commits DESC
LIMIT 10;
```

## Output Formats

- Default: formatted table
- CSV: `devsql -f csv "<query>"`
- JSON: `devsql -f json "<query>"`
