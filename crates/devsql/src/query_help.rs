//! Answers the questions agents ask when they do not know the schema.
//!
//! Two shapes recur in query history. Agents ask for the schema the way they
//! would in another SQL shell (`SHOW TABLES`, `.schema commits`, `DESCRIBE
//! history`), and they guess column names from other tools (`commits.hash`,
//! `history.message`, `diff_files.file_path`). Both used to end in a bare
//! SQLite error. This module recognizes the first and explains the second.

use rusqlite::Connection;

/// One queryable table, as listed by `devsql schema`.
pub struct TableInfo {
    pub name: &'static str,
    pub source: &'static str,
    pub about: &'static str,
}

/// Every table a query can load. A name here must load through
/// `tools::load_query_tables`; the `catalog_tables_are_detected` test pins that
/// each one is at least recognized by table detection.
pub const TABLES: &[TableInfo] = &[
    TableInfo {
        name: "history",
        source: "Claude Code",
        about: "prompts you typed (display, timestamp, project)",
    },
    TableInfo {
        name: "sessions",
        source: "Claude Code",
        about: "one row per session with title, cwd, and token totals",
    },
    TableInfo {
        name: "transcripts",
        source: "Claude Code",
        about: "every conversation record (large; filter early)",
    },
    TableInfo {
        name: "tool_calls",
        source: "Claude Code",
        about: "tool invocations from transcripts",
    },
    TableInfo {
        name: "todos",
        source: "Claude Code",
        about: "todo lists",
    },
    TableInfo {
        name: "jhistory",
        source: "Codex CLI",
        about: "Codex prompts (alias: codex_history)",
    },
    TableInfo {
        name: "codex_threads",
        source: "Codex CLI",
        about: "one row per Codex conversation",
    },
    TableInfo {
        name: "codex_messages",
        source: "Codex CLI",
        about: "Codex conversation messages",
    },
    TableInfo {
        name: "codex_events",
        source: "Codex CLI",
        about: "raw Codex events",
    },
    TableInfo {
        name: "codex_tool_executions",
        source: "Codex CLI",
        about: "Codex commands with their output",
    },
    TableInfo {
        name: "codex_tool_calls",
        source: "Codex CLI",
        about: "Codex tool calls",
    },
    TableInfo {
        name: "codex_compactions",
        source: "Codex CLI",
        about: "Codex context compactions",
    },
    TableInfo {
        name: "codex_ingest_errors",
        source: "Codex CLI",
        about: "Codex records that failed to parse",
    },
    TableInfo {
        name: "commits",
        source: "Git",
        about: "commits in --repo (id, summary, message, authored_at)",
    },
    TableInfo {
        name: "diffs",
        source: "Git",
        about: "per-commit totals (files_changed, insertions, deletions)",
    },
    TableInfo {
        name: "diff_files",
        source: "Git",
        about: "per-file changes (commit_id, path, insertions, deletions)",
    },
    TableInfo {
        name: "branches",
        source: "Git",
        about: "branches in --repo",
    },
    TableInfo {
        name: "commit_parents",
        source: "Git",
        about: "commit to parent edges",
    },
    TableInfo {
        name: "tags",
        source: "Git",
        about: "tags and what they point at",
    },
    TableInfo {
        name: "refs",
        source: "Git",
        about: "every ref",
    },
    TableInfo {
        name: "stashes",
        source: "Git",
        about: "stash entries",
    },
    TableInfo {
        name: "reflog",
        source: "Git",
        about: "HEAD and branch reflog entries",
    },
    TableInfo {
        name: "blame",
        source: "Git",
        about: "line authorship for every tracked file (slow on large repos)",
    },
    TableInfo {
        name: "config",
        source: "Git",
        about: "Git config entries",
    },
    TableInfo {
        name: "remotes",
        source: "Git",
        about: "remotes and their URLs",
    },
    TableInfo {
        name: "submodules",
        source: "Git",
        about: "submodules",
    },
    TableInfo {
        name: "status",
        source: "Git",
        about: "working tree status per file",
    },
    TableInfo {
        name: "worktrees",
        source: "Git",
        about: "linked worktrees",
    },
    TableInfo {
        name: "hooks",
        source: "Git",
        about: "installed hooks",
    },
    TableInfo {
        name: "notes",
        source: "Git",
        about: "Git notes",
    },
    TableInfo {
        name: "source_files",
        source: "Code",
        about: "files in --repo",
    },
    TableInfo {
        name: "source_lines",
        source: "Code",
        about: "lines of files in --repo",
    },
    TableInfo {
        name: "symbols",
        source: "Code",
        about: "functions, types, and other symbols in --repo",
    },
    TableInfo {
        name: "shell_history",
        source: "Shell",
        about: "Atuin, zsh, and bash history",
    },
    TableInfo {
        name: "command_events",
        source: "Shell",
        about: "shell and agent commands in one stream",
    },
    TableInfo {
        name: "work_tasks",
        source: "Worklog",
        about: "tasks from `devsql work`",
    },
    TableInfo {
        name: "work_events",
        source: "Worklog",
        about: "start, update, done, and note events",
    },
    TableInfo {
        name: "grok_bots",
        source: "Grok Bots",
        about: "Grok bots",
    },
    TableInfo {
        name: "grok_entries",
        source: "Grok Bots",
        about: "Grok bot entries",
    },
    TableInfo {
        name: "grok_messages",
        source: "Grok Bots",
        about: "Grok bot messages",
    },
    TableInfo {
        name: "grok_ingest_errors",
        source: "Grok Bots",
        about: "Grok records that failed to parse",
    },
    TableInfo {
        name: "macos_logs",
        source: "macOS",
        about: "Unified Log records (bounded by --log-* options)",
    },
];

