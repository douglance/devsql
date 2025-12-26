# DevSQL Claude Code Plugin

Query your Claude Code history with SQL, right from Claude Code.

## Installation

1. Install the devsql CLI:
```bash
brew install douglance/tap/devsql
```

2. Add the plugin to Claude Code:
```bash
# From the devsql repo
claude plugin add ./plugin

# Or from GitHub
claude marketplace add https://github.com/douglance/devsql
claude plugin install devsql
```

## Usage

### Slash Command

```
/devsql:query SELECT * FROM conversations LIMIT 10
```

### Natural Language

Just ask Claude about your history:
- "How many Claude sessions did I have this week?"
- "Which prompts led to the most commits?"
- "Show my productivity patterns"

Claude will automatically use devsql to answer.

## Available Tables

### Claude Code
- `conversations` - Chat sessions
- `messages` - Individual messages
- `todos` - Todo items
- `projects` - Project contexts

### Git
- `commits` - Commit history
- `branches` - Branch info
- `files` - Changed files
- `blame` - Line attribution

## Examples

```sql
-- Recent sessions
SELECT * FROM conversations ORDER BY timestamp DESC LIMIT 10

-- Commits correlated with Claude sessions
SELECT date(c.timestamp) as day, COUNT(*) as commits
FROM commits c
JOIN conversations conv ON date(c.timestamp) = date(conv.timestamp)
GROUP BY day

-- Most active days
SELECT date(timestamp) as day, COUNT(*) as sessions
FROM conversations
GROUP BY day ORDER BY sessions DESC LIMIT 7
```

## License

MIT
