#![cfg(feature = "tree-sitter-ast")]

use crate::Result;
use rusqlite::{params, Connection, Statement};
use std::path::Path;

use super::{detect_language, walk_source_files, LineIndex};
use super::tree_sitter::{import_query, TsLanguageKind, TsParser};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor, Tree};

const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS imports (
    file_path TEXT,
    line_number INTEGER,
    module TEXT,
    name TEXT,
    alias TEXT,
    kind TEXT,
    is_default INTEGER,
    is_wildcard INTEGER
)"#;

const INSERT_SQL: &str = r#"
INSERT INTO imports (
    file_path,
    line_number,
    module,
    name,
    alias,
    kind,
    is_default,
    is_wildcard
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
"#;

pub fn load(conn: &Connection, repo_path: &Path) -> Result<()> {
    ensure_table(conn)?;

    let files = walk_source_files(repo_path);
    conn.execute_batch("BEGIN")?;
    let mut stmt = conn.prepare(INSERT_SQL)?;
    let mut parser = TsParser::new();

    for file in files {
        if file.size > MAX_FILE_SIZE {
            continue;
        }

        let language = detect_language(&file.extension);
        let Some(lang_kind) = TsLanguageKind::from_name(language) else {
            continue;
        };

        // Currently only TypeScript/JavaScript imports are implemented.
        if !matches!(
            lang_kind,
            TsLanguageKind::TypeScript | TsLanguageKind::Tsx | TsLanguageKind::JavaScript | TsLanguageKind::Jsx
        ) {
            continue;
        }

        let abs_path = repo_path.join(&file.path);
        let Ok(contents) = std::fs::read_to_string(&abs_path) else {
            continue;
        };

        let line_index = LineIndex::new(&contents);
        let Some(tree) = parser.parse(language, &contents) else {
            continue;
        };

        let rows = extract_imports_from_tree(
            lang_kind,
            &tree,
            &line_index,
            &contents,
            &file.path,
        );

        for row in rows {
            insert_row(&mut stmt, &row)?;
        }
    }

    drop(stmt);
    conn.execute_batch("COMMIT")?;
    create_indexes(conn)?;
    Ok(())
}

fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(())
}

fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_imports_module ON imports(module);
         CREATE INDEX IF NOT EXISTS idx_imports_name ON imports(name);",
    )?;
    Ok(())
}

struct ImportRow {
    file_path: String,
    line_number: i64,
    module: String,
    name: Option<String>,
    alias: Option<String>,
    kind: String,
    is_default: bool,
    is_wildcard: bool,
}

fn insert_row(stmt: &mut Statement<'_>, row: &ImportRow) -> Result<()> {
    stmt.execute(params![
        row.file_path,
        row.line_number,
        row.module,
        row.name.as_deref(),
        row.alias.as_deref(),
        row.kind,
        if row.is_default { 1 } else { 0 },
        if row.is_wildcard { 1 } else { 0 },
    ])?;
    Ok(())
}

fn extract_imports_from_tree(
    language: TsLanguageKind,
    tree: &Tree,
    lines: &LineIndex<'_>,
    source: &str,
    file_path: &str,
) -> Vec<ImportRow> {
    let mut rows = Vec::new();

    match language {
        TsLanguageKind::TypeScript | TsLanguageKind::Tsx => {
            rows.extend(extract_ts_like_imports(
                language,
                tree,
                lines,
                source,
                file_path,
            ));
        }
        TsLanguageKind::JavaScript | TsLanguageKind::Jsx => {
            rows.extend(extract_ts_like_imports(
                language,
                tree,
                lines,
                source,
                file_path,
            ));
        }
        _ => {}
    }

    rows
}

fn extract_ts_like_imports(
    language: TsLanguageKind,
    tree: &Tree,
    lines: &LineIndex<'_>,
    source: &str,
    file_path: &str,
) -> Vec<ImportRow> {
    let Some(query) = import_query(language) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());

    while {
        matches.advance();
        matches.get().is_some()
    } {
        let matched = matches.get().unwrap();
        for capture in matched.captures {
            let node = capture.node;
            let capture_name = &query.capture_names()[capture.index as usize];
            match node.kind() {
                "import_statement" if capture_name.starts_with("import") => {
                    parse_ts_import_statement(node, source, lines, file_path, &mut rows);
                }
                "export_statement" if capture_name.starts_with("export") => {
                    parse_ts_export_statement(node, source, lines, file_path, &mut rows);
                }
                _ => {}
            }
        }
    }

    rows
}

