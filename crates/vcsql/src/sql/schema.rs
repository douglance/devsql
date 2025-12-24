//! Table schema definitions for vcsql.
//!
//! This module defines the schema for all 17 queryable Git tables.

use std::collections::HashMap;

/// Metadata about a table column.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name as used in SQL queries.
    pub name: &'static str,
    /// SQLite type (TEXT, INTEGER, DATETIME, BOOLEAN).
    pub sql_type: &'static str,
    /// Whether the column can contain NULL values.
    pub nullable: bool,
    /// Human-readable description of the column.
    pub description: &'static str,
}

/// Metadata about a queryable table.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Table name as used in SQL queries.
    pub name: &'static str,
    /// Human-readable description of the table.
    pub description: &'static str,
    /// Category grouping (CORE, REFERENCES, CHANGES, etc.).
    pub category: &'static str,
    /// Column definitions for this table.
    pub columns: &'static [ColumnInfo],
    /// SQL CREATE TABLE statement.
    pub create_sql: &'static str,
}

// ============================================================================
// CORE TABLES
// ============================================================================

pub static COMMITS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "id",
        sql_type: "TEXT",
        nullable: false,
        description: "Full SHA-1 hash (40 characters)",
    },
    ColumnInfo {
        name: "short_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Abbreviated hash (7 characters)",
    },
    ColumnInfo {
        name: "tree_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Tree object SHA",
    },
    ColumnInfo {
        name: "author_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Author's name",
    },
    ColumnInfo {
        name: "author_email",
        sql_type: "TEXT",
        nullable: false,
        description: "Author's email",
    },
    ColumnInfo {
        name: "authored_at",
        sql_type: "DATETIME",
        nullable: false,
        description: "When originally written",
    },
    ColumnInfo {
        name: "committer_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Committer's name",
    },
    ColumnInfo {
        name: "committer_email",
        sql_type: "TEXT",
        nullable: false,
        description: "Committer's email",
    },
    ColumnInfo {
        name: "committed_at",
        sql_type: "DATETIME",
        nullable: false,
        description: "When committed",
    },
    ColumnInfo {
        name: "message",
        sql_type: "TEXT",
        nullable: false,
        description: "Full commit message",
    },
    ColumnInfo {
        name: "summary",
        sql_type: "TEXT",
        nullable: false,
        description: "First line of message",
    },
    ColumnInfo {
        name: "body",
        sql_type: "TEXT",
        nullable: true,
        description: "Message body (lines 2+)",
    },
    ColumnInfo {
        name: "parent_count",
        sql_type: "INTEGER",
        nullable: false,
        description: "Number of parents",
    },
    ColumnInfo {
        name: "is_merge",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "True if merge commit",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static COMMIT_PARENTS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "commit_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Child commit SHA",
    },
    ColumnInfo {
        name: "parent_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Parent commit SHA",
    },
    ColumnInfo {
        name: "parent_index",
        sql_type: "INTEGER",
        nullable: false,
        description: "Parent order (0=first)",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// REFERENCE TABLES
// ============================================================================

pub static BRANCHES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Branch name",
    },
    ColumnInfo {
        name: "full_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Full refname",
    },
    ColumnInfo {
        name: "target_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Commit SHA",
    },
    ColumnInfo {
        name: "is_remote",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Remote tracking branch",
    },
    ColumnInfo {
        name: "is_head",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Currently checked out",
    },
    ColumnInfo {
        name: "remote_name",
        sql_type: "TEXT",
        nullable: true,
        description: "Remote name",
    },
    ColumnInfo {
        name: "upstream",
        sql_type: "TEXT",
        nullable: true,
        description: "Upstream branch",
    },
    ColumnInfo {
        name: "ahead",
        sql_type: "INTEGER",
        nullable: true,
        description: "Commits ahead",
    },
    ColumnInfo {
        name: "behind",
        sql_type: "INTEGER",
        nullable: true,
        description: "Commits behind",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static TAGS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Tag name",
    },
    ColumnInfo {
        name: "full_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Full refname",
    },
    ColumnInfo {
        name: "target_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Tagged object SHA",
    },
    ColumnInfo {
        name: "target_type",
        sql_type: "TEXT",
        nullable: false,
        description: "commit/tree/blob/tag",
    },
    ColumnInfo {
        name: "is_annotated",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Annotated tag",
    },
    ColumnInfo {
        name: "tagger_name",
        sql_type: "TEXT",
        nullable: true,
        description: "Tagger name",
    },
    ColumnInfo {
        name: "tagger_email",
        sql_type: "TEXT",
        nullable: true,
        description: "Tagger email",
    },
    ColumnInfo {
        name: "tagged_at",
        sql_type: "DATETIME",
        nullable: true,
        description: "Tag creation time",
    },
    ColumnInfo {
        name: "message",
        sql_type: "TEXT",
        nullable: true,
        description: "Tag message",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static REFS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Short name",
    },
    ColumnInfo {
        name: "full_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Full reference name",
    },
    ColumnInfo {
        name: "target_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Target SHA",
    },
    ColumnInfo {
        name: "kind",
        sql_type: "TEXT",
        nullable: false,
        description: "branch/remote/tag/note/stash/other",
    },
    ColumnInfo {
        name: "is_symbolic",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Symbolic ref",
    },
    ColumnInfo {
        name: "symbolic_target",
        sql_type: "TEXT",
        nullable: true,
        description: "Target reference",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static STASHES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "stash_index",
        sql_type: "INTEGER",
        nullable: false,
        description: "Stash index (0=most recent)",
    },
    ColumnInfo {
        name: "commit_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Stash commit SHA",
    },
    ColumnInfo {
        name: "message",
        sql_type: "TEXT",
        nullable: false,
        description: "Stash message",
    },
    ColumnInfo {
        name: "author_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Who stashed",
    },
    ColumnInfo {
        name: "author_email",
        sql_type: "TEXT",
        nullable: false,
        description: "Email",
    },
    ColumnInfo {
        name: "created_at",
        sql_type: "DATETIME",
        nullable: false,
        description: "When stashed",
    },
    ColumnInfo {
        name: "branch",
        sql_type: "TEXT",
        nullable: false,
        description: "Branch when stashed",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static REFLOG_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "ref_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Reference name",
    },
    ColumnInfo {
        name: "entry_index",
        sql_type: "INTEGER",
        nullable: false,
        description: "Entry index (0=most recent)",
    },
    ColumnInfo {
        name: "old_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Previous SHA",
    },
    ColumnInfo {
        name: "new_id",
        sql_type: "TEXT",
        nullable: false,
        description: "New SHA",
    },
    ColumnInfo {
        name: "committer_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Who made change",
    },
    ColumnInfo {
        name: "committer_email",
        sql_type: "TEXT",
        nullable: false,
        description: "Email",
    },
    ColumnInfo {
        name: "committed_at",
        sql_type: "DATETIME",
        nullable: false,
        description: "When changed",
    },
    ColumnInfo {
        name: "message",
        sql_type: "TEXT",
        nullable: false,
        description: "Reflog message",
    },
    ColumnInfo {
        name: "action",
        sql_type: "TEXT",
        nullable: false,
        description: "Action type",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// DIFF & CHANGE TABLES
// ============================================================================

pub static DIFFS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "commit_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Commit SHA",
    },
    ColumnInfo {
        name: "parent_id",
        sql_type: "TEXT",
        nullable: true,
        description: "Parent SHA",
    },
    ColumnInfo {
        name: "files_changed",
        sql_type: "INTEGER",
        nullable: false,
        description: "Files changed",
    },
    ColumnInfo {
        name: "insertions",
        sql_type: "INTEGER",
        nullable: false,
        description: "Lines added",
    },
    ColumnInfo {
        name: "deletions",
        sql_type: "INTEGER",
        nullable: false,
        description: "Lines removed",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static DIFF_FILES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "commit_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Commit SHA",
    },
    ColumnInfo {
        name: "parent_id",
        sql_type: "TEXT",
        nullable: true,
        description: "Parent SHA",
    },
    ColumnInfo {
        name: "old_path",
        sql_type: "TEXT",
        nullable: true,
        description: "Path before",
    },
    ColumnInfo {
        name: "new_path",
        sql_type: "TEXT",
        nullable: true,
        description: "Path after",
    },
    ColumnInfo {
        name: "status",
        sql_type: "TEXT",
        nullable: false,
        description: "A/D/M/R/C/T",
    },
    ColumnInfo {
        name: "insertions",
        sql_type: "INTEGER",
        nullable: false,
        description: "Lines added",
    },
    ColumnInfo {
        name: "deletions",
        sql_type: "INTEGER",
        nullable: false,
        description: "Lines removed",
    },
    ColumnInfo {
        name: "is_binary",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Binary file",
    },
    ColumnInfo {
        name: "similarity",
        sql_type: "INTEGER",
        nullable: true,
        description: "Rename similarity %",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static BLAME_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "path",
        sql_type: "TEXT",
        nullable: false,
        description: "File path",
    },
    ColumnInfo {
        name: "line_number",
        sql_type: "INTEGER",
        nullable: false,
        description: "Line number",
    },
    ColumnInfo {
        name: "commit_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Commit that introduced line",
    },
    ColumnInfo {
        name: "original_line",
        sql_type: "INTEGER",
        nullable: false,
        description: "Original line number",
    },
    ColumnInfo {
        name: "original_path",
        sql_type: "TEXT",
        nullable: false,
        description: "Original file path",
    },
    ColumnInfo {
        name: "author_name",
        sql_type: "TEXT",
        nullable: false,
        description: "Author",
    },
    ColumnInfo {
        name: "author_email",
        sql_type: "TEXT",
        nullable: false,
        description: "Email",
    },
    ColumnInfo {
        name: "authored_at",
        sql_type: "DATETIME",
        nullable: false,
        description: "When written",
    },
    ColumnInfo {
        name: "line_content",
        sql_type: "TEXT",
        nullable: false,
        description: "Line text",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// CONFIGURATION TABLES
// ============================================================================

pub static CONFIG_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "level",
        sql_type: "TEXT",
        nullable: false,
        description: "system/global/local",
    },
    ColumnInfo {
        name: "section",
        sql_type: "TEXT",
        nullable: false,
        description: "Config section",
    },
    ColumnInfo {
        name: "subsection",
        sql_type: "TEXT",
        nullable: true,
        description: "Subsection",
    },
    ColumnInfo {
        name: "key",
        sql_type: "TEXT",
        nullable: false,
        description: "Config key",
    },
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Full name",
    },
    ColumnInfo {
        name: "value",
        sql_type: "TEXT",
        nullable: false,
        description: "Config value",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static REMOTES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Remote name",
    },
    ColumnInfo {
        name: "url",
        sql_type: "TEXT",
        nullable: true,
        description: "Fetch URL",
    },
    ColumnInfo {
        name: "push_url",
        sql_type: "TEXT",
        nullable: true,
        description: "Push URL",
    },
    ColumnInfo {
        name: "fetch_refspec",
        sql_type: "TEXT",
        nullable: true,
        description: "Fetch refspec",
    },
    ColumnInfo {
        name: "push_refspec",
        sql_type: "TEXT",
        nullable: true,
        description: "Push refspec",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static SUBMODULES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Submodule name",
    },
    ColumnInfo {
        name: "path",
        sql_type: "TEXT",
        nullable: false,
        description: "Filesystem path",
    },
    ColumnInfo {
        name: "url",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository URL",
    },
    ColumnInfo {
        name: "branch",
        sql_type: "TEXT",
        nullable: true,
        description: "Tracked branch",
    },
    ColumnInfo {
        name: "head_id",
        sql_type: "TEXT",
        nullable: true,
        description: "Current HEAD SHA",
    },
    ColumnInfo {
        name: "status",
        sql_type: "TEXT",
        nullable: false,
        description: "current/modified/uninitialized",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// WORKING DIRECTORY TABLES
// ============================================================================

pub static STATUS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "path",
        sql_type: "TEXT",
        nullable: false,
        description: "File path",
    },
    ColumnInfo {
        name: "status_code",
        sql_type: "TEXT",
        nullable: false,
        description: "Two-character status",
    },
    ColumnInfo {
        name: "head_status",
        sql_type: "TEXT",
        nullable: false,
        description: "Index vs HEAD",
    },
    ColumnInfo {
        name: "index_status",
        sql_type: "TEXT",
        nullable: false,
        description: "Worktree vs index",
    },
    ColumnInfo {
        name: "is_staged",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "In staging area",
    },
    ColumnInfo {
        name: "is_modified",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Modified",
    },
    ColumnInfo {
        name: "is_new",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Untracked",
    },
    ColumnInfo {
        name: "is_deleted",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Deleted",
    },
    ColumnInfo {
        name: "is_renamed",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Renamed",
    },
    ColumnInfo {
        name: "is_copied",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Copied",
    },
    ColumnInfo {
        name: "is_ignored",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Ignored",
    },
    ColumnInfo {
        name: "is_conflicted",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Conflicted",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static WORKTREES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Worktree name",
    },
    ColumnInfo {
        name: "path",
        sql_type: "TEXT",
        nullable: true,
        description: "Filesystem path",
    },
    ColumnInfo {
        name: "head_id",
        sql_type: "TEXT",
        nullable: true,
        description: "HEAD commit SHA",
    },
    ColumnInfo {
        name: "branch",
        sql_type: "TEXT",
        nullable: true,
        description: "Checked out branch",
    },
    ColumnInfo {
        name: "is_bare",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Bare worktree",
    },
    ColumnInfo {
        name: "is_detached",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Detached HEAD",
    },
    ColumnInfo {
        name: "is_locked",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Locked state",
    },
    ColumnInfo {
        name: "lock_reason",
        sql_type: "TEXT",
        nullable: true,
        description: "Lock reason",
    },
    ColumnInfo {
        name: "is_prunable",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Can be pruned",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// OPERATIONAL TABLES
// ============================================================================

pub static HOOKS_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "name",
        sql_type: "TEXT",
        nullable: false,
        description: "Hook name",
    },
    ColumnInfo {
        name: "path",
        sql_type: "TEXT",
        nullable: false,
        description: "Full path",
    },
    ColumnInfo {
        name: "is_executable",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Has execute permission",
    },
    ColumnInfo {
        name: "is_sample",
        sql_type: "BOOLEAN",
        nullable: false,
        description: "Is a .sample file",
    },
    ColumnInfo {
        name: "size",
        sql_type: "INTEGER",
        nullable: false,
        description: "File size in bytes",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

pub static NOTES_COLUMNS: &[ColumnInfo] = &[
    ColumnInfo {
        name: "notes_ref",
        sql_type: "TEXT",
        nullable: false,
        description: "Notes reference",
    },
    ColumnInfo {
        name: "target_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Annotated object SHA",
    },
    ColumnInfo {
        name: "note_id",
        sql_type: "TEXT",
        nullable: false,
        description: "Note blob SHA",
    },
    ColumnInfo {
        name: "content",
        sql_type: "TEXT",
        nullable: false,
        description: "Note text",
    },
    ColumnInfo {
        name: "repo",
        sql_type: "TEXT",
        nullable: false,
        description: "Repository path",
    },
];

// ============================================================================
// ALL TABLES
// ============================================================================

pub static TABLES: &[TableInfo] = &[
    // CORE
    TableInfo {
        name: "commits",
        description: "Commit history and metadata",
        category: "CORE",
        columns: COMMITS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS commits (id TEXT NOT NULL, short_id TEXT NOT NULL, tree_id TEXT NOT NULL, author_name TEXT NOT NULL, author_email TEXT NOT NULL, authored_at TEXT NOT NULL, committer_name TEXT NOT NULL, committer_email TEXT NOT NULL, committed_at TEXT NOT NULL, message TEXT NOT NULL, summary TEXT NOT NULL, body TEXT, parent_count INTEGER NOT NULL, is_merge INTEGER NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (id, repo))",
    },
    TableInfo {
        name: "commit_parents",
        description: "Parent-child relationships",
        category: "CORE",
        columns: COMMIT_PARENTS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS commit_parents (commit_id TEXT NOT NULL, parent_id TEXT NOT NULL, parent_index INTEGER NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (commit_id, parent_id, repo))",
    },
    // REFERENCES
    TableInfo {
        name: "branches",
        description: "Local and remote branches",
        category: "REFERENCES",
        columns: BRANCHES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS branches (name TEXT NOT NULL, full_name TEXT NOT NULL, target_id TEXT NOT NULL, is_remote INTEGER NOT NULL, is_head INTEGER NOT NULL, remote_name TEXT, upstream TEXT, ahead INTEGER, behind INTEGER, repo TEXT NOT NULL, PRIMARY KEY (full_name, repo))",
    },
    TableInfo {
        name: "tags",
        description: "Annotated and lightweight tags",
        category: "REFERENCES",
        columns: TAGS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS tags (name TEXT NOT NULL, full_name TEXT NOT NULL, target_id TEXT NOT NULL, target_type TEXT NOT NULL, is_annotated INTEGER NOT NULL, tagger_name TEXT, tagger_email TEXT, tagged_at TEXT, message TEXT, repo TEXT NOT NULL, PRIMARY KEY (full_name, repo))",
    },
    TableInfo {
        name: "refs",
        description: "All references (unified view)",
        category: "REFERENCES",
        columns: REFS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS refs (name TEXT NOT NULL, full_name TEXT NOT NULL, target_id TEXT NOT NULL, kind TEXT NOT NULL, is_symbolic INTEGER NOT NULL, symbolic_target TEXT, repo TEXT NOT NULL, PRIMARY KEY (full_name, repo))",
    },
    TableInfo {
        name: "stashes",
        description: "Stashed changes",
        category: "REFERENCES",
        columns: STASHES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS stashes (stash_index INTEGER NOT NULL, commit_id TEXT NOT NULL, message TEXT NOT NULL, author_name TEXT NOT NULL, author_email TEXT NOT NULL, created_at TEXT NOT NULL, branch TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (stash_index, repo))",
    },
    TableInfo {
        name: "reflog",
        description: "Reference history",
        category: "REFERENCES",
        columns: REFLOG_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS reflog (ref_name TEXT NOT NULL, entry_index INTEGER NOT NULL, old_id TEXT NOT NULL, new_id TEXT NOT NULL, committer_name TEXT NOT NULL, committer_email TEXT NOT NULL, committed_at TEXT NOT NULL, message TEXT NOT NULL, action TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (ref_name, entry_index, repo))",
    },
    // CHANGES
    TableInfo {
        name: "diffs",
        description: "Per-commit diff summary",
        category: "CHANGES",
        columns: DIFFS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS diffs (commit_id TEXT NOT NULL, parent_id TEXT, files_changed INTEGER NOT NULL, insertions INTEGER NOT NULL, deletions INTEGER NOT NULL, repo TEXT NOT NULL)",
    },
    TableInfo {
        name: "diff_files",
        description: "Per-file changes",
        category: "CHANGES",
        columns: DIFF_FILES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS diff_files (commit_id TEXT NOT NULL, parent_id TEXT, old_path TEXT, new_path TEXT, status TEXT NOT NULL, insertions INTEGER NOT NULL, deletions INTEGER NOT NULL, is_binary INTEGER NOT NULL, similarity INTEGER, repo TEXT NOT NULL)",
    },
    TableInfo {
        name: "blame",
        description: "Per-line attribution",
        category: "CHANGES",
        columns: BLAME_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS blame (path TEXT NOT NULL, line_number INTEGER NOT NULL, commit_id TEXT NOT NULL, original_line INTEGER NOT NULL, original_path TEXT NOT NULL, author_name TEXT NOT NULL, author_email TEXT NOT NULL, authored_at TEXT NOT NULL, line_content TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (path, line_number, repo))",
    },
    // CONFIGURATION
    TableInfo {
        name: "config",
        description: "Git configuration",
        category: "CONFIGURATION",
        columns: CONFIG_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS config (level TEXT NOT NULL, section TEXT NOT NULL, subsection TEXT, key TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL, repo TEXT NOT NULL)",
    },
    TableInfo {
        name: "remotes",
        description: "Remote repositories",
        category: "CONFIGURATION",
        columns: REMOTES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS remotes (name TEXT NOT NULL, url TEXT, push_url TEXT, fetch_refspec TEXT, push_refspec TEXT, repo TEXT NOT NULL, PRIMARY KEY (name, repo))",
    },
    TableInfo {
        name: "submodules",
        description: "Nested repositories",
        category: "CONFIGURATION",
        columns: SUBMODULES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS submodules (name TEXT NOT NULL, path TEXT NOT NULL, url TEXT NOT NULL, branch TEXT, head_id TEXT, status TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (name, repo))",
    },
    // WORKING DIRECTORY
    TableInfo {
        name: "status",
        description: "Working directory status",
        category: "WORKING DIRECTORY",
        columns: STATUS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS status (path TEXT NOT NULL, status_code TEXT NOT NULL, head_status TEXT NOT NULL, index_status TEXT NOT NULL, is_staged INTEGER NOT NULL, is_modified INTEGER NOT NULL, is_new INTEGER NOT NULL, is_deleted INTEGER NOT NULL, is_renamed INTEGER NOT NULL, is_copied INTEGER NOT NULL, is_ignored INTEGER NOT NULL, is_conflicted INTEGER NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (path, repo))",
    },
    TableInfo {
        name: "worktrees",
        description: "Linked working trees",
        category: "WORKING DIRECTORY",
        columns: WORKTREES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS worktrees (name TEXT NOT NULL, path TEXT, head_id TEXT, branch TEXT, is_bare INTEGER NOT NULL, is_detached INTEGER NOT NULL, is_locked INTEGER NOT NULL, lock_reason TEXT, is_prunable INTEGER NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (name, repo))",
    },
    // OPERATIONAL
    TableInfo {
        name: "hooks",
        description: "Installed git hooks",
        category: "OPERATIONAL",
        columns: HOOKS_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS hooks (name TEXT NOT NULL, path TEXT NOT NULL, is_executable INTEGER NOT NULL, is_sample INTEGER NOT NULL, size INTEGER NOT NULL, repo TEXT NOT NULL)",
    },
    TableInfo {
        name: "notes",
        description: "Git notes",
        category: "OPERATIONAL",
        columns: NOTES_COLUMNS,
        create_sql: "CREATE TABLE IF NOT EXISTS notes (notes_ref TEXT NOT NULL, target_id TEXT NOT NULL, note_id TEXT NOT NULL, content TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY (notes_ref, target_id, repo))",
    },
];

/// Finds a table by name.
///
/// Returns `None` if no table with the given name exists.
pub fn get_table_info(name: &str) -> Option<&'static TableInfo> {
    TABLES.iter().find(|t| t.name == name)
}

/// Groups tables by their category.
///
/// Returns a map from category name to list of tables in that category.
pub fn get_tables_by_category() -> HashMap<&'static str, Vec<&'static TableInfo>> {
    let mut map: HashMap<&'static str, Vec<&'static TableInfo>> = HashMap::new();
    for table in TABLES {
        map.entry(table.category).or_default().push(table);
    }
    map
}
