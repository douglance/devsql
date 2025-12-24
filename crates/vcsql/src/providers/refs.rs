use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct RefsProvider;

impl Provider for RefsProvider {
    fn table_name(&self) -> &'static str {
        "refs"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO refs (
                name, full_name, target_id, kind, is_symbolic, symbolic_target, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        for reference_result in git_repo.references()? {
            let reference = reference_result?;

            let full_name = reference.name().unwrap_or("").to_string();
            let short_name = reference.shorthand().unwrap_or("").to_string();

            let kind = if full_name.starts_with("refs/heads/") {
                "branch"
            } else if full_name.starts_with("refs/remotes/") {
                "remote"
            } else if full_name.starts_with("refs/tags/") {
                "tag"
            } else if full_name.starts_with("refs/notes/") {
                "note"
            } else if full_name.starts_with("refs/stash") {
                "stash"
            } else {
                "other"
            };

            let is_symbolic = reference.kind() == Some(git2::ReferenceType::Symbolic);

            let symbolic_target = if is_symbolic {
                reference.symbolic_target().map(|s| s.to_string())
            } else {
                None
            };

            let target_id = if is_symbolic {
                reference
                    .resolve()
                    .ok()
                    .and_then(|r| r.target())
                    .map(|oid| oid.to_string())
                    .unwrap_or_default()
            } else {
                reference
                    .target()
                    .map(|oid| oid.to_string())
                    .unwrap_or_default()
            };

            stmt.execute((
                &short_name,
                &full_name,
                &target_id,
                kind,
                if is_symbolic { 1 } else { 0 },
                &symbolic_target,
                &repo_path,
            ))?;
        }

        Ok(())
    }
}
