# DevSQL

**Query your AI coding history to become a better prompter.**

DevSQL lets you analyze your Claude Code conversations alongside your Git commits. Find your most productive prompts, identify patterns in successful coding sessions, and learn what actually works for you.

## Why?

Your `~/.claude/` folder contains a goldmine of data: every prompt you've written, every tool Claude used, every conversation that led to shipped code. DevSQL turns that into queryable insights.

**Ask questions like:**
- "Which of my prompts led to the most commits?"
- "What patterns do my successful coding sessions have?"
- "When do I struggle most—and what prompts help me recover?"
- "Which tools does Claude use most when I'm productive?"

## What You Can Do

### Find Your Most Productive Prompts
```bash
# See which prompts preceded commits
devsql "SELECT h.display as prompt, COUNT(c.id) as commits_after
FROM history h
LEFT JOIN commits c ON DATE(datetime(h.timestamp/1000, 'unixepoch')) = DATE(c.authored_at)
GROUP BY h.display
HAVING commits_after > 0
ORDER BY commits_after DESC
LIMIT 20"
```

### Identify Struggle Sessions
```bash
# High prompt count + few commits = struggling
devsql "SELECT
  DATE(datetime(h.timestamp/1000, 'unixepoch')) as day,
  COUNT(*) as prompts,
  COUNT(DISTINCT c.id) as commits,
  CAST(COUNT(*) AS FLOAT) / MAX(1, COUNT(DISTINCT c.id)) as struggle_ratio
FROM history h
LEFT JOIN commits c ON DATE(datetime(h.timestamp/1000, 'unixepoch')) = DATE(c.authored_at)
GROUP BY day
ORDER BY struggle_ratio DESC
LIMIT 10"
```

### Analyze Your Prompting Patterns
```bash
# What tools correlate with productivity?
ccql "SELECT tool_name, COUNT(*) as uses
FROM transcripts
WHERE type = 'tool_use'
GROUP BY tool_name
ORDER BY uses DESC"
```

### Train Your AI Agent
Tell Claude Code to query your history:

> "Use devsql to find my 10 most effective prompts from the past month—the ones that led to commits the same day. Then analyze what they have in common."

> "Query my Claude history to find sessions where I used many prompts but made few commits. What was I struggling with?"

> "Find patterns in my successful refactoring sessions using devsql."

## Installation

### Claude Code Plugin (Recommended)

Install the plugin to use devsql directly within Claude Code:

```
/plugin marketplace add douglance/devsql
/plugin install devsql@devsql
```

Restart Claude Code to load the plugin. The plugin auto-installs the devsql binary on first use.

**Usage:**
- `/devsql:query SELECT * FROM history LIMIT 10` - Direct SQL queries
- Or just ask Claude: "Show my most productive prompts from last week"

### Homebrew (macOS/Linux)
```bash
brew install douglance/tap/devsql
```

### Direct Download
Download from [GitHub Releases](https://github.com/douglance/devsql/releases) for macOS or Linux.

### Build from Source
```bash
git clone https://github.com/douglance/devsql.git
cd devsql && cargo install --path crates/devsql
```

## The Three Tools

| Tool | What It Queries |
|------|-----------------|
| `ccql` | Your Claude Code data (~/.claude/) |
| `vcsql` | Your Git repositories |
| `devsql` | Both together—join conversations with commits |

## Available Tables

**Claude Code**: `history` (your prompts), `transcripts` (full conversations), `todos` (tasks)

**Git**: `commits`, `branches`, `tags`, `diffs`, `diff_files`, `blame`

## License

MIT
