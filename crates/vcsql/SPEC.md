# VCSQL - Version Control SQL

**SQL query engine for Git repository data**

---

## Executive Summary

Git stores rich, structured data about repository history, but exposes it through dozens of specialized commands with inconsistent interfaces. VCSQL provides a unified SQL interface to query ALL git data—commits, branches, diffs, stashes, reflog, blame, config, and more—using standard SQL that developers already know.

```sql
SELECT author_name, COUNT(*) as commits
FROM commits
WHERE authored_at > datetime('now', '-30 days')
GROUP BY author_name
ORDER BY commits DESC;
```

---

## 1. Vision & Goals

### 1.1 Vision Statement

Make all git repository data queryable through a single, familiar interface. If git stores it, you can query it.

### 1.2 Goals

1. **Complete Coverage**: Expose ALL git data as SQL tables
2. **Full SQL Power**: JOINs, CTEs, window functions, aggregations—everything SQLite supports
3. **Zero Configuration**: Point at a repo and query immediately
4. **Multi-Repository**: Query across multiple repositories simultaneously
5. **Performance**: Handle repositories with 100k+ commits efficiently
6. **Familiar UX**: Follow ccq patterns for consistency

### 1.3 Non-Goals

- Modifying git data (read-only)
- Replacing git CLI for normal operations
- GitHub/GitLab API integration (future consideration)
- Real-time watching/streaming

---

## 2. Requirements

### 2.1 Functional Requirements

| ID | Requirement | Priority | Notes |
|----|-------------|----------|-------|
| **F1** | Query commit history with full metadata | P0 | Core feature |
| **F2** | Query branches (local and remote) | P0 | Include HEAD status |
| **F3** | Query tags (annotated and lightweight) | P0 | Include tagger info |
| **F4** | Query all references | P0 | Unified view |
| **F5** | Query stashes | P0 | What GQL lacks |
| **F6** | Query reflog | P0 | What GQL lacks |
| **F7** | Query per-commit diff stats | P0 | Files changed, insertions, deletions |
| **F8** | Query per-file diff details | P0 | Individual file changes |
| **F9** | Query repository config | P1 | Local, global, system |
| **F10** | Query remotes | P1 | URLs, refspecs |
| **F11** | Query submodules | P1 | Nested repos |
| **F12** | Query working directory status | P1 | Staged, modified, untracked |
| **F13** | Query blame data | P1 | Per-line attribution |
| **F14** | Query worktrees | P2 | Linked working trees |
| **F15** | Query hooks | P2 | Installed hooks |
| **F16** | Query notes | P2 | Git notes |
| **F17** | Support JOINs between all tables | P0 | Full relational power |
| **F18** | Support all SQLite aggregations | P0 | COUNT, SUM, AVG, etc. |
| **F19** | Support date/time functions | P0 | For temporal analysis |
| **F20** | Support LIKE/GLOB pattern matching | P0 | Text filtering |
| **F21** | Multi-repository queries | P1 | -r repo1 -r repo2 |
| **F22** | Output as table, JSON, JSONL, CSV | P0 | Multiple formats |
| **F23** | Interactive SQL shell | P2 | REPL mode |
| **F24** | Progressive disclosure help | P0 | Like ccq |

### 2.2 Non-Functional Requirements

| ID | Requirement | Target | Measurement |
|----|-------------|--------|-------------|
| **NF1** | Query 10k commits | < 2 seconds | Time to first result |
| **NF2** | Query 100k commits | < 10 seconds | Time to first result |
| **NF3** | Memory usage | < 500MB | For 100k commit repo |
| **NF4** | Binary size | < 15MB | Stripped release build |
| **NF5** | Startup time | < 100ms | Before query execution |
| **NF6** | Cross-platform | macOS, Linux, Windows | All tier-1 platforms |
| **NF7** | No runtime deps | Single binary | Self-contained |
| **NF8** | Graceful degradation | Partial results on error | Don't crash on edge cases |

### 2.3 Constraints

| Constraint | Rationale |
|------------|-----------|
| Use `git2` (libgit2) | Mature, full-featured, has stash/reflog/blame APIs |
| Use `rusqlite` (SQLite) | Full SQL support, proven, fast |
| Read-only access | Safety, simplicity |
| Follow ccq patterns | Consistency across tools |
| Rust implementation | Performance, single binary |

---

## 3. Complete Table Schema

### 3.1 Core Object Tables

#### `commits`

The primary table for commit metadata.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `id` | TEXT | No | Full SHA-1 hash (40 characters) |
| `short_id` | TEXT | No | Abbreviated hash (7 characters) |
| `tree_id` | TEXT | No | Tree object SHA |
| `author_name` | TEXT | No | Author's name |
| `author_email` | TEXT | No | Author's email |
| `authored_at` | DATETIME | No | When originally written |
| `committer_name` | TEXT | No | Committer's name |
| `committer_email` | TEXT | No | Committer's email |
| `committed_at` | DATETIME | No | When committed |
| `message` | TEXT | No | Full commit message |
| `summary` | TEXT | No | First line of message |
| `body` | TEXT | Yes | Message body (lines 2+) |
| `parent_count` | INTEGER | No | Number of parents (0=root, 1=normal, 2+=merge) |
| `is_merge` | BOOLEAN | No | True if parent_count > 1 |
| `gpg_signature` | TEXT | Yes | GPG signature if signed |
| `encoding` | TEXT | Yes | Message encoding if non-UTF8 |
| `repo` | TEXT | No | Repository path (for multi-repo) |

