use crate::error::Result;
use crate::git::GitRepo;
use crate::providers::Provider;
use rusqlite::Connection;

pub struct NotesProvider;

impl Provider for NotesProvider {
    fn table_name(&self) -> &'static str {
        "notes"
    }

    fn populate(&self, conn: &Connection, repo: &mut GitRepo) -> Result<()> {
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO notes (
                notes_ref, target_id, note_id, content, repo
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )?;

        let repo_path = repo.path().to_string();
        let git_repo = repo.inner();

        // Find all notes refs
        for reference in git_repo.references()?.flatten() {
            if let Some(name) = reference.name() {
                if name.starts_with("refs/notes/") {
                    let notes_ref = name.to_string();

                    // Get the notes tree
                    if let Ok(tree) = reference.peel_to_tree() {
                        tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
                            // Note entries are named with the target object's SHA
                            if let Some(target_name) = entry.name() {
                                if entry.kind() == Some(git2::ObjectType::Blob) {
                                    let target_id = target_name.to_string();
                                    let note_id = entry.id().to_string();

                                    // Read note content
                                    let content =
                                        if let Ok(blob) = git_repo.find_blob(entry.id()) {
                                            if !blob.is_binary() {
                                                String::from_utf8_lossy(blob.content()).to_string()
                                            } else {
                                                String::new()
                                            }
                                        } else {
                                            String::new()
                                        };

                                    let _ = stmt.execute((
                                        &notes_ref, &target_id, &note_id, &content, &repo_path,
                                    ));
                                }
                            }
                            git2::TreeWalkResult::Ok
                        })
                        .ok();
                    }
                }
            }
        }

        Ok(())
    }
}
