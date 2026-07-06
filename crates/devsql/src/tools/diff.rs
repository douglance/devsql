//! `devsql diff` -- compare two Git refs and return file-level and symbol-level stats.

use git2::Delta;
use incurs::command::{CommandContext, CommandDef, CommandHandler, Example};
use incurs::output::CommandResult;
use serde_json::json;
use std::path::PathBuf;

use super::semantic_diff::{diff_file_symbols, read_file_at_commit, ChangeType, SymbolChange};
use crate::providers::detect_language;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(incurs::Args, serde::Deserialize)]
#[allow(dead_code)]
struct DiffArgs {
    /// Base ref (commit, branch, tag)
    base: String,
    /// Head ref (commit, branch, tag)
    head: String,
}

#[derive(incurs::Options, serde::Deserialize)]
#[allow(dead_code)]
struct DiffOptions {
    /// Git repository path
    #[incur(alias = "r", default = ".")]
    repo: String,
    /// Maximum number of files to return
    #[incur(alias = "n", default = "100")]
    limit: i64,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

struct DiffHandler;

#[async_trait::async_trait]
impl CommandHandler for DiffHandler {
    async fn run(&self, ctx: CommandContext) -> CommandResult {
        let base_ref = match ctx.args.get("base").and_then(|v| v.as_str()) {
            Some(r) => r.to_string(),
            None => {
                return CommandResult::Error {
                    code: "MISSING_ARG".into(),
                    message: "Missing required argument: base".into(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let head_ref = match ctx.args.get("head").and_then(|v| v.as_str()) {
            Some(r) => r.to_string(),
            None => {
                return CommandResult::Error {
                    code: "MISSING_ARG".into(),
                    message: "Missing required argument: head".into(),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let repo_str = ctx
            .options
            .get("repo")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let limit = ctx
            .options
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100) as usize;

        let repo_path = if repo_str == "." {
            match std::env::current_dir() {
                Ok(p) => p,
                Err(e) => {
                    return CommandResult::Error {
                        code: "PATH_ERROR".into(),
                        message: format!("Cannot determine current directory: {e}"),
                        retryable: false,
                        exit_code: Some(1),
                        cta: None,
                    };
                }
            }
        } else {
            PathBuf::from(repo_str)
        };

        let repo = match git2::Repository::open(&repo_path) {
            Ok(r) => r,
            Err(e) => {
                return CommandResult::Error {
                    code: "GIT_ERROR".into(),
                    message: format!("Failed to open repository: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        // Resolve refs to commits
        let base_commit = match repo
            .revparse_single(&base_ref)
            .and_then(|obj| obj.peel_to_commit())
        {
            Ok(c) => c,
            Err(e) => {
                return CommandResult::Error {
                    code: "REF_ERROR".into(),
                    message: format!("Cannot resolve base ref '{base_ref}': {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let head_commit = match repo
            .revparse_single(&head_ref)
            .and_then(|obj| obj.peel_to_commit())
        {
            Ok(c) => c,
            Err(e) => {
                return CommandResult::Error {
                    code: "REF_ERROR".into(),
                    message: format!("Cannot resolve head ref '{head_ref}': {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let base_tree = match base_commit.tree() {
            Ok(t) => t,
            Err(e) => {
                return CommandResult::Error {
                    code: "GIT_ERROR".into(),
                    message: format!("Cannot get base tree: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let head_tree = match head_commit.tree() {
            Ok(t) => t,
            Err(e) => {
                return CommandResult::Error {
                    code: "GIT_ERROR".into(),
                    message: format!("Cannot get head tree: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        let diff = match repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None) {
            Ok(d) => d,
            Err(e) => {
                return CommandResult::Error {
                    code: "GIT_ERROR".into(),
                    message: format!("Failed to compute diff: {e}"),
                    retryable: false,
                    exit_code: Some(1),
                    cta: None,
                };
            }
        };

        // Collect file-level stats
        let mut files = Vec::new();
        let mut total_insertions: usize = 0;
        let mut total_deletions: usize = 0;

        for delta_idx in 0..diff.deltas().len() {
            if files.len() >= limit {
                break;
            }

            let delta = diff.deltas().nth(delta_idx).unwrap();

            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let status = match delta.status() {
                Delta::Added => "A",
                Delta::Deleted => "D",
                Delta::Modified => "M",
                Delta::Renamed => "R",
                Delta::Copied => "C",
                _ => "?",
            };

            let (insertions, deletions) =
                if let Ok(Some(ref p)) = git2::Patch::from_diff(&diff, delta_idx) {
                    let (_, adds, dels) = p.line_stats().unwrap_or((0, 0, 0));
                    (adds, dels)
                } else {
                    (0, 0)
                };

            total_insertions += insertions;
            total_deletions += deletions;

            files.push(json!({
                "path": path,
                "status": status,
                "insertions": insertions,
                "deletions": deletions,
            }));
        }

        // -----------------------------------------------------------------
        // Semantic diff: symbol-level change classification
        // -----------------------------------------------------------------
        let base_oid = base_commit.id();
        let head_oid = head_commit.id();

        let mut all_symbol_changes: Vec<SymbolChange> = Vec::new();

        for delta_idx in 0..diff.deltas().len() {
            let delta = diff.deltas().nth(delta_idx).unwrap();

            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            // Detect language from file extension
            let ext = std::path::Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let language = detect_language(ext);

            // Only analyze files with supported symbol extraction
            if !matches!(
                language,
                "Rust" | "TypeScript" | "TSX" | "JavaScript" | "JSX" | "Python" | "Go"
            ) {
                continue;
            }

            // Read file content at both commits
            let base_content = read_file_at_commit(&repo, base_oid, &path);
            let head_content = read_file_at_commit(&repo, head_oid, &path);

            // Extract hunks (new-side line ranges) from the patch
            let mut hunks: Vec<(usize, usize)> = Vec::new();
            if let Ok(Some(ref p)) = git2::Patch::from_diff(&diff, delta_idx) {
                for hunk_idx in 0..p.num_hunks() {
                    if let Ok((hunk, _)) = p.hunk(hunk_idx) {
                        hunks.push((hunk.new_start() as usize, hunk.new_lines() as usize));
                    }
                }
            }

            let file_changes = diff_file_symbols(
                base_content.as_deref(),
                head_content.as_deref(),
                &path,
                language,
                &hunks,
            );
            all_symbol_changes.extend(file_changes);
        }

        // Partition symbol changes by type
        let mut symbols_added = Vec::new();
        let mut symbols_removed = Vec::new();
        let mut symbols_modified = Vec::new();

        for change in &all_symbol_changes {
            let entry = json!({
                "name": change.name,
                "kind": change.kind,
                "file": change.file_path,
                "line": change.line_start,
            });
            match change.change_type {
                ChangeType::Added => symbols_added.push(entry),
                ChangeType::Removed => symbols_removed.push(entry),
                ChangeType::Modified => symbols_modified.push(entry),
            }
        }

        let base_short = &base_commit.id().to_string()[..7];
        let head_short = &head_commit.id().to_string()[..7];

        CommandResult::Ok {
            data: json!({
                "base": base_short,
                "head": head_short,
                "summary": {
                    "files_changed": files.len(),
                    "insertions": total_insertions,
                    "deletions": total_deletions,
                },
                "files": files,
                "semantic": {
                    "symbols_added": symbols_added,
                    "symbols_removed": symbols_removed,
                    "symbols_modified": symbols_modified,
                    "summary": {
                        "added": symbols_added.len(),
                        "removed": symbols_removed.len(),
                        "modified": symbols_modified.len(),
                    },
                },
            }),
            cta: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn build() -> CommandDef {
    CommandDef::build("diff", DiffHandler)
        .description("Compare two Git refs and return file-level diff stats")
        .args::<DiffArgs>()
        .options::<DiffOptions>()
        .examples(vec![
            Example {
                command: "HEAD~1 HEAD".to_string(),
                description: Some("Diff between previous and current commit".to_string()),
            },
            Example {
                command: "main feature-branch --json".to_string(),
                description: Some("Diff between main and a feature branch".to_string()),
            },
        ])
        .done()
}
