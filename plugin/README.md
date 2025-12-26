# DevSQL Claude Code Plugin

Query your Claude Code history with SQL, right from Claude Code.

## Installation

```
/plugin marketplace add douglance/devsql
/plugin install devsql@devsql
```

Restart Claude Code to load the plugin.

The plugin auto-installs the `devsql` binary via Homebrew on first session start.

## Usage

### Slash Command

```
/devsql:query SELECT * FROM history LIMIT 10
```

### Natural Language

Just ask Claude about your history:
- "How many Claude sessions did I have this week?"
- "Which prompts led to the most commits?"
- "Show my productivity patterns"

Claude will automatically use devsql to answer.

## Available Tables

### Claude Code
- `history` - Your prompts
- `transcripts` - Full conversations
- `todos` - Todo items
- `projects` - Project contexts

### Git
- `commits` - Commit history
- `branches` - Branch info
- `diffs` - File changes
- `blame` - Line attribution

## Examples

```sql
-- Recent prompts
SELECT * FROM history ORDER BY timestamp DESC LIMIT 10

-- Commits correlated with Claude sessions
SELECT date(c.authored_at) as day, COUNT(*) as commits
FROM commits c
JOIN history h ON date(c.authored_at) = date(h.timestamp)
GROUP BY day

-- Most active days
SELECT date(timestamp) as day, COUNT(*) as prompts
FROM history
GROUP BY day ORDER BY prompts DESC LIMIT 7
```

## License

MIT
