use crate::error::{Error, Result};
use serde::de::DeserializeOwned;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

pub async fn read_jsonl<T>(path: impl AsRef<Path>) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::FileNotFound(path.display().to_string()));
    }

    let file = tokio::fs::File::open(path).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut entries = Vec::new();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::debug!("Failed to parse line: {}", e);
                continue;
            }
        }
    }

    Ok(entries)
}

pub async fn read_jsonl_raw(path: impl AsRef<Path>) -> Result<Vec<serde_json::Value>> {
    read_jsonl::<serde_json::Value>(path).await
}

pub async fn read_json<T>(path: impl AsRef<Path>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.as_ref();
    if !path.exists() {
        return Err(Error::FileNotFound(path.display().to_string()));
    }

    let content = tokio::fs::read_to_string(path).await?;
    let data: T = serde_json::from_str(&content)?;
    Ok(data)
}
