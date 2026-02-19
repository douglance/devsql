use crate::config::Config;
use crate::error::Result;
use crate::streaming;

pub struct CodexHistoryDataSource {
    config: Config,
}

impl CodexHistoryDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn load_raw(&self) -> Result<Vec<serde_json::Value>> {
        streaming::read_jsonl_raw(self.config.jhistory_file()).await
    }
}