/// A schema question phrased as a query.
#[derive(Debug, PartialEq, Eq)]
pub enum SchemaRequest {
    ListTables,
    Describe(String),
}

/// Recognize schema questions in the forms agents have typed: `tables`,
/// `schema`, `SHOW TABLES`, `.tables`, `.schema t`, `DESCRIBE t`, `\d t`,
/// and `information_schema.tables`.
pub fn schema_request(input: &str) -> Option<SchemaRequest> {
    let text = input.trim().trim_end_matches(';').trim();
    let lower = text.to_ascii_lowercase();
    if lower.contains("information_schema.tables") {
        return Some(SchemaRequest::ListTables);
    }
    let words: Vec<&str> = lower.split_whitespace().collect();
    let table = |t: &str| {
        Some(SchemaRequest::Describe(
            t.trim_matches(['`', '"', '\'']).to_string(),
        ))
    };
    match words.as_slice() {
        ["tables"] | [".tables"] | ["schema"] | [".schema"] | ["show", "tables"] | ["\\dt"] => {
            Some(SchemaRequest::ListTables)
        }
        [".schema", t] | ["schema", t] | ["describe", t] | ["desc", t] | ["\\d", t] => table(t),
        ["show", "columns", "from", t] => table(t),
        _ => None,
    }
}

/// Column names of a loaded table, in declaration order. Empty when the table
/// is not loaded.
pub fn table_columns(conn: &Connection, table: &str) -> Vec<(String, String)> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "PRAGMA table_info(\"{}\")",
        table.replace('"', "")
    )) else {
        return Vec::new();
    };
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,
            row.get::<_, String>(2).unwrap_or_default(),
        ))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Names people reach for that mean the same thing. A guess in one group
/// suggests any real column in the same group.
const SYNONYMS: &[&[&str]] = &[
    &[
        "id",
        "short_id",
        "hash",
        "sha",
        "commit_hash",
        "commit_sha",
        "oid",
        "commit",
        "commit_id",
    ],
    &["summary", "subject", "title"],
    &[
        "display",
        "message",
        "prompt",
        "text",
        "content",
        "body",
        "first_user_text",
    ],
    &[
        "project",
        "cwd",
        "repo",
        "repo_path",
        "directory",
        "dir",
        "workdir",
    ],
    &["path", "file_path", "file", "filename", "file_name"],
    &[
        "insertions",
        "additions",
        "added",
        "lines_added",
        "added_lines",
    ],
    &[
        "deletions",
        "removed",
        "lines_removed",
        "deleted_lines",
        "deleted",
    ],
    &[
        "first_timestamp",
        "started_at",
        "created_at",
        "start_time",
        "first_event_at",
    ],
    &[
        "last_timestamp",
        "last_event_at",
        "ended_at",
        "updated_at",
        "end_time",
        "finished_at",
    ],
    &[
        "timestamp",
        "ts",
        "time",
        "date",
        "authored_at",
        "committed_at",
    ],
    &["author_name", "author"],
    &["thread_id", "session_id", "conversation_id"],
];

