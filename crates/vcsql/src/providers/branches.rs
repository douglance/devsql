use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use git2::BranchType;
use rusqlite::Connection;

pub struct BranchesProvider;

impl Provider for BranchesProvider {
    fn table_name(&self) -> &'static str {
        "branches"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO branches (
                name, full_name, target_id, is_remote, is_head,
                remote_name, upstream, ahead, behind, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();

        let head_target = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());

        let is_detached = repo.is_head_detached();

        for branch_result in repo.branches(None)? {
            let (branch, branch_type) = branch_result?;

            let name = branch.name()?.unwrap_or("").to_string();
            let reference = branch.get();
            let full_name = reference.name().unwrap_or("").to_string();

            let target_id = reference
                .peel_to_commit()
                .map(|c| c.id().to_string())
                .unwrap_or_default();

            let is_remote = matches!(branch_type, BranchType::Remote);

            let is_head = if is_detached {
                false
            } else {
                head_target
                    .as_ref()
                    .map(|h| h == &target_id && !is_remote)
                    .unwrap_or(false)
                    && reference.is_branch()
                    && repo
                        .head()
                        .ok()
                        .and_then(|h| h.name().map(|n| n == full_name))
                        .unwrap_or(false)
            };

            let remote_name: Option<String> = if is_remote {
                name.split('/').next().map(|s| s.to_string())
            } else {
                None
            };

            let (upstream, ahead, behind) = if !is_remote {
                if let Ok(upstream_branch) = branch.upstream() {
                    let upstream_name =
                        upstream_branch.name().ok().flatten().map(|s| s.to_string());
                    let (ahead, behind) = if let (Some(local_oid), Ok(upstream_ref)) =
                        (reference.target(), upstream_branch.get().peel_to_commit())
                    {
                        repo.graph_ahead_behind(local_oid, upstream_ref.id())
                            .unwrap_or((0, 0))
                    } else {
                        (0, 0)
                    };
                    (upstream_name, Some(ahead as i64), Some(behind as i64))
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };

            stmt.execute((
                &name,
                &full_name,
                &target_id,
                if is_remote { 1 } else { 0 },
                if is_head { 1 } else { 0 },
                &remote_name,
                &upstream,
                ahead,
                behind,
                &repo_path,
            ))?;
        }

        Ok(())
    }
}
