use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;

pub struct BlameProvider {
    pub path_filter: Option<String>,
}

impl BlameProvider {
    pub fn new(path_filter: Option<String>) -> Self {
        Self { path_filter }
    }
}

impl Provider for BlameProvider {
    fn table_name(&self) -> &'static str {
        "blame"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO blame (
                path, line_number, commit_id, original_line, original_path,
                author_name, author_email, authored_at, line_content, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        // If no path filter, we need to blame all files in HEAD
        // This can be expensive, so we'll limit to tracked files
        let paths_to_blame: Vec<String> = if let Some(ref filter) = self.path_filter {
            vec![filter.clone()]
        } else {
            // Get all files from HEAD tree
            if let Ok(head) = git_repo.head() {
                if let Ok(tree) = head.peel_to_tree() {
                    let mut paths = Vec::new();
                    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
                        if entry.kind() == Some(git2::ObjectType::Blob) {
                            let path = if dir.is_empty() {
                                entry.name().unwrap_or("").to_string()
                            } else {
                                format!("{}{}", dir, entry.name().unwrap_or(""))
                            };
                            paths.push(path);
                        }
                        git2::TreeWalkResult::Ok
                    })?;
                    paths
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };

        for path in paths_to_blame {
            if let Ok(blame) = git_repo.blame_file(std::path::Path::new(&path), None) {
                // Read file content to get line text
                let file_content = if let Ok(head) = git_repo.head() {
                    if let Ok(tree) = head.peel_to_tree() {
                        if let Ok(entry) = tree.get_path(std::path::Path::new(&path)) {
                            if let Ok(blob) = git_repo.find_blob(entry.id()) {
                                if !blob.is_binary() {
                                    String::from_utf8_lossy(blob.content()).to_string()
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                let lines: Vec<&str> = file_content.lines().collect();

                for (line_idx, hunk) in blame.iter().enumerate() {
                    let line_number = (line_idx + 1) as i64;
                    let commit_id = hunk.final_commit_id().to_string();
                    let original_line = hunk.orig_start_line() as i64;
                    let original_path = hunk
                        .path()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());

                    let sig = hunk.final_signature();
                    let author_name = sig.name().unwrap_or("").to_string();
                    let author_email = sig.email().unwrap_or("").to_string();
                    let authored_at = format_git_time(sig.when());

                    let line_content = lines.get(line_idx).unwrap_or(&"").to_string();

                    stmt.execute((
                        &path,
                        line_number,
                        &commit_id,
                        original_line,
                        &original_path,
                        &author_name,
                        &author_email,
                        &authored_at,
                        &line_content,
                        &repo_path,
                    ))?;
                }
            }
        }

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