fn parse_ts_import_statement(
    node: Node,
    source: &str,
    lines: &LineIndex<'_>,
    file_path: &str,
    rows: &mut Vec<ImportRow>,
) {
    let line_number = lines.line_for_byte(node.start_byte()) as i64;
    let module = node
        .child_by_field_name("source")
        .map(|n| string_literal_value(n, source))
        .unwrap_or_default();

    let mut entries = 0usize;
    for child in named_children(node) {
        if child.kind() == "import_clause" {
            entries += parse_ts_import_clause(
                child,
                source,
                file_path,
                line_number,
                &module,
                rows,
            );
        }
    }

    if entries == 0 {
        rows.push(ImportRow {
            file_path: file_path.to_string(),
            line_number,
            module: module.clone(),
            name: None,
            alias: None,
            kind: "import".to_string(),
            is_default: false,
            is_wildcard: false,
        });
    }
}

fn parse_ts_import_clause(
    clause: Node,
    source: &str,
    file_path: &str,
    line_number: i64,
    module: &str,
    rows: &mut Vec<ImportRow>,
) -> usize {
    let mut added = 0usize;

    for child in named_children(clause) {
        match child.kind() {
            "identifier" => {
                added += 1;
                rows.push(ImportRow {
                    file_path: file_path.to_string(),
                    line_number,
                    module: module.to_string(),
                    name: None,
                    alias: Some(node_text(child, source)),
                    kind: "import".to_string(),
                    is_default: true,
                    is_wildcard: false,
                });
            }
            "named_imports" => {
                for spec in named_children(child).into_iter().filter(|n| n.kind() == "import_specifier") {
                    let spec_name = spec
                        .child_by_field_name("name")
                        .map(|n| clean_identifier(node_text(n, source)));
                    if spec_name.is_none() {
                        continue;
                    }
                    let alias = spec
                        .child_by_field_name("alias")
                        .map(|n| clean_identifier(node_text(n, source)));

                    added += 1;
                    rows.push(ImportRow {
                        file_path: file_path.to_string(),
                        line_number,
                        module: module.to_string(),
                        name: spec_name,
                        alias,
                        kind: "import".to_string(),
                        is_default: false,
                        is_wildcard: false,
                    });
                }
            }
            "namespace_import" => {
                if let Some(alias_node) = named_children(child)
                    .into_iter()
                    .find(|n| n.kind() == "identifier")
                {
                    added += 1;
                    rows.push(ImportRow {
                        file_path: file_path.to_string(),
                        line_number,
                        module: module.to_string(),
                        name: None,
                        alias: Some(node_text(alias_node, source)),
                        kind: "import".to_string(),
                        is_default: false,
                        is_wildcard: true,
                    });
                }
            }
            _ => {}
        }
    }

    added
}

fn parse_ts_export_statement(
    node: Node,
    source: &str,
    lines: &LineIndex<'_>,
    file_path: &str,
    rows: &mut Vec<ImportRow>,
) {
    let Some(module_node) = node.child_by_field_name("source") else {
        return;
    };
    let module = string_literal_value(module_node, source);
    if module.is_empty() {
        return;
    }

    let line_number = lines.line_for_byte(node.start_byte()) as i64;
    let mut added = 0usize;

    for child in named_children(node) {
        match child.kind() {
            "export_clause" => {
                for spec in named_children(child).into_iter().filter(|n| n.kind() == "export_specifier") {
                    let export_name = spec
                        .child_by_field_name("name")
                        .map(|n| clean_identifier(node_text(n, source)));
                    if export_name.is_none() {
                        continue;
                    }
                    let alias = spec
                        .child_by_field_name("alias")
                        .map(|n| clean_identifier(node_text(n, source)));

                    added += 1;
                    rows.push(ImportRow {
                        file_path: file_path.to_string(),
                        line_number,
                        module: module.clone(),
                        name: export_name,
                        alias,
                        kind: "export".to_string(),
                        is_default: false,
                        is_wildcard: false,
                    });
                }
            }
            "namespace_export" => {
                if let Some(alias_node) = named_children(child)
                    .into_iter()
                    .find(|n| n.kind() == "identifier" || n.kind() == "string")
                {
                    added += 1;
                    rows.push(ImportRow {
                        file_path: file_path.to_string(),
                        line_number,
                        module: module.clone(),
                        name: Some("*".to_string()),
                        alias: Some(clean_identifier(node_text(alias_node, source))),
                        kind: "export".to_string(),
                        is_default: false,
                        is_wildcard: true,
                    });
                }
            }
            _ => {}
        }
    }

    if added == 0 {
        rows.push(ImportRow {
            file_path: file_path.to_string(),
            line_number,
            module: module.clone(),
            name: Some("*".to_string()),
            alias: None,
            kind: "export".to_string(),
            is_default: false,
            is_wildcard: true,
        });
    }
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let mut children = Vec::new();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() {
                children.push(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    children
}

fn string_literal_value(node: Node, source: &str) -> String {
    clean_identifier(node_text(node, source))
}

fn clean_identifier(text: String) -> String {
    let trimmed = text.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('`') && trimmed.ends_with('`'))
    {
        trimmed[1..trimmed.len().saturating_sub(1)].to_string()
    } else {
        trimmed.to_string()
    }
}

fn node_text(node: Node, source: &str) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    source[start..end].trim().to_string()
}
