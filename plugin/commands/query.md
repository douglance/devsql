---
description: Execute SQL queries against Claude Code history and Git data. Usage: /devsql:query <SQL>
---

# DevSQL Query

Execute SQL queries against your Claude Code conversation history joined with Git commit data.

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

### Claude Code Tables (from ~/.claude)
- `conversations` - Chat sessions with Claude
- `messages` - Individual messages in conversations
- `todos` - Todo items from sessions
- `projects` - Project contexts

### Git Tables (from current repo)
- `commits` - Git commit history
- `branches` - Branch information
- `files` - Files changed in commits
- `blame` - Line-by-line blame data
- `hooks` - Git hooks configuration

## Example Queries

```sql
-- Recent conversations
SELECT * FROM conversations ORDER BY timestamp DESC LIMIT 10

-- Commits with their Claude sessions
SELECT c.hash, c.message, conv.id as session
FROM commits c
JOIN conversations conv ON date(c.timestamp) = date(conv.timestamp)

-- Most active days
SELECT date(timestamp) as day, COUNT(*) as sessions
FROM conversations
GROUP BY day ORDER BY sessions DESC
```

## Output

Display results in a formatted table. For large results, suggest the user pipe to csv:
```bash
devsql -f csv "SELECT ..." > output.csv
```
