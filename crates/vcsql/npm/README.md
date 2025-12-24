# vcsql

SQL query engine for Git repository data.

## Installation

```bash
npm install -g vcsql
```

Or with npx:

```bash
npx vcsql "SELECT * FROM commits LIMIT 10"
```

## Usage

```bash
# Query commits
vcsql "SELECT author_name, summary FROM commits LIMIT 5"

# Query branches
vcsql "SELECT name, is_head FROM branches"

# Output as JSON
vcsql --format json "SELECT * FROM commits LIMIT 3"

# Show available tables
vcsql tables

# Show table schema
vcsql schema commits
```

## Available Tables

- `commits` - Commit history and metadata
- `branches` - Local and remote branches
- `tags` - Annotated and lightweight tags
- `diffs` - Per-commit diff summary
- `diff_files` - Per-file changes
- `blame` - Per-line file attribution
- `status` - Working directory status
- `stashes` - Stashed changes
- `reflog` - Reference history
- `config` - Git configuration
- `remotes` - Remote repositories
- `submodules` - Nested repositories
- `worktrees` - Linked working trees
- `hooks` - Installed git hooks
- `notes` - Git notes
- `refs` - All references
- `commit_parents` - Parent-child relationships

## Alternative Installation

If npm installation fails, you can install via cargo:

```bash
cargo install vcsql
```

Or download binaries from [GitHub Releases](https://github.com/douglance/vcsql/releases).

## License

MIT
