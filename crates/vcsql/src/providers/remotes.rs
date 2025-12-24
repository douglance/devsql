use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct RemotesProvider;

impl Provider for RemotesProvider {
    fn table_name(&self) -> &'static str {
        "remotes"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO remotes (
                name, url, push_url, fetch_refspec, push_refspec, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        if let Ok(remotes) = git_repo.remotes() {
            for remote_name in remotes.iter().flatten() {
                if let Ok(remote) = git_repo.find_remote(remote_name) {
                    let name = remote_name.to_string();
                    let url = remote.url().map(|s| s.to_string());
                    let push_url = remote.pushurl().map(|s| s.to_string());

                    // Get fetch refspecs
                    let fetch_refspec = remote.fetch_refspecs().ok().and_then(|specs| {
                        let specs: Vec<String> =
                            specs.iter().flatten().map(|s| s.to_string()).collect();
                        if specs.is_empty() {
                            None
                        } else {
                            Some(specs.join(", "))
                        }
                    });

                    // Get push refspecs
                    let push_refspec = remote.push_refspecs().ok().and_then(|specs| {
                        let specs: Vec<String> =
                            specs.iter().flatten().map(|s| s.to_string()).collect();
                        if specs.is_empty() {
                            None
                        } else {
                            Some(specs.join(", "))
                        }
                    });

                    stmt.execute((
                        &name,
                        &url,
                        &push_url,
                        &fetch_refspec,
                        &push_refspec,
                        &repo_path,
                    ))?;
                }
            }
        }

        Ok(())
    }
}
