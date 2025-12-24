# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025-01-XX

### Added

- Initial release
- SQL query interface for Git repositories
- 17 queryable tables:
  - `commits` - commit history with author, message, timestamps
  - `branches` - local and remote branches
  - `tags` - annotated and lightweight tags
  - `refs` - all references (branches, tags, remotes)
  - `diffs` - commit diffs with stats
  - `diff_files` - per-file changes in commits
  - `blame` - line-by-line file attribution
  - `status` - working directory status
  - `stashes` - stashed changes
  - `reflog` - reference log history
  - `config` - git configuration values
  - `remotes` - configured remotes
  - `submodules` - submodule information
  - `worktrees` - linked worktrees
  - `hooks` - git hooks
  - `notes` - git notes
  - `commit_parents` - parent-child commit relationships
- Multi-repository queries with `--repos` flag
- Four output formats: table, JSON, JSONL, CSV
- Built-in examples with `vcsql examples` command
- Full SQL support: JOINs, CTEs, window functions, aggregations