**Indexes**: `id` (primary), `authored_at`, `author_email`

---

#### `commit_parents`

Parent-child relationships enabling ancestry traversal.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `commit_id` | TEXT | No | Child commit SHA |
| `parent_id` | TEXT | No | Parent commit SHA |
| `parent_index` | INTEGER | No | 0=first parent, 1=second parent (merge), etc. |
| `repo` | TEXT | No | Repository path |

**Indexes**: `commit_id`, `parent_id`

**Usage**:
```sql
-- Find merge commits and what was merged
SELECT c.summary, p1.summary as main_parent, p2.summary as merged
FROM commits c
JOIN commit_parents cp1 ON cp1.commit_id = c.id AND cp1.parent_index = 0
JOIN commits p1 ON p1.id = cp1.parent_id
JOIN commit_parents cp2 ON cp2.commit_id = c.id AND cp2.parent_index = 1
JOIN commits p2 ON p2.id = cp2.parent_id
WHERE c.is_merge = 1;
```

---

#### `trees`

Tree (directory) objects.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `id` | TEXT | No | Tree object SHA |
| `commit_id` | TEXT | No | Commit this tree belongs to |
| `entry_count` | INTEGER | No | Number of entries in tree |
| `repo` | TEXT | No | Repository path |

---

#### `tree_entries`

Files and subdirectories within trees.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `tree_id` | TEXT | No | Parent tree SHA |
| `name` | TEXT | No | Entry filename |
| `path` | TEXT | No | Full path from repository root |
| `object_id` | TEXT | No | SHA of blob or subtree |
| `mode` | TEXT | No | File mode (100644, 100755, 040000, 120000, 160000) |
| `kind` | TEXT | No | 'blob', 'tree', or 'commit' (submodule) |
| `repo` | TEXT | No | Repository path |

**Mode values**:
- `100644` - Normal file
- `100755` - Executable file
- `040000` - Directory (tree)
- `120000` - Symbolic link
- `160000` - Submodule (gitlink)

---

#### `blobs`

File content objects.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `id` | TEXT | No | Blob SHA |
| `size` | INTEGER | No | Size in bytes |
| `is_binary` | BOOLEAN | No | Binary file detection |
| `content` | TEXT | Yes | Content if text and < 1MB |
| `repo` | TEXT | No | Repository path |

**Note**: Content is only populated for text files under 1MB to avoid memory issues.

---

### 3.2 Reference Tables

#### `branches`

Local and remote tracking branches.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Branch name (e.g., "main", "origin/main") |
| `full_name` | TEXT | No | Full refname (refs/heads/main) |
| `target_id` | TEXT | No | Commit SHA branch points to |
| `is_remote` | BOOLEAN | No | Remote tracking branch |
| `is_head` | BOOLEAN | No | Currently checked out |
| `remote_name` | TEXT | Yes | Remote name for remote branches |
| `upstream` | TEXT | Yes | Upstream branch name |
| `ahead` | INTEGER | Yes | Commits ahead of upstream |
| `behind` | INTEGER | Yes | Commits behind upstream |
| `repo` | TEXT | No | Repository path |

---

#### `tags`

Annotated and lightweight tags.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Tag name |
| `full_name` | TEXT | No | Full refname (refs/tags/v1.0) |
| `target_id` | TEXT | No | Tagged object SHA |
| `target_type` | TEXT | No | 'commit', 'tree', 'blob', or 'tag' |
| `is_annotated` | BOOLEAN | No | True if annotated tag |
| `tagger_name` | TEXT | Yes | Tagger name (annotated only) |
| `tagger_email` | TEXT | Yes | Tagger email (annotated only) |
| `tagged_at` | DATETIME | Yes | Tag creation time (annotated only) |
| `message` | TEXT | Yes | Tag message (annotated only) |
| `repo` | TEXT | No | Repository path |

---

#### `refs`

All references (unified view).

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Short name |
| `full_name` | TEXT | No | Full reference name |
| `target_id` | TEXT | No | Target SHA |
| `kind` | TEXT | No | 'branch', 'remote', 'tag', 'note', 'stash', 'other' |
| `is_symbolic` | BOOLEAN | No | Symbolic ref (like HEAD) |
| `symbolic_target` | TEXT | Yes | Target reference if symbolic |
| `repo` | TEXT | No | Repository path |

---

#### `stashes`