const TABLE_SYNONYMS: &[(&str, &str)] = &[
    ("work", "work_tasks"),
    ("tasks", "work_tasks"),
    ("worklog", "work_events"),
    ("prompts", "history"),
    ("messages", "codex_messages"),
    ("conversations", "sessions"),
    ("threads", "codex_threads"),
    ("commands", "command_events"),
    ("shell", "shell_history"),
    ("files", "diff_files"),
    ("logs", "macos_logs"),
];

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn resembles(guess: &str, real: &str) -> bool {
    if SYNONYMS
        .iter()
        .any(|group| group.contains(&guess) && group.contains(&real))
    {
        return true;
    }
    let close = guess.len() > 3 && edit_distance(guess, real) <= 2;
    let nested =
        guess.len() >= 4 && real.len() >= 4 && (real.contains(guess) || guess.contains(real));
    close || nested
}

fn mentioned_tables(sql: &str) -> Vec<&'static str> {
    let lower = sql.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    let mut tables: Vec<&'static str> = TABLES
        .iter()
        .map(|t| t.name)
        .filter(|name| words.contains(name))
        .collect();
    if words.contains(&"codex_history") && !tables.contains(&"jhistory") {
        tables.push("jhistory");
    }
    tables
}

fn captured<'a>(message: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &message[message.find(marker)? + marker.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|name| !name.is_empty())
}

