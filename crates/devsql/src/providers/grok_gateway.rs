//! Drive the `grokctl` binary to read full Grok Bot history from the gateway.
//!
//! devsql cannot link `grokctl-core`: both crates declare `links = "sqlite3"`
//! against different `libsqlite3-sys` majors, so Cargo refuses the graph. The
//! binary is therefore the interface.
//!
//! Only commands `grokctl` classifies as `effect: read` are used —
//! `bot list` and `bot transcript-tail`. The generic `gateway call` escape
//! hatch is classified destructive and open-world, and is deliberately avoided.

use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Cap on a single `grokctl` response, so a pathological page cannot exhaust
/// memory.
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum GatewayError {
    /// grokctl is not installed. Not an error condition for callers: local
    /// replicas still work, exactly as a missing shell history does.
    BinaryNotFound(String),
    Spawn(String),
    Timeout(String),
    Failed {
        status: String,
        stderr: String,
    },
    Parse(String),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BinaryNotFound(path) => write!(f, "grokctl not found (looked for {path})"),
            Self::Spawn(message) => write!(f, "could not run grokctl: {message}"),
            Self::Timeout(command) => write!(f, "grokctl {command} timed out"),
            Self::Failed { status, stderr } => {
                let detail = stderr.trim();
                if detail.is_empty() {
                    write!(f, "grokctl exited with {status}")
                } else {
                    write!(f, "grokctl exited with {status}: {detail}")
                }
            }
            Self::Parse(message) => write!(f, "could not parse grokctl output: {message}"),
        }
    }
}

pub struct GrokctlClient {
    binary: PathBuf,
    timeout: Duration,
}

impl GrokctlClient {
    /// Resolve the binary. An explicit path is used verbatim; otherwise try
    /// `DEVSQL_GROKCTL_BIN`, then `PATH`, then the default cargo install
    /// location.
    pub fn discover(explicit: Option<&str>, timeout: Duration) -> Result<Self, GatewayError> {
        // An explicit path is a choice, not a hint: never silently fall back to
        // some other grokctl the caller did not name.
        let candidates: Vec<PathBuf> = match explicit {
            Some(path) => vec![PathBuf::from(path)],
            None => std::env::var_os("DEVSQL_GROKCTL_BIN")
                .map(PathBuf::from)
                .into_iter()
                .chain(std::iter::once(PathBuf::from("grokctl")))
                .chain(dirs::home_dir().map(|home| home.join(".cargo").join("bin").join("grokctl")))
                .collect(),
        };

        for candidate in &candidates {
            // A bare name is resolved by the OS against PATH; anything else has
            // to exist on disk.
            if candidate.components().count() == 1 || candidate.exists() {
                let client = Self {
                    binary: candidate.clone(),
                    timeout,
                };
                if client.is_runnable() {
                    return Ok(client);
                }
            }
        }

        Err(GatewayError::BinaryNotFound(
            candidates
                .first()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "grokctl".to_string()),
        ))
    }

    fn is_runnable(&self) -> bool {
        Command::new(&self.binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// `grokctl bot list` returns a bare array with no result envelope.
    pub fn list_bots(&self) -> Result<Vec<Value>, GatewayError> {
        let value = self.run(&["bot", "list", "--format", "json"])?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    /// One page of transcript, newest first.
    ///
    /// `before_seq` walks backward through full history; the returned
    /// `next_before_seq` is the cursor for the next page.
    pub fn transcript_page(
        &self,
        bot_id: &str,
        limit: i64,
        before_seq: Option<i64>,
    ) -> Result<TranscriptPage, GatewayError> {
        let mut body = serde_json::Map::new();
        body.insert("id".to_string(), Value::String(bot_id.to_string()));
        body.insert("limit".to_string(), Value::from(limit));
        if let Some(seq) = before_seq {
            body.insert("beforeSeq".to_string(), Value::from(seq));
        }
        let body = Value::Object(body).to_string();

        let value = self.run(&[
            "bot",
            "transcript-tail",
            "--body",
            &body,
            "--format",
            "json",
        ])?;

        // transcript-tail wraps its payload in a result envelope.
        let result = value.get("result").unwrap_or(&value);
        let entries = result
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next_before_seq = result.get("nextBeforeSeq").and_then(Value::as_i64);

        Ok(TranscriptPage {
            entries,
            next_before_seq,
        })
    }

    fn run(&self, args: &[&str]) -> Result<Value, GatewayError> {
        let stdout = self.capture(args)?;
        serde_json::from_slice::<Value>(&stdout).map_err(|error| {
            let preview: String = String::from_utf8_lossy(&stdout).chars().take(200).collect();
            GatewayError::Parse(format!("{error} (output began: {preview:?})"))
        })
    }

    fn capture(&self, args: &[&str]) -> Result<Vec<u8>, GatewayError> {
        let mut child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| GatewayError::Spawn(error.to_string()))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (sender, receiver) = sync_channel(2);

        let stdout_sender = sender.clone();
        let stdout_thread = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stream) = stdout {
                let _ = stream
                    .by_ref()
                    .take(MAX_OUTPUT_BYTES as u64)
                    .read_to_end(&mut buffer);
            }
            let _ = stdout_sender.send(Stream::Stdout(buffer));
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(mut stream) = stderr {
                let _ = stream
                    .by_ref()
                    .take(MAX_OUTPUT_BYTES as u64)
                    .read_to_end(&mut buffer);
            }
            let _ = sender.send(Stream::Stderr(buffer));
        });

        let deadline = Instant::now() + self.timeout;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut seen = 0;
        let mut timed_out = false;
        while seen < 2 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(Stream::Stdout(buffer)) => {
                    out = buffer;
                    seen += 1;
                }
                Ok(Stream::Stderr(buffer)) => {
                    err = buffer;
                    seen += 1;
                }
                Err(RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        if timed_out {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(GatewayError::Timeout(args.join(" ")));
        }

        let status = child
            .wait()
            .map_err(|error| GatewayError::Spawn(error.to_string()))?;
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();

        if !status.success() {
            return Err(GatewayError::Failed {
                status: status.to_string(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        Ok(out)
    }
}

enum Stream {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct TranscriptPage {
    pub entries: Vec<Value>,
    pub next_before_seq: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_binary_is_reported_not_panicked() {
        let error = GrokctlClient::discover(
            Some("/nonexistent/grokctl-does-not-exist"),
            Duration::from_secs(1),
        );
        assert!(matches!(error, Err(GatewayError::BinaryNotFound(_))));
    }
}
