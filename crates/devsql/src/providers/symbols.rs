//! Provider for the `symbols` table.

use crate::Result;
use regex::Regex;
use rusqlite::{params, Connection, Statement};
use std::path::Path;
use std::sync::LazyLock;

use super::{detect_language, walk_source_files};
#[cfg(feature = "tree-sitter-ast")]
use super::LineIndex;

#[cfg(feature = "tree-sitter-ast")]
use super::tree_sitter::{symbol_query, TsLanguageKind, TsParser};
#[cfg(feature = "tree-sitter-ast")]
use std::collections::HashMap;
#[cfg(feature = "tree-sitter-ast")]
use streaming_iterator::StreamingIterator;
#[cfg(feature = "tree-sitter-ast")]
use tree_sitter::{Node, QueryCursor, Tree};

/// Maximum file size (in bytes) to scan for symbols.
const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

const CREATE_SYMBOLS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS symbols (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT,
    name TEXT,
    kind TEXT,
    line_start INTEGER,
    line_end INTEGER,
    signature TEXT,
    visibility TEXT,
    parent_id INTEGER,
    parameters TEXT,
    return_type TEXT,
    language TEXT
)"#;

const INSERT_SYMBOL_SQL: &str = r#"
INSERT INTO symbols (
    id,
    file_path,
    name,
    kind,
    line_start,
    line_end,
    signature,
    visibility,
    parent_id,
    parameters,
    return_type,
    language
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
"#;

struct SymbolRow {
    id: i64,
    file_path: String,
    name: String,
    kind: String,
    line_start: i64,
    line_end: i64,
    signature: String,
    visibility: String,
    parent_id: Option<i64>,
    parameters: String,
    return_type: String,
    language: String,
}

pub fn load(conn: &Connection, repo_path: &Path) -> Result<()> {
    #[cfg(feature = "tree-sitter-ast")]
    {
        load_with_tree_sitter(conn, repo_path)
    }
    #[cfg(not(feature = "tree-sitter-ast"))]
    {
        load_with_regex(conn, repo_path)
    }
}

#[cfg(feature = "tree-sitter-ast")]
fn load_with_tree_sitter(conn: &Connection, repo_path: &Path) -> Result<()> {
    ensure_table(conn)?;
    let files = walk_source_files(repo_path);

    conn.execute_batch("BEGIN")?;
    let mut insert_stmt = conn.prepare(INSERT_SYMBOL_SQL)?;
    let mut parser = TsParser::new();
    let mut next_id: i64 = 1;

    for file_info in &files {
        if file_info.size > MAX_FILE_SIZE {
            continue;
        }
        let language = detect_language(&file_info.extension);
        let abs_path = repo_path.join(&file_info.path);
        let Ok(content) = std::fs::read_to_string(&abs_path) else {
            continue;
        };
        let line_index = LineIndex::new(&content);
        let mut rows = Vec::new();

        if let Some(lang_kind) = TsLanguageKind::from_name(language) {
            if let Some(tree) = parser.parse(language, &content) {
                rows = extract_symbols_from_tree(
                    lang_kind,
                    &tree,
                    &line_index,
                    &content,
                    &file_info.path,
                    language,
                    &mut next_id,
                );
            }
        }

        if rows.is_empty() {
            rows = extract_symbols_with_regex(language, &content, &file_info.path, language, &mut next_id);
        }

        for row in rows {
            insert_symbol(&mut insert_stmt, &row)?;
        }
    }

    drop(insert_stmt);
    conn.execute_batch("COMMIT")?;
    create_indexes(conn)?;
    Ok(())
}

#[cfg(not(feature = "tree-sitter-ast"))]
fn load_with_regex(conn: &Connection, repo_path: &Path) -> Result<()> {
    ensure_table(conn)?;
    let files = walk_source_files(repo_path);

    conn.execute_batch("BEGIN")?;
    let mut insert_stmt = conn.prepare(INSERT_SYMBOL_SQL)?;
    let mut next_id: i64 = 1;

    for file_info in &files {
        if file_info.size > MAX_FILE_SIZE {
            continue;
        }
        let language = detect_language(&file_info.extension);
        let abs_path = repo_path.join(&file_info.path);
        let Ok(content) = std::fs::read_to_string(&abs_path) else {
            continue;
        };
        let rows = extract_symbols_with_regex(language, &content, &file_info.path, language, &mut next_id);
        for row in rows {
            insert_symbol(&mut insert_stmt, &row)?;
        }
    }

    drop(insert_stmt);
    conn.execute_batch("COMMIT")?;
    create_indexes(conn)?;
    Ok(())
}

fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_SYMBOLS_TABLE)?;
    Ok(())
}

fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
         CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
         CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);",
    )?;
    Ok(())
}

fn insert_symbol(stmt: &mut Statement<'_>, row: &SymbolRow) -> Result<()> {
    stmt.execute(params![
        row.id,
        row.file_path,
        row.name,
        row.kind,
        row.line_start,
        row.line_end,
        row.signature,
        row.visibility,
        row.parent_id,
        row.parameters,
        row.return_type,
        row.language,
    ])?;
    Ok(())
}

#[cfg(feature = "tree-sitter-ast")]
fn extract_symbols_from_tree(
    language: TsLanguageKind,
    tree: &Tree,
    lines: &LineIndex<'_>,
    source: &str,
    file_path: &str,
    language_label: &str,
    next_id: &mut i64,
) -> Vec<SymbolRow> {
    let mut rows = Vec::new();
    let mut node_to_id: HashMap<usize, i64> = HashMap::new();
    let Some(query) = symbol_query(language) else {
        return rows;
    };

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while {
        matches.advance();
        matches.get().is_some()
    } {
        let m = matches.get().unwrap();
        for capture in m.captures {
            let capture_name = &query.capture_names()[capture.index as usize];
            if !capture_name.ends_with(".definition") {
                continue;
            }
            let mut kind_key = capture_name.split('.').next().unwrap_or("").to_string();
            let mut node = capture.node;
            if language == TsLanguageKind::Python && kind_key == "decorated" {
                if let Some(inner) = decorated_target(node) {
                    kind_key = inner.kind().to_string();
                    node = inner;
                }
            }
            let name = extract_name(node, source);
            if name.is_empty() {
                continue;
            }
            let kind = normalize_kind(language, &kind_key, node);
            let line_start = (node.start_position().row + 1) as i64;
            let line_end = lines.inclusive_line_for_end(node.end_byte()) as i64;
            let signature = signature_text(node, source);
            let visibility = visibility_text(language, node, source);
            let parameters = parameters_text(language, node, source);
            let return_type = return_type_text(language, node, source);
            let parent_id = parent_symbol_id(node, &node_to_id);
            let id = *next_id;
            *next_id += 1;

            node_to_id.insert(node.id(), id);
            rows.push(SymbolRow {
                id,
                file_path: file_path.to_string(),
                name,
                kind,
                line_start,
                line_end,
                signature,
                visibility,
                parent_id,
                parameters,
                return_type,
                language: language_label.to_string(),
            });
        }
    }

    rows
}

#[cfg(feature = "tree-sitter-ast")]
fn extract_name(node: Node, source: &str) -> String {
    if let Some(name_node) = node.child_by_field_name("name") {
        return node_text(name_node, source);
    }
    if node.kind() == "impl_item" {
        if let Some(target) = node.child_by_field_name("type") {
            return node_text(target, source);
        }
    }
    String::new()
}

#[cfg(feature = "tree-sitter-ast")]
fn decorated_target(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    let mut iter = node.named_children(&mut cursor);
    iter.find(|child| child.kind() == "function_definition" || child.kind() == "class_definition")
}

#[cfg(feature = "tree-sitter-ast")]
fn normalize_kind(language: TsLanguageKind, key: &str, node: Node) -> String {
    match (language, key) {
        (TsLanguageKind::Rust, "function") => {
            if let Some(parent) = node.parent() {
                if matches!(parent.kind(), "impl_item" | "trait_item") {
                    return "method".to_string();
                }
            }
            "function".to_string()
        }
        (TsLanguageKind::Rust, "impl") => "impl".to_string(),
        (TsLanguageKind::Rust, other) => other.to_string(),
        (TsLanguageKind::Python, "decorated") => {
            if let Some(target) = decorated_target(node) {
                return normalize_kind(language, target.kind(), target);
            }
            "decorated".to_string()
        }
        (_, "function") => "function".to_string(),
        (_, "method") => "method".to_string(),
        (_, "class") => "class".to_string(),
        (_, "interface") => "interface".to_string(),
        (_, "type_alias") => "type_alias".to_string(),
        (_, "enum") => "enum".to_string(),
        (_, "trait") => "trait".to_string(),
        (_, "module") => "module".to_string(),
        (_, other) => other.to_string(),
    }
}

