use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;

pub struct CommitsProvider;

impl Provider for CommitsProvider {
    fn table_name(&self) -> &'static str {
        "commits"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO commits (
                id, short_id, tree_id,
                author_name, author_email, authored_at,
                committer_name, committer_email, committed_at,
                message, summary, body,
                parent_count, is_merge, repo
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
            )
            "#,
        )?;

        let repo_path = repo.path().to_string();

        for commit_result in repo.walk_commits()? {
            let commit = commit_result?;

            let id = commit.id().to_string();
            let short_id = &id[..7.min(id.len())];
            let tree_id = commit.tree_id().to_string();

            let author = commit.author();
            let author_name = author.name().unwrap_or("").to_string();
            let author_email = author.email().unwrap_or("").to_string();
            let authored_at = format_git_time(author.when());

            let committer = commit.committer();
            let committer_name = committer.name().unwrap_or("").to_string();
            let committer_email = committer.email().unwrap_or("").to_string();
            let committed_at = format_git_time(committer.when());

            let message = commit.message().unwrap_or("").to_string();
            let summary = commit.summary().unwrap_or("").to_string();
            let body = commit
                .body()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());

            let parent_count = commit.parent_count() as i64;
            let is_merge = if parent_count > 1 { 1 } else { 0 };

            stmt.execute((
                &id,
                short_id,
                &tree_id,
                &author_name,
                &author_email,
                &authored_at,
                &committer_name,
                &committer_email,
                &committed_at,
                &message,
                &summary,
                &body,
                parent_count,
                is_merge,
                &repo_path,
            ))?;
        }

        Ok(())
    }
}

fn format_git_time(time: git2::Time) -> String {
    let timestamp = time.seconds();
    let offset_minutes = time.offset_minutes();

    if let Some(dt) = Utc.timestamp_opt(timestamp, 0).single() {
        let offset_hours = offset_minutes / 60;
        let offset_mins = (offset_minutes % 60).abs();
        let sign = if offset_minutes >= 0 { '+' } else { '-' };
        format!(
            "{} {}{:02}{:02}",
            dt.format("%Y-%m-%d %H:%M:%S"),
            sign,
            offset_hours.abs(),
            offset_mins
        )
    } else {
        timestamp.to_string()
    }
}
