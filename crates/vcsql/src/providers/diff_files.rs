use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use git2::{Delta, DiffOptions};
use rusqlite::Connection;

pub struct DiffFilesProvider;

impl Provider for DiffFilesProvider {
    fn table_name(&self) -> &'static str {
        "diff_files"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO diff_files (
                commit_id, parent_id, old_path, new_path, status,
                insertions, deletions, is_binary, similarity, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        let mut diff_opts = DiffOptions::new();
        diff_opts.include_untracked(false);

        for commit_result in repo.walk_commits()? {
            let commit = commit_result?;
            let commit_id = commit.id().to_string();
            let tree = commit.tree()?;

            if commit.parent_count() == 0 {
                // Root commit
                let diff = git_repo.diff_tree_to_tree(None, Some(&tree), Some(&mut diff_opts))?;
                insert_diff_files(&mut stmt, &diff, &commit_id, None, &repo_path)?;
            } else {
                for parent in commit.parents() {
                    let parent_id = parent.id().to_string();
                    let parent_tree = parent.tree()?;

                    let diff = git_repo.diff_tree_to_tree(
                        Some(&parent_tree),
                        Some(&tree),
                        Some(&mut diff_opts),
                    )?;
                    insert_diff_files(&mut stmt, &diff, &commit_id, Some(&parent_id), &repo_path)?;
                }
            }
        }

        Ok(())
    }
}

fn insert_diff_files(
    stmt: &mut rusqlite::Statement,
    diff: &git2::Diff,
    commit_id: &str,
    parent_id: Option<&str>,
    repo_path: &str,
) -> Result<()> {
    for (delta_idx, delta) in diff.deltas().enumerate() {
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().to_string());
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().to_string());

        let status = match delta.status() {
            Delta::Added => "A",
            Delta::Deleted => "D",
            Delta::Modified => "M",
            Delta::Renamed => "R",
            Delta::Copied => "C",
            Delta::Typechange => "T",
            Delta::Unmodified => "U",
            Delta::Ignored => "I",
            Delta::Untracked => "?",
            Delta::Conflicted => "X",
            Delta::Unreadable => "!",
        };

        let is_binary = delta.old_file().is_binary() || delta.new_file().is_binary();

        // Get line stats for this specific file
        let mut insertions = 0i64;
        let mut deletions = 0i64;

        if let Ok(Some(patch)) = git2::Patch::from_diff(diff, delta_idx) {
            let (_, adds, dels) = patch.line_stats()?;
            insertions = adds as i64;
            deletions = dels as i64;
        }

        // Similarity percentage is not directly available in git2-rs API
        // We'd need to compute it ourselves or skip it
        let similarity: Option<i64> = None;

        stmt.execute((
            commit_id,
            parent_id,
            &old_path,
            &new_path,
            status,
            insertions,
            deletions,
            if is_binary { 1 } else { 0 },
            similarity,
            repo_path,
        ))?;
    }

    Ok(())
}
