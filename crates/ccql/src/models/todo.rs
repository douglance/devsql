use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoEntry {
    pub content: String,
    pub status: TodoStatus,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TodoFile {
    pub workspace_id: String,
    pub agent_id: String,
    pub todos: Vec<TodoEntry>,
}

impl TodoFile {
    pub fn from_filename(filename: &str, todos: Vec<TodoEntry>) -> Option<Self> {
        let parts: Vec<&str> = filename.trim_end_matches(".json").split("-agent-").collect();
        if parts.len() == 2 {
            Some(TodoFile {
                workspace_id: parts[0].to_string(),
                agent_id: parts[1].to_string(),
                todos,
            })
        } else {
            None
        }
    }
}
