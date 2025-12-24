use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub display: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(rename = "pastedContents", default)]
    pub pasted_contents: HashMap<String, serde_json::Value>,
}

impl HistoryEntry {
    pub fn is_user_prompt(&self) -> bool {
        !self.display.is_empty() && !self.display.starts_with('/')
    }

    pub fn is_command(&self) -> bool {
        self.display.starts_with('/')
    }

    pub fn timestamp_datetime(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(self.timestamp)
            .unwrap_or_else(chrono::Utc::now)
    }

    pub fn formatted_time(&self) -> String {
        self.timestamp_datetime()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    pub fn project_name(&self) -> Option<&str> {
        self.project.as_ref().and_then(|p| p.split('/').next_back())
    }
}
