use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;

pub struct ReflogProvider;

impl Provider for ReflogProvider {
    fn table_name(&self) -> &'static str {
        "reflog"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO reflog (
                ref_name, entry_index, old_id, new_id,
                committer_name, committer_email, committed_at,
                message, action, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        // Get reflog for HEAD
        if let Ok(reflog) = git_repo.reflog("HEAD") {
            for (index, entry) in reflog.iter().enumerate() {
                let old_id = entry.id_old().to_string();
                let new_id = entry.id_new().to_string();

                let committer = entry.committer();
                let committer_name = committer.name().unwrap_or("").to_string();
                let committer_email = committer.email().unwrap_or("").to_string();
                let committed_at = format_git_time(committer.when());

                let message = entry.message().unwrap_or("").to_string();
                let action = extract_action(&message);

                stmt.execute((
                    "HEAD",
                    index as i64,
                    &old_id,
                    &new_id,
                    &committer_name,
                    &committer_email,
                    &committed_at,
                    &message,
                    &action,
                    &repo_path,
                ))?;
            }
        }

        // Get reflog for all branches
        for reference in git_repo.references()?.flatten() {
            if let Some(name) = reference.name() {
                if name.starts_with("refs/heads/") {
                    if let Ok(reflog) = git_repo.reflog(name) {
                        for (index, entry) in reflog.iter().enumerate() {
                            let old_id = entry.id_old().to_string();
                            let new_id = entry.id_new().to_string();

                            let committer = entry.committer();
                            let committer_name = committer.name().unwrap_or("").to_string();
                            let committer_email = committer.email().unwrap_or("").to_string();
                            let committed_at = format_git_time(committer.when());

                            let message = entry.message().unwrap_or("").to_string();
                            let action = extract_action(&message);

                            stmt.execute((
                                name,
                                index as i64,
                                &old_id,
                                &new_id,
                                &committer_name,
                                &committer_email,
                                &committed_at,
                                &message,
                                &action,
                                &repo_path,
                            ))?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn extract_action(message: &str) -> String {
    let msg_lower = message.to_lowercase();
    if msg_lower.starts_with("commit:")
        || msg_lower.starts_with("commit (initial):")
        || msg_lower.starts_with("commit (amend):")
    {
        "commit".to_string()
    } else if msg_lower.starts_with("checkout:") {
        "checkout".to_string()
    } else if msg_lower.starts_with("merge") {
        "merge".to_string()
    } else if msg_lower.starts_with("rebase") {
        "rebase".to_string()
    } else if msg_lower.starts_with("reset:") {
        "reset".to_string()
    } else if msg_lower.starts_with("pull:") {
        "pull".to_string()
    } else if msg_lower.starts_with("push") {
        "push".to_string()
    } else if msg_lower.starts_with("branch:") {
        "branch".to_string()
    } else if msg_lower.starts_with("clone:") {
        "clone".to_string()
    } else if msg_lower.starts_with("cherry-pick:") {
        "cherry-pick".to_string()
    } else if msg_lower.starts_with("revert:") {
        "revert".to_string()
    } else {
        "other".to_string()
    }
}

fn format_git_time(time: git2::Time) -> String {
    let timestamp = time.seconds();
    if let Some(dt) = Utc.timestamp_opt(timestamp, 0).single() {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        timestamp.to_string()
    }
}
