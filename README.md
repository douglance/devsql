# DevSQL

Unified SQL queries across developer data: Claude Code conversations + Git commits.

## Tools

| Tool | Description |
|------|-------------|
| `ccql` | SQL queries for Claude Code data (~/.claude/) |
| `vcsql` | SQL queries for Git repository data |
| `devsql` | **Unified queries** - join Claude + Git for productivity analysis |

## Installation

### Homebrew (macOS/Linux)

```bash
brew install douglance/tap/devsql
```

This installs all three binaries: `ccql`, `vcsql`, and `devsql`.

### Direct Download

Download pre-built binaries from the [GitHub Releases](https://github.com/douglance/devsql/releases) page.

Available platforms:
- macOS (Apple Silicon): `aarch64-apple-darwin`
- macOS (Intel): `x86_64-apple-darwin`
- Linux (ARM64): `aarch64-unknown-linux-gnu`
- Linux (x86_64): `x86_64-unknown-linux-gnu`
- Windows (x86_64): `x86_64-pc-windows-msvc`

### Build from Source

```bash
# Clone the repository
git clone https://github.com/douglance/devsql.git
cd devsql

# Build all binaries
cargo build --release

# Binaries are in target/release/
./target/release/ccql --version
./target/release/vcsql --version
./target/release/devsql --version

# Or install to ~/.cargo/bin
cargo install --path crates/devsql
cargo install --path crates/ccql
cargo install --path crates/vcsql
```

## Quick Start

```bash
# Claude Code queries
ccql "SELECT * FROM history LIMIT 5"

# Git queries
vcsql "SELECT short_id, summary FROM commits LIMIT 5"

# Cross-database: productivity analysis
devsql "SELECT
  DATE(h.timestamp) as day,
  COUNT(*) as claude_msgs,
  COUNT(DISTINCT c.id) as commits
FROM history h
LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at)
GROUP BY day
ORDER BY day DESC
LIMIT 14"
```

## Tables

### Claude Code (ccql, devsql)
- `history` - User prompts
- `transcripts` - AI conversation logs
- `todos` - Task tracking

### Git (vcsql, devsql)
- `commits` - Commit history
- `diffs` - Per-commit diff stats
- `diff_files` - Per-file changes
- `branches` - Branch info
- `tags` - Repository tags
- `blame` - Per-line attribution

## Key Queries

### Struggle Ratio
```sql
SELECT
  DATE(h.timestamp) as day,
  COUNT(*) as msgs,
  COUNT(DISTINCT c.id) as commits,
  CAST(COUNT(*) AS FLOAT) / MAX(1, COUNT(DISTINCT c.id)) as ratio
FROM history h
LEFT JOIN commits c ON DATE(h.timestamp) = DATE(c.authored_at)
GROUP BY day
ORDER BY ratio DESC
```
*High ratio = days you struggled (many messages, few commits)*

### Tool Usage
```sql
SELECT tool_name, COUNT(*) as uses
FROM transcripts
WHERE type = 'tool_use'
GROUP BY tool_name
ORDER BY uses DESC
```

### Hot Files
```sql
SELECT path, COUNT(*) as changes
FROM diff_files
GROUP BY path
ORDER BY changes DESC
LIMIT 20
```

## License

MIT
