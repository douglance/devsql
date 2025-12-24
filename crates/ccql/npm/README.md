# ccql

SQL query engine for Claude Code data.

## Installation

```bash
npm install -g ccql
```

## Usage

```bash
ccql "SELECT display FROM history LIMIT 5"
ccql "SELECT tool_name, COUNT(*) FROM transcripts WHERE type='tool_use' GROUP BY tool_name"
ccql tables
```

See [GitHub](https://github.com/douglance/ccql) for full documentation.
