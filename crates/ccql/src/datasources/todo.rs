use crate::config::Config;
use crate::error::Result;
use crate::models::{TodoEntry, TodoFile, TodoStatus};
use crate::streaming;
use walkdir::WalkDir;

pub struct TodoDataSource {
    config: Config,
}

impl TodoDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn load_all(&self) -> Result<Vec<TodoFile>> {
        let todos_dir = self.config.todos_dir();
        if !todos_dir.exists() {
            return Ok(vec![]);
        }

        let mut todo_files = Vec::new();

        for entry in WalkDir::new(&todos_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                    match streaming::read_json::<Vec<TodoEntry>>(path).await {
                        Ok(todos) if !todos.is_empty() => {
                            if let Some(todo_file) = TodoFile::from_filename(filename, todos) {
                                todo_files.push(todo_file);
                            }
                        }
                        Ok(_) => {} // Empty todo list
                        Err(e) => tracing::debug!("Failed to parse {}: {}", filename, e),
                    }
                }
            }
        }

        Ok(todo_files)
    }

    pub async fn filter_by_status(&self, status: TodoStatus) -> Result<Vec<(TodoFile, Vec<&TodoEntry>)>> {
        let files = self.load_all().await?;
        // Need to return owned data since we're filtering
        let filtered: Vec<_> = files
            .into_iter()
            .filter_map(|f| {
                let matching: Vec<TodoEntry> = f.todos.iter().filter(|t| t.status == status).cloned().collect();
                if matching.is_empty() {
                    None
                } else {
                    Some(TodoFile {
                        workspace_id: f.workspace_id,
                        agent_id: f.agent_id,
                        todos: matching,
                    })
                }
            })
            .collect();

        Ok(filtered.into_iter().map(|f| (f, vec![])).collect())
    }

    pub async fn all_todos_flat(&self) -> Result<Vec<(String, String, TodoEntry)>> {
        let files = self.load_all().await?;
        let mut flat = Vec::new();

        for file in files {
            for todo in file.todos {
                flat.push((file.workspace_id.clone(), file.agent_id.clone(), todo));
            }
        }

        Ok(flat)
    }
}
