use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct SubmodulesProvider;

impl Provider for SubmodulesProvider {
    fn table_name(&self) -> &'static str {
        "submodules"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO submodules (
                name, path, url, branch, head_id, status, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        if let Ok(submodules) = git_repo.submodules() {
            for submodule in submodules {
                let name = submodule.name().unwrap_or("").to_string();
                let path = submodule.path().to_string_lossy().to_string();
                let url = submodule.url().unwrap_or("").to_string();
                let branch = submodule.branch().map(|b| b.to_string());
                let head_id = submodule.head_id().map(|oid| oid.to_string());

                let status = match (submodule.head_id(), submodule.workdir_id()) {
                    (None, None) => "uninitialized",
                    (Some(head), Some(wd)) if head == wd => "current",
                    (Some(_), Some(_)) => "modified",
                    (Some(_), None) => "uninitialized",
                    (None, Some(_)) => "added",
                };

                stmt.execute((&name, &path, &url, &branch, &head_id, status, &repo_path))?;
            }
        }

        Ok(())
    }
}
