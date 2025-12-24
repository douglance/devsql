use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TranscriptEntry {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolCall(ToolCallMessage),
    ToolResult(ToolResultMessage),
    Generic(GenericMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub message: UserMessageContent,
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageContent {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub message: AssistantMessageContent,
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessageContent {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub tool_name: String,
    pub result: serde_json::Value,
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericMessage {
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl TranscriptEntry {
    pub fn message_type(&self) -> &str {
        match self {
            TranscriptEntry::User(m) => &m.message_type,
            TranscriptEntry::Assistant(m) => &m.message_type,
            TranscriptEntry::ToolCall(m) => &m.message_type,
            TranscriptEntry::ToolResult(m) => &m.message_type,
            TranscriptEntry::Generic(m) => m.message_type.as_deref().unwrap_or("unknown"),
        }
    }

    pub fn is_user(&self) -> bool {
        matches!(self, TranscriptEntry::User(_))
            || matches!(self, TranscriptEntry::Generic(g) if g.message_type.as_deref() == Some("user"))
    }

    pub fn content_preview(&self, max_len: usize) -> String {
        let content = match self {
            TranscriptEntry::User(m) => self.extract_text_content(&m.message.content),
            TranscriptEntry::Assistant(m) => self.extract_text_content(&m.message.content),
            TranscriptEntry::ToolCall(m) => format!("{}()", m.tool_name),
            TranscriptEntry::ToolResult(m) => format!("{} result", m.tool_name),
            TranscriptEntry::Generic(m) => {
                m.data.get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
        };

        if content.len() > max_len {
            format!("{}...", &content[..max_len])
        } else {
            content
        }
    }

    fn extract_text_content(&self, content: &serde_json::Value) -> String {
        match content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(arr) => {
                arr.iter()
                    .filter_map(|v| {
                        if v.get("type").and_then(|t| t.as_str()) == Some("text") {
                            v.get("text").and_then(|t| t.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            _ => String::new(),
        }
    }
}
