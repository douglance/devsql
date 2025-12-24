use crate::config::Config;
use crate::error::Result;
use crate::models::StatsCache;
use crate::streaming;

pub struct StatsDataSource {
    config: Config,
}

impl StatsDataSource {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn load(&self) -> Result<StatsCache> {
        streaming::read_json(self.config.stats_file()).await
    }

    pub async fn load_raw(&self) -> Result<serde_json::Value> {
        streaming::read_json(self.config.stats_file()).await
    }
}