#[cfg(feature = "tree-sitter-ast")]
fn signature_text(node: Node, source: &str) -> String {
    if let Some(body) = node.child_by_field_name("body") {
        let start = node.start_byte();
        let end = body.start_byte();
        return source[start..end].trim().to_string();
    }
    node_text(node, source)
}

#[cfg(feature = "tree-sitter-ast")]
fn visibility_text(language: TsLanguageKind, node: Node, source: &str) -> String {
    if let Some(vis) = node.child_by_field_name("visibility") {
        return node_text(vis, source);
    }
    match language {
        TsLanguageKind::Go => {
            if let Some(name) = node.child_by_field_name("name") {
                let text = node_text(name, source);
                if text.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    "export".to_string()
                } else {
                    "private".to_string()
                }
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

#[cfg(feature = "tree-sitter-ast")]
fn parameters_text(language: TsLanguageKind, node: Node, source: &str) -> String {
    let field_candidates = match language {
        TsLanguageKind::Go => &["parameters", "signature"][..],
        _ => &["parameters"][..],
    };
    for field in field_candidates {
        if let Some(p) = node.child_by_field_name(field) {
            return node_text(p, source);
        }
    }
    // TypeScript/JavaScript use `formal_parameters`.
    if let Some(formal) = node
        .named_children(&mut node.walk())
        .find(|child| child.kind().ends_with("parameters"))
    {
        return node_text(formal, source);
    }
    String::new()
}

#[cfg(feature = "tree-sitter-ast")]
fn return_type_text(language: TsLanguageKind, node: Node, source: &str) -> String {
    match language {
        TsLanguageKind::Rust | TsLanguageKind::Python => {
            if let Some(ret) = node.child_by_field_name("return_type") {
                return node_text(ret, source);
            }
            String::new()
        }
        TsLanguageKind::TypeScript | TsLanguageKind::Tsx | TsLanguageKind::JavaScript | TsLanguageKind::Jsx => {
            if let Some(ret) = node.child_by_field_name("return_type") {
                return node_text(ret, source);
            }
            if let Some(annotation) = node.child_by_field_name("type") {
                return node_text(annotation, source);
            }
            String::new()
        }
        TsLanguageKind::Go => {
            if let Some(result) = node.child_by_field_name("result") {
                return node_text(result, source);
            }
            String::new()
        }
    }
}

#[cfg(feature = "tree-sitter-ast")]
fn parent_symbol_id(node: Node, map: &HashMap<usize, i64>) -> Option<i64> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if let Some(id) = map.get(&parent.id()) {
            return Some(*id);
        }
        current = parent.parent();
    }
    None
}

#[cfg(feature = "tree-sitter-ast")]
fn node_text(node: Node, source: &str) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    source[start..end].trim().to_string()
}

fn extract_symbols_with_regex(
    language: &str,
    content: &str,
    file_path: &str,
    label: &str,
    next_id: &mut i64,
) -> Vec<SymbolRow> {
    let matches = extract_symbols(language, content);
    matches
        .into_iter()
        .map(|m| {
            let id = *next_id;
            *next_id += 1;
            SymbolRow {
                id,
                file_path: file_path.to_string(),
                name: m.name,
                kind: m.kind,
                line_start: m.line_start as i64,
                line_end: m.line_end as i64,
                signature: m.signature,
                visibility: m.visibility,
                parent_id: None,
                parameters: String::new(),
                return_type: String::new(),
                language: label.to_string(),
            }
        })
        .collect()
}

pub struct SymbolMatch {
    pub name: String,
    pub kind: String,
    pub line_start: usize,
    pub line_end: usize,
    pub signature: String,
    pub visibility: String,
}

