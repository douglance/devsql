use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use chrono::{TimeZone, Utc};
use git2::Oid;
use rusqlite::Connection;

pub struct StashesProvider;

impl Provider for StashesProvider {
    fn table_name(&self) -> &'static str {
        "stashes"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO stashes (
                stash_index, commit_id, message, author_name, author_email,
                created_at, branch, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )?;

        let repo_path = repo.path().to_string();

        // First, collect all stash info
        let mut stashes: Vec<(usize, String, Oid)> = Vec::new();
        {
            let git_repo = repo.inner_mut();
            let mut stash_index = 0usize;
            git_repo.stash_foreach(|_index, message, oid| {
                stashes.push((stash_index, message.to_string(), *oid));
                stash_index += 1;
                true
            })?;
        }

        // Now process each stash with immutable access
        let git_repo = repo.inner();
        for (stash_index, msg, oid) in stashes {
            if let Ok(commit) = git_repo.find_commit(oid) {
                let commit_id = oid.to_string();

                let author = commit.author();
                let author_name = author.name().unwrap_or("").to_string();
                let author_email = author.email().unwrap_or("").to_string();
                let created_at = format_git_time(author.when());

                let branch = extract_branch_from_message(&msg);

                stmt.execute((
                    stash_index as i64,
                    &commit_id,
                    &msg,
                    &author_name,
                    &author_email,
                    &created_at,
                    &branch,
                    &repo_path,
                ))?;
            }
        }

        Ok(())
    }
}

fn extract_branch_from_message(message: &str) -> String {
    // Parse patterns like "WIP on main: abc123 message" or "On main: message"
    if let Some(rest) = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))
    {
        if let Some(colon_pos) = rest.find(':') {
            return rest[..colon_pos].to_string();
        }
    }
    "unknown".to_string()
}

fn format_git_time(time: git2::Time) -> String {
    let timestamp = time.seconds();
    if let Some(dt) = Utc.timestamp_opt(timestamp, 0).single() {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        timestamp.to_string()
    }
}