/// Explain a failed query in terms of the real schema: the columns of each
/// table the query names, and the closest real name to what was guessed.
/// Returns `None` when the error is not about a name.
pub fn explain_error(conn: &Connection, sql: &str, message: &str) -> Option<String> {
    if let Some(name) = captured(message, "no such column: ") {
        let guess = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
        let mut suggestions = Vec::new();
        let mut listings = Vec::new();
        for table in mentioned_tables(sql) {
            let columns = table_columns(conn, table);
            if columns.is_empty() {
                continue;
            }
            for (column, _) in &columns {
                if resembles(&guess, column) {
                    suggestions.push(format!("{table}.{column}"));
                }
            }
            let names: Vec<&str> = columns.iter().map(|(c, _)| c.as_str()).collect();
            listings.push(format!("{table}({})", names.join(", ")));
        }
        if listings.is_empty() {
            return None;
        }
        let mut text = format!("No column named `{name}`.");
        if !suggestions.is_empty() {
            text.push_str(&format!(" Did you mean {}?", suggestions.join(" or ")));
        } else {
            let mentioned = mentioned_tables(sql);
            let elsewhere: Vec<&str> = TABLES
                .iter()
                .filter(|t| !mentioned.contains(&t.name) && guess.len() >= 4)
                .filter(|t| {
                    t.about
                        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .any(|w| w == guess)
                })
                .map(|t| t.name)
                .collect();
            if !elsewhere.is_empty() {
                text.push_str(&format!(" `{guess}` is in {}.", elsewhere.join(", ")));
            }
        }
        text.push_str(&format!(" Columns: {}.", listings.join("; ")));
        return Some(text);
    }
    if let Some(name) = captured(message, "no such table: ") {
        let guess = name.to_ascii_lowercase();
        let mut suggestions: Vec<&str> = TABLE_SYNONYMS
            .iter()
            .filter(|(alias, _)| *alias == guess)
            .map(|(_, table)| *table)
            .collect();
        for table in TABLES.iter().map(|t| t.name) {
            if !suggestions.contains(&table) && resembles(&guess, table) {
                suggestions.push(table);
            }
        }
        let mut text = format!("No table named `{name}`.");
        if !suggestions.is_empty() {
            text.push_str(&format!(" Did you mean {}?", suggestions.join(" or ")));
        }
        let names: Vec<&str> = TABLES.iter().map(|t| t.name).collect();
        text.push_str(&format!(" Tables: {}.", names.join(", ")));
        return Some(text);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with(ddl: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ddl).unwrap();
        conn
    }

    #[test]
    fn recognizes_schema_questions_from_other_sql_shells() {
        for text in [
            "tables",
            "SHOW TABLES;",
            ".tables",
            "schema",
            "\\dt",
            "SELECT table_name FROM information_schema.tables",
        ] {
            assert_eq!(
                schema_request(text),
                Some(SchemaRequest::ListTables),
                "{text}"
            );
        }
        for text in [
            ".schema commits",
            "DESCRIBE commits",
            "desc `commits`;",
            "schema commits",
            "SHOW COLUMNS FROM commits",
        ] {
            assert_eq!(
                schema_request(text),
                Some(SchemaRequest::Describe("commits".into())),
                "{text}"
            );
        }
        assert_eq!(schema_request("SELECT * FROM commits"), None);
        assert_eq!(schema_request("SELECT tables FROM x"), None);
    }

    #[test]
    fn suggests_the_real_column_for_a_guessed_one() {
        let conn = conn_with("CREATE TABLE commits (id TEXT, short_id TEXT, summary TEXT, message TEXT, authored_at TEXT);");
        let text = explain_error(
            &conn,
            "SELECT hash FROM commits",
            "SQL error: no such column: hash in SELECT",
        )
        .unwrap();
        assert!(
            text.contains("Did you mean commits.id or commits.short_id?"),
            "{text}"
        );
        assert!(
            text.contains("commits(id, short_id, summary, message, authored_at)"),
            "{text}"
        );

        let conn = conn_with(
            "CREATE TABLE history (rowid INTEGER, display TEXT, timestamp TEXT, project TEXT);",
        );
        let text = explain_error(&conn, "SELECT cwd FROM history", "no such column: cwd").unwrap();
        assert!(text.contains("Did you mean history.project?"), "{text}");
        let text = explain_error(
            &conn,
            "SELECT h.message FROM history h",
            "no such column: h.message",
        )
        .unwrap();
        assert!(text.contains("history.display"), "{text}");
    }

    #[test]
    fn names_the_table_that_has_a_missing_column() {
        let conn = conn_with("CREATE TABLE commits (id TEXT, summary TEXT);");
        let text = explain_error(
            &conn,
            "SELECT SUM(insertions) FROM commits",
            "no such column: insertions",
        )
        .unwrap();
        assert!(
            text.contains("`insertions` is in diffs, diff_files."),
            "{text}"
        );
    }

    #[test]
    fn lists_columns_even_without_a_close_match() {
        let conn =
            conn_with("CREATE TABLE diff_files (commit_id TEXT, path TEXT, insertions INTEGER);");
        let text =
            explain_error(&conn, "SELECT zzz FROM diff_files", "no such column: zzz").unwrap();
        assert!(!text.contains("Did you mean"), "{text}");
        assert!(
            text.contains("diff_files(commit_id, path, insertions)"),
            "{text}"
        );
    }

    #[test]
    fn suggests_the_real_table_for_a_guessed_one() {
        let conn = conn_with("");
        let text = explain_error(&conn, "SELECT * FROM work", "no such table: work").unwrap();
        assert!(text.contains("Did you mean work_tasks"), "{text}");
        let text = explain_error(&conn, "SELECT * FROM comits", "no such table: comits").unwrap();
        assert!(text.contains("Did you mean commits"), "{text}");
    }

    #[test]
    fn leaves_other_errors_alone() {
        let conn = conn_with("");
        assert_eq!(
            explain_error(&conn, "SELEC 1", "near \"SELEC\": syntax error"),
            None
        );
    }

    #[test]
    fn catalog_tables_are_detected() {
        for table in TABLES {
            let needed = crate::engine::detect_tables(&format!("SELECT * FROM {}", table.name));
            let found = [
                needed.claude,
                needed.git,
                needed.code,
                needed.shell,
                needed.work,
                needed.system,
                needed.grok,
            ]
            .concat();
            assert!(
                found.iter().any(|t| t == table.name),
                "{} is listed but never loaded",
                table.name
            );
        }
    }
}
