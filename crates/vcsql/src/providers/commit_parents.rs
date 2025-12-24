use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct CommitParentsProvider;

impl Provider for CommitParentsProvider {
    fn table_name(&self) -> &'static str {
        "commit_parents"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO commit_parents (commit_id, parent_id, parent_index, repo)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )?;

        let repo_path = repo.path().to_string();

        for commit_result in repo.walk_commits()? {
            let commit = commit_result?;
            let commit_id = commit.id().to_string();

            for (index, parent) in commit.parents().enumerate() {
                let parent_id = parent.id().to_string();
                stmt.execute((&commit_id, &parent_id, index as i64, &repo_path))?;
            }
        }

        Ok(())
    }
}