pub fn extract_symbols(language: &str, content: &str) -> Vec<SymbolMatch> {
    match language {
        "Rust" => extract_rust(content),
        "TypeScript" | "TSX" | "JavaScript" | "JSX" => extract_ts_js(content),
        "Python" => extract_python(content),
        "Go" => extract_go(content),
        _ => Vec::new(),
    }
}

fn line_number_at(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].matches('\n').count() + 1
}

fn line_text(content: &str, line_num: usize) -> &str {
    content.lines().nth(line_num.saturating_sub(1)).unwrap_or("")
}

// --- Regex fallback extractors (unchanged logic) ---

static RE_RUST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[[:space:]]*(pub(\(crate\))?\s+)?(async\s+)?(fn|struct|enum|trait|type|const|static|impl|mod|macro_rules!)\s+(\w+)",
    )
    .expect("Rust regex")
});

fn extract_rust(content: &str) -> Vec<SymbolMatch> {
    let mut rows = Vec::new();
    for caps in RE_RUST.captures_iter(content) {
        let full_match = caps.get(0).unwrap();
        let line_start = line_number_at(content, full_match.start());
        let signature = line_text(content, line_start).trim().to_string();

        let visibility = if caps.get(1).is_some() {
            if caps.get(2).is_some() {
                "pub(crate)".to_string()
            } else {
                "pub".to_string()
            }
        } else {
            "private".to_string()
        };

        let kind = caps.get(4).map(|m| m.as_str()).unwrap_or("unknown");
        if kind == "impl" {
            continue;
        }
        let kind_str = if kind == "macro_rules!" { "macro" } else { kind };
        let name = caps.get(5).map(|m| m.as_str()).unwrap_or("");

        rows.push(SymbolMatch {
            name: name.to_string(),
            kind: kind_str.to_string(),
            line_start,
            line_end: line_start,
            signature,
            visibility: visibility.to_string(),
        });
    }
    rows
}

static RE_TS_JS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[[:space:]]*(export\s+)?(default\s+)?(async\s+)?(function|class|interface|type|enum)\s+(\w+)",
    )
    .expect("TS/JS regex")
});

fn extract_ts_js(content: &str) -> Vec<SymbolMatch> {
    let mut rows = Vec::new();
    for caps in RE_TS_JS.captures_iter(content) {
        let full_match = caps.get(0).unwrap();
        let line_start = line_number_at(content, full_match.start());
        let signature = line_text(content, line_start).trim().to_string();
        let visibility = if caps.get(1).is_some() {
            "export".to_string()
        } else {
            String::new()
        };
        let kind = caps.get(4).map(|m| m.as_str()).unwrap_or("unknown");
        let name = caps.get(5).map(|m| m.as_str()).unwrap_or("");
        rows.push(SymbolMatch {
            name: name.to_string(),
            kind: kind.to_string(),
            line_start,
            line_end: line_start,
            signature,
            visibility,
        });
    }
    rows
}

static RE_PYTHON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[[:space:]]*(class|def)\s+(\w+)").expect("Python regex"));

fn extract_python(content: &str) -> Vec<SymbolMatch> {
    let mut rows = Vec::new();
    for caps in RE_PYTHON.captures_iter(content) {
        let full_match = caps.get(0).unwrap();
        let line_start = line_number_at(content, full_match.start());
        let signature = line_text(content, line_start).trim().to_string();
        let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("unknown");
        let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        rows.push(SymbolMatch {
            name: name.to_string(),
            kind: kind.to_string(),
            line_start,
            line_end: line_start,
            signature,
            visibility: String::new(),
        });
    }
    rows
}

static RE_GO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^func\s+(\([^)]*\)\s+)?(\w+)").expect("Go regex"));

fn extract_go(content: &str) -> Vec<SymbolMatch> {
    let mut rows = Vec::new();
    for caps in RE_GO.captures_iter(content) {
        let full_match = caps.get(0).unwrap();
        let line_start = line_number_at(content, full_match.start());
        let signature = line_text(content, line_start).trim().to_string();
        let name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let visibility = if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            "export"
        } else {
            ""
        };
        rows.push(SymbolMatch {
            name: name.to_string(),
            kind: "function".to_string(),
            line_start,
            line_end: line_start,
            signature,
            visibility: visibility.to_string(),
        });
    }
    rows
}
