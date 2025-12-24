use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;
use std::fs;
use std::os::unix::fs::PermissionsExt;

pub struct HooksProvider;

impl Provider for HooksProvider {
    fn table_name(&self) -> &'static str {
        "hooks"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO hooks (
                name, path, is_executable, is_sample, size, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        let hooks_dir = git_repo.path().join("hooks");

        if hooks_dir.exists() && hooks_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&hooks_dir) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.is_file() {
                        let file_name = entry.file_name().to_string_lossy().to_string();

                        // Extract hook name (remove .sample suffix if present)
                        let is_sample = file_name.ends_with(".sample");
                        let hook_name = if is_sample {
                            file_name.strip_suffix(".sample").unwrap_or(&file_name)
                        } else {
                            &file_name
                        };

                        // Check if it's a recognized hook name
                        if !is_valid_hook_name(hook_name) && !is_sample {
                            continue;
                        }

                        let metadata = fs::metadata(&file_path)?;
                        let is_executable = metadata.permissions().mode() & 0o111 != 0;
                        let size = metadata.len() as i64;

                        stmt.execute((
                            hook_name,
                            file_path.to_string_lossy().to_string(),
                            if is_executable { 1 } else { 0 },
                            if is_sample { 1 } else { 0 },
                            size,
                            &repo_path,
                        ))?;
                    }
                }
            }
        }

        Ok(())
    }
}

fn is_valid_hook_name(name: &str) -> bool {
    matches!(
        name,
        "applypatch-msg"
            | "pre-applypatch"
            | "post-applypatch"
            | "pre-commit"
            | "pre-merge-commit"
            | "prepare-commit-msg"
            | "commit-msg"
            | "post-commit"
            | "pre-rebase"
            | "post-checkout"
            | "post-merge"
            | "pre-push"
            | "pre-receive"
            | "update"
            | "proc-receive"
            | "post-receive"
            | "post-update"
            | "reference-transaction"
            | "push-to-checkout"
            | "pre-auto-gc"
            | "post-rewrite"
            | "sendemail-validate"
            | "fsmonitor-watchman"
            | "p4-changelist"
            | "p4-prepare-changelist"
            | "p4-post-changelist"
            | "p4-pre-submit"
            | "post-index-change"
    )
}
