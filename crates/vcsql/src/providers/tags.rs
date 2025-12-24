use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;

pub struct TagsProvider;

impl Provider for TagsProvider {
    fn table_name(&self) -> &'static str {
        "tags"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO tags (
                name, full_name, target_id, target_type, is_annotated,
                tagger_name, tagger_email, tagged_at, message, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        git_repo.tag_foreach(|oid, name_bytes| {
            let name_str = String::from_utf8_lossy(name_bytes);
            let full_name = name_str.to_string();
            let short_name = full_name
                .strip_prefix("refs/tags/")
                .unwrap_or(&full_name)
                .to_string();

            if let Ok(obj) = git_repo.find_object(oid, None) {
                let (
                    target_id,
                    target_type,
                    is_annotated,
                    tagger_name,
                    tagger_email,
                    tagged_at,
                    message,
                ) = if let Some(tag) = obj.as_tag() {
                    let target = tag.target_id().to_string();
                    let target_type = match tag.target_type() {
                        Some(git2::ObjectType::Commit) => "commit",
                        Some(git2::ObjectType::Tree) => "tree",
                        Some(git2::ObjectType::Blob) => "blob",
                        Some(git2::ObjectType::Tag) => "tag",
                        _ => "unknown",
                    };
                    let tagger = tag.tagger();
                    let tagger_name = tagger
                        .as_ref()
                        .and_then(|t| t.name().map(|s| s.to_string()));
                    let tagger_email = tagger
                        .as_ref()
                        .and_then(|t| t.email().map(|s| s.to_string()));
                    let tagged_at = tagger.as_ref().map(|t| format_git_time(t.when()));
                    let message = tag.message().map(|s| s.to_string());
                    (
                        target,
                        target_type.to_string(),
                        true,
                        tagger_name,
                        tagger_email,
                        tagged_at,
                        message,
                    )
                } else {
                    // Lightweight tag - points directly to a commit
                    let target_type = match obj.kind() {
                        Some(git2::ObjectType::Commit) => "commit",
                        Some(git2::ObjectType::Tree) => "tree",
                        Some(git2::ObjectType::Blob) => "blob",
                        _ => "unknown",
                    };
                    (
                        oid.to_string(),
                        target_type.to_string(),
                        false,
                        None,
                        None,
                        None,
                        None,
                    )
                };

                let _ = stmt.execute((
                    &short_name,
                    &full_name,
                    &target_id,
                    &target_type,
                    if is_annotated { 1 } else { 0 },
                    &tagger_name,
                    &tagger_email,
                    &tagged_at,
                    &message,
                    &repo_path,
                ));
            }
            true
        })?;

        Ok(())
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
