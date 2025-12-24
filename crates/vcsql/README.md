# vcsql

SQL query engine for Git repositories. Query commits, branches, diffs, blame, and more with full SQL power.

## Features

- **Full SQL support** - JOINs, CTEs, window functions, aggregations, subqueries
- **17 queryable tables** - commits, branches, tags, diffs, blame, config, and more
- **Multiple output formats** - table, JSON, JSONL, CSV
- **Multi-repo queries** - aggregate data across multiple repositories
- **Zero configuration** - point at any repo and start querying

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
./target/release/vcsql --help
```

## Quick Start

```bash
# Recent commits
vcsql "SELECT short_id, summary, authored_at FROM commits ORDER BY authored_at DESC LIMIT 10"

# Commits by author
vcsql "SELECT author_name, COUNT(*) as commits FROM commits GROUP BY author_name ORDER BY commits DESC"

# Current branch
vcsql "SELECT name FROM branches WHERE is_head = 1"

# Join commits with diffs
vcsql "SELECT c.short_id, c.summary, d.insertions, d.deletions
       FROM commits c JOIN diffs d ON d.commit_id = c.id
       ORDER BY d.insertions DESC LIMIT 5"
```

## Available Tables

### Core
| Table | Description |
|-------|-------------|
| `commits` | Commit history and metadata |
| `commit_parents` | Parent-child relationships |

### References
| Table | Description |
|-------|-------------|
| `branches` | Local and remote branches |
| `tags` | Annotated and lightweight tags |
| `refs` | All references (unified view) |
| `stashes` | Stashed changes |
| `reflog` | Reference history |

### Changes
| Table | Description |
|-------|-------------|
| `diffs` | Per-commit diff summary |
| `diff_files` | Per-file changes |
| `blame` | Per-line attribution |

### Configuration
| Table | Description |
|-------|-------------|
| `config` | Git configuration |
| `remotes` | Remote repositories |
| `submodules` | Nested repositories |

### Working Directory
| Table | Description |
|-------|-------------|
| `status` | Working directory status |
| `worktrees` | Linked working trees |

### Operational
| Table | Description |
|-------|-------------|
| `hooks` | Installed git hooks |
| `notes` | Git notes |

## Commands

```bash
# List all tables
vcsql tables

# Show table schema
vcsql schema commits
vcsql schema          # all tables

# Show example queries
vcsql examples
```

## Output Formats

```bash
# Table (default)
vcsql "SELECT * FROM branches"

# JSON
vcsql -f json "SELECT * FROM commits LIMIT 3"

# CSV
vcsql -f csv "SELECT * FROM commits" > commits.csv

# JSONL (one object per line)
vcsql -f jsonl "SELECT * FROM commits"

# No header
vcsql -H "SELECT name FROM branches"

# Verbose (shows timing)
vcsql -v "SELECT COUNT(*) FROM commits"
```

## Multi-Repository Queries

```bash
# Query multiple repos
vcsql -r ./repo1 -r ./repo2 "SELECT repo, COUNT(*) as commits FROM commits GROUP BY repo"
```

## Example Queries

### Analytics

```sql
-- Commits by day of week
SELECT
  CASE CAST(strftime('%w', substr(authored_at, 1, 10)) AS INTEGER)
    WHEN 0 THEN 'Sun' WHEN 1 THEN 'Mon' WHEN 2 THEN 'Tue'
    WHEN 3 THEN 'Wed' WHEN 4 THEN 'Thu' WHEN 5 THEN 'Fri' WHEN 6 THEN 'Sat'
  END as day,
  COUNT(*) as commits
FROM commits
GROUP BY day

-- Most modified files
SELECT new_path, COUNT(*) as times_modified, SUM(insertions) as total_lines
FROM diff_files
WHERE new_path IS NOT NULL
GROUP BY new_path
ORDER BY times_modified DESC
LIMIT 10

-- Daily activity
SELECT
  substr(authored_at, 1, 10) as date,
  COUNT(*) as commits,
  SUM(d.insertions) as lines_added
FROM commits c
JOIN diffs d ON d.commit_id = c.id
GROUP BY date
ORDER BY date DESC
LIMIT 7
```

### Using CTEs

```sql
WITH file_churn AS (
  SELECT
    new_path as path,
    COUNT(*) as modifications,
    SUM(insertions) as total_insertions
  FROM diff_files
  WHERE new_path IS NOT NULL
  GROUP BY new_path
  HAVING COUNT(*) > 2
)
SELECT * FROM file_churn
ORDER BY modifications DESC
LIMIT 10
```

### Joins

```sql
-- Commits with branch info
SELECT c.short_id, c.summary, b.name as branch
FROM commits c
JOIN branches b ON b.target_id = c.id

-- Find merge commits with their parents
SELECT c.summary, p.parent_id, p.parent_index
FROM commits c
JOIN commit_parents p ON p.commit_id = c.id
WHERE c.is_merge = 1
LIMIT 10
```

## License

MIT
