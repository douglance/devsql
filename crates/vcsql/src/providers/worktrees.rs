use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;
use std::fs;
use std::path::Path;

pub struct WorktreesProvider;

impl Provider for WorktreesProvider {
    fn table_name(&self) -> &'static str {
        "worktrees"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO worktrees (
                name, path, head_id, branch, is_bare, is_detached,
                is_locked, lock_reason, is_prunable, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        // Add main worktree
        let main_path = git_repo
            .workdir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| git_repo.path().to_string_lossy().to_string());

        let head_id = git_repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());
        let branch = git_repo.head().ok().and_then(|h| {
            if h.is_branch() {
                h.shorthand().map(|s| s.to_string())
            } else {
                None
            }
        });
        let is_bare = git_repo.is_bare();
        let is_detached = git_repo.head_detached().unwrap_or(false);

        stmt.execute((
            "main",
            &main_path,
            &head_id,
            &branch,
            if is_bare { 1 } else { 0 },
            if is_detached { 1 } else { 0 },
            0, // not locked
            Option::<String>::None,
            0, // not prunable
            &repo_path,
        ))?;

        // Check for linked worktrees in .git/worktrees
        let worktrees_dir = git_repo.path().join("worktrees");
        if worktrees_dir.exists() && worktrees_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&worktrees_dir) {
                for entry in entries.flatten() {
                    let wt_name = entry.file_name().to_string_lossy().to_string();
                    let wt_path = entry.path();

                    // Read gitdir file to get actual worktree path
                    let gitdir_file = wt_path.join("gitdir");
                    let actual_path = if gitdir_file.exists() {
                        fs::read_to_string(&gitdir_file).ok().map(|s| {
                            let p = s.trim();
                            // gitdir points to .git file, parent is worktree
                            Path::new(p)
                                .parent()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| p.to_string())
                        })
                    } else {
                        None
                    };

                    // Read HEAD
                    let head_file = wt_path.join("HEAD");
                    let (wt_head_id, wt_branch, wt_detached) = if head_file.exists() {
                        if let Ok(content) = fs::read_to_string(&head_file) {
                            let content = content.trim();
                            if content.starts_with("ref: ") {
                                let ref_name = content.strip_prefix("ref: ").unwrap();
                                let short_name =
                                    ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);
                                (None, Some(short_name.to_string()), false)
                            } else {
                                (Some(content.to_string()), None, true)
                            }
                        } else {
                            (None, None, false)
                        }
                    } else {
                        (None, None, false)
                    };

                    // Check if locked
                    let locked_file = wt_path.join("locked");
                    let (is_locked, lock_reason) = if locked_file.exists() {
                        let reason = fs::read_to_string(&locked_file).ok();
                        (true, reason)
                    } else {
                        (false, None)
                    };

                    stmt.execute((
                        &wt_name,
                        &actual_path,
                        &wt_head_id,
                        &wt_branch,
                        0, // linked worktrees are not bare
                        if wt_detached { 1 } else { 0 },
                        if is_locked { 1 } else { 0 },
                        &lock_reason,
                        0, // TODO: check if prunable
                        &repo_path,
                    ))?;
                }
            }
        }

        Ok(())
    }
}
