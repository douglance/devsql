use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsCache {
    pub version: u32,
    #[serde(rename = "lastComputedDate")]
    pub last_computed_date: String,
    #[serde(rename = "dailyActivity")]
    pub daily_activity: Vec<DailyActivity>,
    #[serde(rename = "dailyModelTokens", default)]
    pub daily_model_tokens: Vec<DailyModelTokens>,
    #[serde(rename = "hourCounts", default)]
    pub hour_counts: HashMap<String, u64>,
    #[serde(rename = "modelUsage", default)]
    pub model_usage: HashMap<String, ModelUsageData>,
    #[serde(rename = "totalMessages")]
    pub total_messages: u64,
    #[serde(rename = "totalSessions")]
    pub total_sessions: u64,
    #[serde(rename = "firstSessionDate")]
    pub first_session_date: String,
    #[serde(rename = "longestSession", skip_serializing_if = "Option::is_none")]
    pub longest_session: Option<LongestSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyActivity {
    pub date: String,
    #[serde(rename = "messageCount")]
    pub message_count: u64,
    #[serde(rename = "sessionCount")]
    pub session_count: u64,
    #[serde(rename = "toolCallCount")]
    pub tool_call_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyModelTokens {
    pub date: String,
    #[serde(rename = "tokensByModel", default)]
    pub tokens_by_model: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageData {
    #[serde(rename = "inputTokens", default)]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens", default)]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadInputTokens", default)]
    pub cache_read_input_tokens: u64,
    #[serde(rename = "cacheCreationInputTokens", default)]
    pub cache_creation_input_tokens: u64,
    #[serde(rename = "webSearchRequests", default)]
    pub web_search_requests: u64,
    #[serde(rename = "costUSD", default)]
    pub cost_usd: f64,
    #[serde(rename = "contextWindow", default)]
    pub context_window: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongestSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default)]
    pub duration: u64,
    #[serde(rename = "messageCount")]
    pub message_count: u64,
    #[serde(default)]
    pub timestamp: String,
}

impl StatsCache {
    pub fn total_tokens(&self) -> u64 {
        self.model_usage
            .values()
            .map(|m| m.input_tokens + m.output_tokens)
            .sum()
    }

    pub fn activity_by_date(&self, date: &str) -> Option<&DailyActivity> {
        self.daily_activity.iter().find(|a| a.date == date)
    }
}