Stashed changes (WHAT GQL DOESN'T HAVE).

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `stash_index` | INTEGER | No | Stash index (0 = most recent) |
| `commit_id` | TEXT | No | Stash commit SHA |
| `message` | TEXT | No | Stash message |
| `author_name` | TEXT | No | Who stashed |
| `author_email` | TEXT | No | Email |
| `created_at` | DATETIME | No | When stashed |
| `branch` | TEXT | No | Branch when stashed |
| `repo` | TEXT | No | Repository path |

**Example queries**:
```sql
-- Stashes older than 30 days
SELECT * FROM stashes
WHERE created_at < datetime('now', '-30 days');

-- Stashes by branch
SELECT branch, COUNT(*) as stash_count
FROM stashes
GROUP BY branch;
```

---

#### `reflog`

Reference history (WHAT GQL DOESN'T HAVE).

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `ref_name` | TEXT | No | Reference name |
| `entry_index` | INTEGER | No | Entry index (0 = most recent) |
| `old_id` | TEXT | No | Previous SHA |
| `new_id` | TEXT | No | New SHA |
| `committer_name` | TEXT | No | Who made change |
| `committer_email` | TEXT | No | Email |
| `committed_at` | DATETIME | No | When changed |
| `message` | TEXT | No | Reflog message |
| `action` | TEXT | No | Extracted action (commit, checkout, merge, rebase, etc.) |
| `repo` | TEXT | No | Repository path |

**Example queries**:
```sql
-- Recent reflog for HEAD
SELECT action, message, committed_at
FROM reflog
WHERE ref_name = 'HEAD'
ORDER BY entry_index
LIMIT 20;

-- Count operations by type
SELECT action, COUNT(*) as count
FROM reflog
WHERE ref_name = 'HEAD'
GROUP BY action
ORDER BY count DESC;
```

---

### 3.3 Diff & Change Tables

#### `diffs`

Per-commit diff summary.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `commit_id` | TEXT | No | Commit SHA |
| `parent_id` | TEXT | Yes | Parent SHA (null for root commit) |
| `files_changed` | INTEGER | No | Number of files changed |
| `insertions` | INTEGER | No | Lines added |
| `deletions` | INTEGER | No | Lines removed |
| `repo` | TEXT | No | Repository path |

---

#### `diff_files`

Per-file changes within commits.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `commit_id` | TEXT | No | Commit SHA |
| `parent_id` | TEXT | Yes | Parent SHA |
| `old_path` | TEXT | Yes | Path before (null if added) |
| `new_path` | TEXT | Yes | Path after (null if deleted) |
| `status` | TEXT | No | 'A' added, 'D' deleted, 'M' modified, 'R' renamed, 'C' copied, 'T' typechange |
| `insertions` | INTEGER | No | Lines added in this file |
| `deletions` | INTEGER | No | Lines removed in this file |
| `is_binary` | BOOLEAN | No | Binary file |
| `similarity` | INTEGER | Yes | Rename/copy similarity percentage |
| `repo` | TEXT | No | Repository path |

**Example queries**:
```sql
-- Most frequently changed files
SELECT new_path as path, COUNT(*) as changes
FROM diff_files
WHERE status != 'D'
GROUP BY new_path
ORDER BY changes DESC
LIMIT 20;

-- Files with most churn
SELECT new_path as path,
       SUM(insertions + deletions) as churn,
       SUM(insertions) as added,
       SUM(deletions) as removed
FROM diff_files
WHERE status = 'M'
GROUP BY new_path
ORDER BY churn DESC
LIMIT 20;
```

---

#### `blame`

Per-line attribution (requires file path filter for performance).

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `path` | TEXT | No | File path |
| `line_number` | INTEGER | No | Current line number (1-indexed) |
| `commit_id` | TEXT | No | Commit that introduced line |
| `original_line` | INTEGER | No | Original line number in that commit |
| `original_path` | TEXT | No | Original file path (for renames) |
| `author_name` | TEXT | No | Author |
| `author_email` | TEXT | No | Email |
| `authored_at` | DATETIME | No | When written |
| `line_content` | TEXT | No | Line text content |
| `repo` | TEXT | No | Repository path |

**Note**: This table requires a WHERE clause on `path` for performance. Without it, we'd have to blame every file.

**Example queries**:
```sql
-- Who wrote main.rs?
SELECT author_name, COUNT(*) as lines
FROM blame
WHERE path = 'src/main.rs'
GROUP BY author_name
ORDER BY lines DESC;

-- Lines by age
SELECT
  CASE
    WHEN authored_at > datetime('now', '-7 days') THEN 'This week'
    WHEN authored_at > datetime('now', '-30 days') THEN 'This month'
    WHEN authored_at > datetime('now', '-365 days') THEN 'This year'
    ELSE 'Older'
  END as age,
  COUNT(*) as lines
FROM blame
WHERE path = 'src/main.rs'
GROUP BY age;
```

---

### 3.4 Configuration Tables

#### `config`

Git configuration values.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `level` | TEXT | No | 'system', 'global', 'local', 'worktree' |
| `section` | TEXT | No | Config section (e.g., 'core', 'user') |
| `subsection` | TEXT | Yes | Subsection (e.g., remote name) |
| `key` | TEXT | No | Config key |
| `name` | TEXT | No | Full name (section.key or section.subsection.key) |
| `value` | TEXT | No | Config value |
| `repo` | TEXT | No | Repository path |

**Example queries**:
```sql
-- All user config
SELECT name, value FROM config
WHERE section = 'user';

-- Compare local vs global
SELECT
  c1.name,
  c1.value as local_value,
  c2.value as global_value
FROM config c1
LEFT JOIN config c2 ON c1.name = c2.name AND c2.level = 'global'
WHERE c1.level = 'local';
```

---

#### `remotes`

Remote repositories.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Remote name |
| `url` | TEXT | No | Fetch URL |
| `push_url` | TEXT | Yes | Push URL if different |
| `fetch_refspec` | TEXT | Yes | Fetch refspec |
| `push_refspec` | TEXT | Yes | Push refspec |
| `repo` | TEXT | No | Repository path |

---

#### `submodules`

Nested repositories.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Submodule name |
| `path` | TEXT | No | Filesystem path |
| `url` | TEXT | No | Repository URL |
| `branch` | TEXT | Yes | Tracked branch |
| `head_id` | TEXT | Yes | Current HEAD SHA |
| `status` | TEXT | No | 'current', 'modified', 'uninitialized', 'added', 'deleted' |
| `repo` | TEXT | No | Repository path |

---

### 3.5 Working Directory Tables

#### `status`

Working directory status (staged, modified, untracked).

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `path` | TEXT | No | File path |
| `status_code` | TEXT | No | Two-character status (like git status --short) |
| `head_status` | TEXT | No | Status in HEAD (index vs HEAD) |
| `index_status` | TEXT | No | Status in index (worktree vs index) |
| `is_staged` | BOOLEAN | No | Changes in staging area |
| `is_modified` | BOOLEAN | No | Modified in working tree |
| `is_new` | BOOLEAN | No | Untracked file |
| `is_deleted` | BOOLEAN | No | Deleted |
| `is_renamed` | BOOLEAN | No | Renamed |
| `is_copied` | BOOLEAN | No | Copied |
| `is_ignored` | BOOLEAN | No | Ignored by .gitignore |
| `is_conflicted` | BOOLEAN | No | Has merge conflict |
| `repo` | TEXT | No | Repository path |

---

#### `worktrees`

Linked working trees.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Worktree name |
| `path` | TEXT | No | Filesystem path |
| `head_id` | TEXT | Yes | HEAD commit SHA |
| `branch` | TEXT | Yes | Checked out branch |
| `is_bare` | BOOLEAN | No | Bare worktree |
| `is_detached` | BOOLEAN | No | Detached HEAD |
| `is_locked` | BOOLEAN | No | Locked state |
| `lock_reason` | TEXT | Yes | Lock reason |
| `is_prunable` | BOOLEAN | No | Can be pruned |
| `repo` | TEXT | No | Repository path |

---

### 3.6 Operational Tables

#### `hooks`

Installed git hooks.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Hook name (pre-commit, post-merge, etc.) |
| `path` | TEXT | No | Full filesystem path |
| `is_executable` | BOOLEAN | No | Has execute permission |
| `is_sample` | BOOLEAN | No | Is a .sample file |
| `size` | INTEGER | No | File size in bytes |
| `repo` | TEXT | No | Repository path |

---

#### `notes`

Git notes.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `notes_ref` | TEXT | No | Notes reference (refs/notes/commits) |
| `target_id` | TEXT | No | Annotated object SHA |
| `note_id` | TEXT | No | Note blob SHA |
| `content` | TEXT | No | Note text content |
| `repo` | TEXT | No | Repository path |

---

### 3.7 Computed/Virtual Tables

These tables are computed on-demand from other data.

#### `contributors`

Aggregated contributor statistics.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `name` | TEXT | No | Contributor name |
| `email` | TEXT | No | Primary email |
| `emails` | TEXT | No | All known emails (JSON array) |
| `commit_count` | INTEGER | No | Total commits |
| `first_commit_at` | DATETIME | No | First contribution |
| `last_commit_at` | DATETIME | No | Most recent |
| `insertions` | INTEGER | No | Total lines added |
| `deletions` | INTEGER | No | Total lines removed |
| `files_touched` | INTEGER | No | Unique files modified |
| `repo` | TEXT | No | Repository path |

**Computed as**:
```sql
-- This is how the table is generated
SELECT
  author_name as name,
  author_email as email,
  json_group_array(DISTINCT author_email) as emails,
  COUNT(*) as commit_count,
  MIN(authored_at) as first_commit_at,
  MAX(authored_at) as last_commit_at,
  SUM(d.insertions) as insertions,
  SUM(d.deletions) as deletions,
  COUNT(DISTINCT df.new_path) as files_touched
FROM commits c
LEFT JOIN diffs d ON d.commit_id = c.id
LEFT JOIN diff_files df ON df.commit_id = c.id
GROUP BY author_name;
```

---

#### `file_history`

Track a file through renames.

| Column | Type | Nullable | Description |
|--------|------|----------|-------------|
| `current_path` | TEXT | No | Current/queried path |
| `path_at_commit` | TEXT | No | Path at this commit |
| `commit_id` | TEXT | No | Commit SHA |
| `change_type` | TEXT | No | How file changed |
| `authored_at` | DATETIME | No | Commit time |
| `author_name` | TEXT | No | Author |
| `summary` | TEXT | No | Commit summary |
| `repo` | TEXT | No | Repository path |

---

## 4. Architecture

### 4.1 Component Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                              CLI Layer                               │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐              │
│  │ Arg Parser  │  │ Output       │  │ Progressive    │              │
│  │ (clap)      │  │ Formatter    │  │ Help System    │              │
│  └─────────────┘  └──────────────┘  └────────────────┘              │
└────────────────────────────┬────────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────────┐
│                           SQL Engine                                 │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐              │
│  │ Query       │  │ Result       │  │ Virtual Table  │              │
│  │ Executor    │  │ Transformer  │  │ Manager        │              │
│  │ (rusqlite)  │  │              │  │                │              │
│  └─────────────┘  └──────────────┘  └────────────────┘              │
└────────────────────────────┬────────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────────┐
│                        Data Provider Layer                           │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ │
│  │commits │ │branches│ │stashes │ │reflog  │ │diffs   │ │blame   │ │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ │
│  │tags    │ │refs    │ │config  │ │remotes │ │status  │ │hooks   │ │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ │
└────────────────────────────┬────────────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────────────┐
│                          Git Layer                                   │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐      │
│  │ Repository      │  │ Multi-Repo      │  │ Cache           │      │
│  │ Wrapper (git2)  │  │ Manager         │  │ Manager         │      │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘      │
└────────────────────────────┬────────────────────────────────────────┘
                             │
                      ┌──────▼──────┐
                      │    .git/    │
                      │  directory  │
                      └─────────────┘
```

### 4.2 Data Flow

```
User Query
    │
    ▼
┌─────────────────┐
│ Parse SQL       │ ◄─── Validate syntax
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Identify Tables │ ◄─── Extract table names from query
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Load Providers  │ ◄─── Only load tables used in query
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Populate SQLite │ ◄─── In-memory database
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Execute Query   │ ◄─── Let SQLite do the work
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Format Output   │ ◄─── Table, JSON, CSV
└────────┬────────┘
         │
         ▼
    Results
```

### 4.3 Module Structure

```
vcsql/
├── Cargo.toml
├── src/
│   ├── main.rs                 # Entry point, CLI
│   ├── lib.rs                  # Library exports
│   │
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── args.rs             # Clap definitions
│   │   ├── commands.rs         # Subcommand handlers
│   │   ├── output.rs           # Formatters
│   │   └── help.rs             # Progressive disclosure
│   │
│   ├── sql/
│   │   ├── mod.rs
│   │   ├── engine.rs           # SQLite wrapper
│   │   ├── schema.rs           # Table definitions
│   │   ├── loader.rs           # Lazy loading logic
│   │   └── functions.rs        # Custom SQL functions
│   │
│   ├── providers/
│   │   ├── mod.rs              # Provider trait
│   │   ├── commits.rs
│   │   ├── commit_parents.rs
│   │   ├── branches.rs
│   │   ├── tags.rs
│   │   ├── refs.rs
│   │   ├── stashes.rs          # Key differentiator
│   │   ├── reflog.rs           # Key differentiator
│   │   ├── diffs.rs
│   │   ├── diff_files.rs
│   │   ├── blame.rs
│   │   ├── config.rs
│   │   ├── remotes.rs
│   │   ├── submodules.rs
│   │   ├── status.rs
│   │   ├── worktrees.rs
│   │   ├── hooks.rs
│   │   ├── notes.rs
│   │   └── contributors.rs
│   │
│   ├── git/
│   │   ├── mod.rs
│   │   ├── repository.rs       # git2 wrapper
│   │   └── multi_repo.rs       # Multi-repo support
│   │
│   ├── cache/
│   │   ├── mod.rs
│   │   └── blame.rs            # Blame caching
│   │
│   └── error.rs                # Error types
│
├── tests/
│   ├── integration/
│   │   ├── basic_queries.rs
│   │   ├── joins.rs
│   │   ├── aggregations.rs
│   │   └── multi_repo.rs
│   └── fixtures/
│       └── test_repo/          # Test repository
│
└── benches/
    └── large_repo.rs           # Performance benchmarks
```

---

## 5. CLI Design

### 5.1 Primary Interface

```bash
# Direct SQL (default command, like ccq)
vcsql "SELECT * FROM commits LIMIT 10"

# With repository path
vcsql -r /path/to/repo "SELECT * FROM branches"

# Multiple repositories
vcsql -r repo1 -r repo2 "SELECT repo, COUNT(*) FROM commits GROUP BY repo"

# Output formats
vcsql -f json "SELECT * FROM stashes"
vcsql -f csv "SELECT * FROM commits" > commits.csv
vcsql -f jsonl "SELECT * FROM reflog"
```

### 5.2 Command Structure

```
vcsql [OPTIONS] [SQL]
vcsql <COMMAND>

Commands:
  tables      List all available tables
  schema      Show table schema(s)
  shell       Interactive SQL shell
  examples    Show example queries
  help        Print help information

Arguments:
  [SQL]  SQL query to execute

Options:
  -r, --repo <PATH>      Repository path (repeatable, default: current dir)
  -f, --format <FORMAT>  Output format: table, json, jsonl, csv [default: table]
  -H, --no-header        Omit header row (table/csv)
  -q, --quiet            Suppress non-essential output
  -v, --verbose          Verbose output (timing, row counts)
  -h, --help             Print help
  -V, --version          Print version
```

### 5.3 Subcommands

#### `vcsql tables`

```
$ vcsql tables

Available tables:

  CORE
    commits          Commit history and metadata
    commit_parents   Parent-child relationships

  REFERENCES
    branches         Local and remote branches
    tags             Annotated and lightweight tags
    refs             All references (unified view)
    stashes          Stashed changes
    reflog           Reference history

  CHANGES
    diffs            Per-commit diff summary
    diff_files       Per-file changes
    blame            Per-line attribution (requires path filter)

  CONFIGURATION
    config           Git configuration
    remotes          Remote repositories
    submodules       Nested repositories

  WORKING DIRECTORY
    status           Staged/modified/untracked files
    worktrees        Linked working trees

  OPERATIONAL
    hooks            Installed git hooks
    notes            Git notes

  COMPUTED
    contributors     Aggregated contributor stats

Use 'vcsql schema <table>' for column details.
```

#### `vcsql schema`

```
$ vcsql schema commits

TABLE: commits
Commit history and metadata

COLUMNS:
  id               TEXT      Full SHA-1 hash
  short_id         TEXT      Abbreviated hash (7 chars)
  tree_id          TEXT      Tree object SHA
  author_name      TEXT      Author's name
  author_email     TEXT      Author's email
  authored_at      DATETIME  When originally written
  committer_name   TEXT      Committer's name
  committer_email  TEXT      Committer's email
  committed_at     DATETIME  When committed
  message          TEXT      Full commit message
  summary          TEXT      First line of message
  body             TEXT      Message body (nullable)
  parent_count     INTEGER   Number of parents
  is_merge         BOOLEAN   True if merge commit
  gpg_signature    TEXT      GPG signature (nullable)
  repo             TEXT      Repository path

EXAMPLES:
  SELECT short_id, summary, author_name FROM commits LIMIT 10;
  SELECT author_name, COUNT(*) FROM commits GROUP BY author_name;
```

#### `vcsql examples`

```
$ vcsql examples

VCSQL EXAMPLE QUERIES
═══════════════════════════════════════════════════════════════

BASIC QUERIES
───────────────────────────────────────────────────────────────

  # Recent commits
  vcsql "SELECT short_id, summary, authored_at
         FROM commits
         ORDER BY authored_at DESC
         LIMIT 10"

  # Current branch
  vcsql "SELECT name FROM branches WHERE is_head = 1"

  # All stashes (what GQL can't do!)
  vcsql "SELECT stash_index, message, created_at FROM stashes"

ANALYTICS
───────────────────────────────────────────────────────────────

  # Commits by author
  vcsql "SELECT author_name, COUNT(*) as commits
         FROM commits
         GROUP BY author_name
         ORDER BY commits DESC"

  # Commits by day of week
  vcsql "SELECT
           CASE CAST(strftime('%w', authored_at) AS INTEGER)
             WHEN 0 THEN 'Sun' WHEN 1 THEN 'Mon'
             WHEN 2 THEN 'Tue' WHEN 3 THEN 'Wed'
             WHEN 4 THEN 'Thu' WHEN 5 THEN 'Fri' WHEN 6 THEN 'Sat'
           END as day,
           COUNT(*) as commits
         FROM commits
         GROUP BY day"

  # Hottest files
  vcsql "SELECT new_path, COUNT(*) as changes
         FROM diff_files
         WHERE status != 'D'
         GROUP BY new_path
         ORDER BY changes DESC
         LIMIT 20"

STASH & REFLOG (vcsql exclusives)
───────────────────────────────────────────────────────────────

  # Old stashes
  vcsql "SELECT * FROM stashes
         WHERE created_at < datetime('now', '-30 days')"

  # Recent reflog
  vcsql "SELECT action, message, committed_at
         FROM reflog
         WHERE ref_name = 'HEAD'
         ORDER BY entry_index
         LIMIT 20"

  # Operations by type
  vcsql "SELECT action, COUNT(*) as count
         FROM reflog
         GROUP BY action
         ORDER BY count DESC"

JOINS
───────────────────────────────────────────────────────────────

  # Commits with their diffs
  vcsql "SELECT c.short_id, c.summary,
                d.files_changed, d.insertions, d.deletions
         FROM commits c
         JOIN diffs d ON d.commit_id = c.id
         ORDER BY d.insertions DESC
         LIMIT 10"

  # Merge commits with parents
  vcsql "SELECT c.summary, p1.summary as main, p2.summary as merged
         FROM commits c
         JOIN commit_parents cp1 ON cp1.commit_id = c.id
              AND cp1.parent_index = 0
         JOIN commits p1 ON p1.id = cp1.parent_id
         JOIN commit_parents cp2 ON cp2.commit_id = c.id
              AND cp2.parent_index = 1
         JOIN commits p2 ON p2.id = cp2.parent_id
         WHERE c.is_merge = 1"
```

### 5.4 Output Formats

#### Table (default)
```
$ vcsql "SELECT short_id, summary, author_name FROM commits LIMIT 3"

short_id  summary                                    author_name
────────  ─────────────────────────────────────────  ───────────
abc1234   Add user authentication                   Alice
def5678   Fix login bug                             Bob
ghi9012   Update dependencies                       Carol
```

#### JSON
```
$ vcsql -f json "SELECT short_id, summary FROM commits LIMIT 2"
[
  {"short_id": "abc1234", "summary": "Add user authentication"},
  {"short_id": "def5678", "summary": "Fix login bug"}
]
```

#### JSONL
```
$ vcsql -f jsonl "SELECT short_id, summary FROM commits LIMIT 2"
{"short_id": "abc1234", "summary": "Add user authentication"}
{"short_id": "def5678", "summary": "Fix login bug"}
```

#### CSV
```
$ vcsql -f csv "SELECT short_id, summary FROM commits LIMIT 2"
short_id,summary
abc1234,Add user authentication
def5678,Fix login bug
```

---

## 6. Feature Comparison

### 6.1 vcsql vs gitql (GQL)

| Feature | vcsql | gitql |
|---------|-------|-------|
| **SQL Engine** | SQLite (full SQL) | Custom (limited) |
| **JOINs** | Full support | No |
| **Table aliases** | Yes | No |
| **CTEs (WITH)** | Yes | No |
| **Window functions** | Yes | No |
| **Subqueries** | Yes | Limited |
| **stashes table** | Yes | No |
| **reflog table** | Yes | No |
| **blame table** | Yes | No |
| **config table** | Yes | No |
| **worktrees table** | Yes | No |
| **hooks table** | Yes | No |
| **Multi-repo** | Yes | Yes |
| **Output formats** | 4 (table/json/jsonl/csv) | 4 |
| **Interactive shell** | Planned | Yes |

### 6.2 vcsql vs ccq

| Aspect | vcsql | ccq |
|--------|-------|-----|
| **Data source** | .git directory | Claude Code JSONL |
| **SQL engine** | rusqlite (SQLite) | GlueSQL |
| **Write support** | No (read-only) | Yes (with --write) |
| **CLI style** | Same | Same |
| **Output formats** | Same | Same |
| **Architecture pattern** | Same | Same |

---

## 7. Implementation Phases

### Phase 1: Foundation (MVP)
**Goal**: Basic commit querying with full SQL

- [ ] Project scaffolding (Cargo.toml, CI, linting)
- [ ] CLI skeleton with clap
- [ ] git2 repository wrapper
- [ ] rusqlite integration
- [ ] `commits` table
- [ ] `commit_parents` table
- [ ] `branches` table
- [ ] Table output format
- [ ] JSON output format
- [ ] Basic help/tables/schema commands

**Exit Criteria**: Can run `vcsql "SELECT * FROM commits LIMIT 10"` with JOINs

### Phase 2: Core Tables
**Goal**: All reference and basic diff tables

- [ ] `tags` table
- [ ] `refs` table
- [ ] `stashes` table (key differentiator!)
- [ ] `reflog` table (key differentiator!)
- [ ] `diffs` table
- [ ] `diff_files` table
- [ ] CSV output format
- [ ] JSONL output format
- [ ] Multi-repository support

**Exit Criteria**: All core tables queryable, multi-repo works

### Phase 3: Extended Tables
**Goal**: Configuration and working directory

- [ ] `config` table
- [ ] `remotes` table
- [ ] `submodules` table
- [ ] `status` table
- [ ] `blame` table with path filtering
- [ ] Blame caching

**Exit Criteria**: Full configuration and status querying

### Phase 4: Advanced Features
**Goal**: Remaining tables and polish

- [ ] `worktrees` table
- [ ] `hooks` table
- [ ] `notes` table
- [ ] `contributors` computed table
- [ ] Interactive shell (optional)
- [ ] Performance optimization
- [ ] Comprehensive test suite

**Exit Criteria**: Feature complete, production ready

### Phase 5: Distribution
**Goal**: Easy installation

- [ ] GitHub releases with binaries
- [ ] Homebrew formula
- [ ] cargo install support
- [ ] Documentation website

---

## 8. Dependencies

```toml
[dependencies]
# Git access - libgit2 bindings (has stash, reflog, blame)
git2 = "0.18"

# SQL engine - full SQLite with virtual table support
rusqlite = { version = "0.31", features = ["bundled", "vtab"] }

# CLI framework
clap = { version = "4", features = ["derive"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
csv = "1"

# Date/time handling
chrono = { version = "0.4", features = ["serde"] }

# Table output
tabled = "0.15"

# Error handling
thiserror = "1"
anyhow = "1"

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

---

## 9. Testing Strategy

### 9.1 Test Categories

| Category | Description | Coverage Target |
|----------|-------------|-----------------|
| Unit | Individual provider functions | 80% |
| Integration | Full query execution | Key scenarios |
| CLI | Command parsing, output | All formats |
| Performance | Large repo benchmarks | Regression tracking |

### 9.2 Test Repository

Create a fixture repository with known data:
- 100 commits with various patterns
- Multiple branches
- Tags (annotated and lightweight)
- Stashes
- Submodules
- Various file changes

### 9.3 Key Test Cases

```rust
// Commit queries
assert_query("SELECT COUNT(*) FROM commits", expect: 100);
assert_query("SELECT * FROM commits WHERE is_merge = 1", expect: 5);

// Stash queries (what makes us different)
assert_query("SELECT * FROM stashes", expect: 3);
assert_query("SELECT * FROM stashes WHERE branch = 'main'", expect: 2);

// Reflog queries
assert_query("SELECT COUNT(*) FROM reflog WHERE ref_name = 'HEAD'", expect: 50);

// JOINs (what GQL can't do)
assert_query("SELECT c.summary, d.insertions
              FROM commits c
              JOIN diffs d ON d.commit_id = c.id",
             expect: 100);

// Multi-repo
assert_query_multi("SELECT repo, COUNT(*) FROM commits GROUP BY repo",
                   repos: [repo1, repo2],
                   expect: 2);
```

---

## 10. Performance Considerations

### 10.1 Optimization Strategies

| Strategy | Description | Benefit |
|----------|-------------|---------|
| Lazy loading | Only load tables used in query | Memory, startup time |
| Streaming | Stream large results | Memory for big repos |
| Blame caching | Cache expensive blame results | Repeat query speed |
| Connection pooling | Reuse SQLite connections | Shell mode speed |
| Parallel loading | Load independent tables in parallel | Startup time |

### 10.2 Benchmarks to Track

| Scenario | Target | Notes |
|----------|--------|-------|
| Cold start, simple query (10k commits) | < 1s | First query |
| Warm query (cached) | < 100ms | Repeat query |
| Full repo load (100k commits) | < 10s | All tables |
| Blame single file | < 500ms | With caching |
| Multi-repo (5 repos) | < 5s | Parallel load |

---

## 11. Future Considerations

### 11.1 Potential Extensions

| Feature | Description | Priority |
|---------|-------------|----------|
| GitHub integration | Query PRs, issues, CI | Medium |
| GitLab integration | MRs, pipelines | Medium |
| Visualization | Built-in charts | Low |
| Watch mode | Live query updates | Low |
| Index persistence | SQLite file cache | Medium |
| Custom functions | Git-specific SQL functions | Medium |
| Plugin system | Custom providers | Low |

### 11.2 Custom SQL Functions (Future)

```sql
-- Potential custom functions
SELECT git_short(id) FROM commits;  -- Shorten SHA
SELECT git_describe(id) FROM commits;  -- Like git describe
SELECT git_is_ancestor(id1, id2) FROM commits;  -- Ancestry check
SELECT git_path_exists(tree_id, 'src/main.rs') FROM commits;  -- File existence
```

---

## 12. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Query correctness | 100% | Test suite passes |
| Performance | Meet NF targets | Benchmark suite |
| CLI usability | Intuitive | User feedback |
| Documentation | Complete | All tables documented |
| Cross-platform | Works everywhere | CI on all platforms |
| Binary size | < 15MB | Release build |

---

## Appendix A: Complete Example Session

```bash
$ cd my-project

$ vcsql tables
Available tables: commits, branches, tags, stashes, reflog, ...

$ vcsql "SELECT COUNT(*) as total FROM commits"
total
─────
1247

$ vcsql "SELECT author_name, COUNT(*) as n FROM commits GROUP BY author_name ORDER BY n DESC LIMIT 5"
author_name    n
───────────    ────
Alice          523
Bob            312
Carol          198
Dave           142
Eve             72

$ vcsql "SELECT * FROM stashes"
stash_index  message              created_at           branch
───────────  ───────────────────  ───────────────────  ──────
0            WIP: new feature     2024-01-15 14:30:00  main
1            debug stuff          2024-01-10 09:15:00  feature

$ vcsql "SELECT action, COUNT(*) FROM reflog WHERE ref_name = 'HEAD' GROUP BY action"
action      COUNT(*)
──────────  ────────
commit      892
checkout    156
merge        45
rebase       32
reset        12

$ vcsql -f json "SELECT short_id, summary FROM commits LIMIT 2"
[
  {"short_id": "abc1234", "summary": "Add feature X"},
  {"short_id": "def5678", "summary": "Fix bug Y"}
]

$ vcsql "SELECT c.summary, d.insertions, d.deletions
         FROM commits c
         JOIN diffs d ON d.commit_id = c.id
         WHERE d.insertions > 100
         ORDER BY d.insertions DESC
         LIMIT 5"
summary                          insertions  deletions
───────────────────────────────  ──────────  ─────────
Initial commit                   5234        0
Add vendor dependencies          2341        12
Refactor auth module             892         756
Import legacy code               654         0
New API endpoints                432         23
```

---

## Appendix B: Comparison with git Commands

| Task | git command | vcsql |
|------|-------------|-------|
| Recent commits | `git log -10` | `SELECT * FROM commits LIMIT 10` |
| Commits by author | `git shortlog -sn` | `SELECT author_name, COUNT(*) FROM commits GROUP BY author_name` |
| List branches | `git branch -a` | `SELECT * FROM branches` |
| List stashes | `git stash list` | `SELECT * FROM stashes` |
| Show reflog | `git reflog` | `SELECT * FROM reflog WHERE ref_name = 'HEAD'` |
| Files changed in commit | `git show --stat abc123` | `SELECT * FROM diff_files WHERE commit_id LIKE 'abc123%'` |
| Blame file | `git blame file.rs` | `SELECT * FROM blame WHERE path = 'file.rs'` |
| Config values | `git config --list` | `SELECT * FROM config` |

The difference: with vcsql, you can JOIN, filter, aggregate, and combine all of these in a single query.
