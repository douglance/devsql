use crate::config::Config;
use crate::error::Result;
use crate::models::HistoryEntry;
use crate::streaming;

pub struct HistoryDataSource {
    config: Config,
}

impl HistoryDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn load_all(&self) -> Result<Vec<HistoryEntry>> {
        streaming::read_jsonl(self.config.history_file()).await
    }

    pub async fn load_raw(&self) -> Result<Vec<serde_json::Value>> {
        streaming::read_jsonl_raw(self.config.history_file()).await
    }

    pub async fn filter_prompts(&self) -> Result<Vec<HistoryEntry>> {
        let entries = self.load_all().await?;
        Ok(entries.into_iter().filter(|e| e.is_user_prompt()).collect())
    }

    pub async fn filter_by_project(&self, project: &str) -> Result<Vec<HistoryEntry>> {
        let entries = self.load_all().await?;
        Ok(entries
            .into_iter()
            .filter(|e| {
                e.project
                    .as_ref()
                    .map(|p| p.contains(project))
                    .unwrap_or(false)
            })
            .collect())
    }

    pub async fn filter_by_date_range(
        &self,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Vec<HistoryEntry>> {
        let entries = self.load_all().await?;
        Ok(entries
            .into_iter()
            .filter(|e| {
                let ts = e.timestamp;
                since.map_or(true, |s| ts >= s) && until.map_or(true, |u| ts <= u)
            })
            .collect())
    }
}
