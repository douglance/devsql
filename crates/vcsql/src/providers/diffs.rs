use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct DiffsProvider;

impl Provider for DiffsProvider {
    fn table_name(&self) -> &'static str {
        "diffs"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO diffs (
                commit_id, parent_id, files_changed, insertions, deletions, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        for commit_result in repo.walk_commits()? {
            let commit = commit_result?;
            let commit_id = commit.id().to_string();
            let tree = commit.tree()?;

            if commit.parent_count() == 0 {
                // Root commit - diff against empty tree
                let diff = git_repo.diff_tree_to_tree(None, Some(&tree), None)?;
                let stats = diff.stats()?;

                stmt.execute((
                    &commit_id,
                    Option::<String>::None,
                    stats.files_changed() as i64,
                    stats.insertions() as i64,
                    stats.deletions() as i64,
                    &repo_path,
                ))?;
            } else {
                // Diff against each parent
                for parent in commit.parents() {
                    let parent_id = parent.id().to_string();
                    let parent_tree = parent.tree()?;

                    let diff = git_repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)?;
                    let stats = diff.stats()?;

                    stmt.execute((
                        &commit_id,
                        Some(&parent_id),
                        stats.files_changed() as i64,
                        stats.insertions() as i64,
                        stats.deletions() as i64,
                        &repo_path,
                    ))?;
                }
            }
        }

        Ok(())
    }
}
