# DevSQL Claude Code Plugin

Query your Claude Code and Codex CLI history with SQL, right from Claude Code.

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
/devsql:query SELECT * FROM jhistory LIMIT 10
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

### Codex CLI
- `jhistory` - Prompt history from `~/.codex/history.jsonl`
- `codex_history` - Alias of `jhistory`

### Git
- `commits` - Commit history
- `branches` - Branch info
- `diffs` - Commit diff stats
- `diff_files` - Per-file diff stats

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

-- Recent Codex prompts
SELECT datetime(timestamp/1000, 'unixepoch') as time, display
FROM jhistory
ORDER BY timestamp DESC LIMIT 10
```

## License

MIT
