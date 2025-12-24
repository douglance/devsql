use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use git2::{Status, StatusOptions};
use rusqlite::Connection;

pub struct StatusProvider;

impl Provider for StatusProvider {
    fn table_name(&self) -> &'static str {
        "status"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO status (
                path, status_code, head_status, index_status,
                is_staged, is_modified, is_new, is_deleted,
                is_renamed, is_copied, is_ignored, is_conflicted, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .include_ignored(false)
            .include_unmodified(false)
            .recurse_untracked_dirs(true);

        if let Ok(statuses) = git_repo.statuses(Some(&mut opts)) {
            for entry in statuses.iter() {
                let path = entry.path().unwrap_or("").to_string();
                let status = entry.status();

                let (status_code, head_status, index_status) = format_status(status);

                let is_staged = status.intersects(
                    Status::INDEX_NEW
                        | Status::INDEX_MODIFIED
                        | Status::INDEX_DELETED
                        | Status::INDEX_RENAMED
                        | Status::INDEX_TYPECHANGE,
                );

                let is_modified = status.intersects(Status::WT_MODIFIED | Status::INDEX_MODIFIED);
                let is_new = status.intersects(Status::WT_NEW | Status::INDEX_NEW);
                let is_deleted = status.intersects(Status::WT_DELETED | Status::INDEX_DELETED);
                let is_renamed = status.intersects(Status::WT_RENAMED | Status::INDEX_RENAMED);
                let is_copied = status.intersects(Status::INDEX_TYPECHANGE | Status::WT_TYPECHANGE);
                let is_ignored = status.intersects(Status::IGNORED);
                let is_conflicted = status.intersects(Status::CONFLICTED);

                stmt.execute((
                    &path,
                    &status_code,
                    &head_status,
                    &index_status,
                    if is_staged { 1 } else { 0 },
                    if is_modified { 1 } else { 0 },
                    if is_new { 1 } else { 0 },
                    if is_deleted { 1 } else { 0 },
                    if is_renamed { 1 } else { 0 },
                    if is_copied { 1 } else { 0 },
                    if is_ignored { 1 } else { 0 },
                    if is_conflicted { 1 } else { 0 },
                    &repo_path,
                ))?;
            }
        }

        Ok(())
    }
}

fn format_status(status: Status) -> (String, String, String) {
    let index_char = if status.contains(Status::INDEX_NEW) {
        'A'
    } else if status.contains(Status::INDEX_MODIFIED) {
        'M'
    } else if status.contains(Status::INDEX_DELETED) {
        'D'
    } else if status.contains(Status::INDEX_RENAMED) {
        'R'
    } else if status.contains(Status::INDEX_TYPECHANGE) {
        'T'
    } else {
        ' '
    };

    let wt_char = if status.contains(Status::WT_NEW) {
        '?'
    } else if status.contains(Status::WT_MODIFIED) {
        'M'
    } else if status.contains(Status::WT_DELETED) {
        'D'
    } else if status.contains(Status::WT_RENAMED) {
        'R'
    } else if status.contains(Status::WT_TYPECHANGE) {
        'T'
    } else if status.contains(Status::IGNORED) {
        '!'
    } else if status.contains(Status::CONFLICTED) {
        'U'
    } else {
        ' '
    };

    let status_code = format!("{}{}", index_char, wt_char);
    let head_status = index_char.to_string();
    let index_status = wt_char.to_string();

    (status_code, head_status, index_status)
}
