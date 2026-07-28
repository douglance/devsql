//! Source code providers for devsql
//!
//! Provides `source_files`, `source_lines`, and `symbols` tables by walking
//! the repository tree, reading files, and extracting symbols via regex.

use crate::Result;

#[cfg(feature = "tree-sitter-ast")]
pub mod ast_nodes;
#[cfg(feature = "tree-sitter-ast")]
pub mod imports;
pub mod shell_history;
pub mod source_files;
pub mod source_lines;
pub mod symbols;
#[cfg(feature = "tree-sitter-ast")]
pub mod tree_sitter;

use rusqlite::Connection;
use std::fs;
use std::io::Read;
use std::path::Path;

/// Metadata about a discovered source file.
pub struct FileInfo {
    pub path: String,
    pub name: String,
    pub extension: String,
    pub directory: String,
    pub size: u64,
    pub modified_at: String,
}

/// Efficient line/column lookup helpers for a source file.
pub struct LineIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut line_starts = Vec::with_capacity(text.lines().count() + 1);
        line_starts.push(0);
        for (idx, _) in text.match_indices('\n') {
            line_starts.push(idx + 1);
        }
        line_starts.push(text.len());
        Self { text, line_starts }
    }

    /// Convert a byte offset into a 1-based line number.
    pub fn line_for_byte(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(idx) => idx + 1,
            Err(idx) => idx,
        }
    }

    /// Convert a byte offset into a 1-based column number.
    pub fn column_for_byte(&self, byte: usize) -> usize {
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= byte)
            .saturating_sub(1);
        let line_start = *self.line_starts.get(line_idx).unwrap_or(&0);
        byte.saturating_sub(line_start) + 1
    }

    /// Line number for an exclusive end byte (inclusive end).
    pub fn inclusive_line_for_end(&self, end_byte: usize) -> usize {
        if end_byte == 0 {
            1
        } else {
            self.line_for_byte(end_byte.saturating_sub(1))
        }
    }

    pub fn line_text(&self, line: usize) -> &str {
        if line == 0 {
            return "";
        }
        let idx = line - 1;
        let start = *self.line_starts.get(idx).unwrap_or(&0);
        let end = *self.line_starts.get(idx + 1).unwrap_or(&self.text.len());
        self.text
            .get(start..end)
            .unwrap_or("")
            .trim_end_matches('\n')
    }

    pub fn slice(&self, start: usize, end: usize) -> &str {
        let s = start.min(self.text.len());
        let e = end.min(self.text.len());
        if s >= e {
            ""
        } else {
            &self.text[s..e]
        }
    }

    pub fn text(&self) -> &'a str {
        self.text
    }
}

/// Check if a file appears to be binary by reading the first 8 KB and looking
/// for null bytes.
pub fn is_binary(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return true; // can't read → treat as binary
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = file.read(&mut buf) else {
        return true;
    };
    buf[..n].contains(&0)
}

/// Map a file extension to a human-readable language name.
pub fn detect_language(ext: &str) -> &'static str {
    match ext {
        "rs" => "Rust",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "c" => "C",
        "cpp" | "cc" | "cxx" => "C++",
        "h" => "C Header",
        "hpp" | "hxx" => "C++ Header",
        "rb" => "Ruby",
        "swift" => "Swift",
        "kt" | "kts" => "Kotlin",
        "scala" => "Scala",
        "cs" => "C#",
        "php" => "PHP",
        "lua" => "Lua",
        "r" | "R" => "R",
        "sh" | "bash" | "zsh" => "Shell",
        "sql" => "SQL",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "md" | "markdown" => "Markdown",
        "zig" => "Zig",
        "nim" => "Nim",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "hs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "clj" | "cljs" => "Clojure",
        "dart" => "Dart",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "tf" | "hcl" => "Terraform",
        "proto" => "Protobuf",
        "graphql" | "gql" => "GraphQL",
        "dockerfile" => "Dockerfile",
        "makefile" => "Makefile",
        _ => "Other",
    }
}

/// Directories that should be skipped when walking source trees.
pub fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "vendor"
            | "__pycache__"
            | ".next"
            | "coverage"
            | ".cache"
            | ".idea"
            | ".vscode"
            | ".tox"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".eggs"
            | "venv"
            | ".venv"
            | "env"
    )
}

/// Walk a repository directory and collect metadata for all non-binary source
/// files. Uses the `ignore` crate to automatically respect .gitignore rules
/// and skip .git directories, with `should_skip_dir()` as an additional filter
/// for common non-gitignored directories (node_modules, target, etc.).
pub fn walk_source_files(repo_path: &Path) -> Vec<FileInfo> {
    let mut files = Vec::new();

    let walker = ignore::WalkBuilder::new(repo_path)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    return !should_skip_dir(name);
                }
            }
            true
        })
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        let path = entry.path();
        if is_binary(path) {
            continue;
        }

        let name = entry.file_name().to_str().unwrap_or("").to_string();

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        // Compute a relative path from repo_path
        let rel_path = path
            .strip_prefix(repo_path)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let directory = path
            .parent()
            .map(|p| {
                p.strip_prefix(repo_path)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_default();

        let metadata = entry.metadata().ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_at = metadata
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            })
            .unwrap_or_default();

        files.push(FileInfo {
            path: rel_path,
            name,
            extension,
            directory,
            size,
            modified_at,
        });
    }

    files
}

/// Load one or more code tables using the existing per-table providers.
/// This is a compatibility shim while the single-pass loader is under construction.
pub fn load_all_code_tables(conn: &Connection, repo_path: &Path, tables: &[&str]) -> Result<()> {
    for table in tables {
        match *table {
            "source_files" => source_files::load(conn, repo_path)?,
            "source_lines" => source_lines::load(conn, repo_path)?,
            "symbols" => symbols::load(conn, repo_path)?,
            #[cfg(feature = "tree-sitter-ast")]
            "imports" => imports::load(conn, repo_path)?,
            #[cfg(feature = "tree-sitter-ast")]
            "ast_nodes" => ast_nodes::load(conn, repo_path)?,
            _ => {}
        }
    }
    Ok(())
}
