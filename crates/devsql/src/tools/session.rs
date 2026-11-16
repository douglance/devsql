//! Exact, paginated native conversation reads without a global history scan.
use incurs::command::{CommandDef, TypedContext, TypedResult};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    path::Path,
};
#[derive(serde::Deserialize, incurs::Args)]
struct Args {
    id: String,
}
#[derive(serde::Deserialize, incurs::Options)]
struct Options {
    source: String,
    data_dir: Option<String>,
    cursor: Option<usize>,
    limit: Option<usize>,
    /// Read the latest N conversation records; incompatible with cursor.
    tail: Option<usize>,
}
fn reader(path: &Path) -> Result<Box<dyn BufRead>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if path.extension().and_then(|s| s.to_str()) == Some("zst") {
        Ok(Box::new(BufReader::new(
            zstd::stream::read::Decoder::new(file).map_err(|e| e.to_string())?,
        )))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}
fn relevant(source: &str, kind: &str) -> bool {
    if source == "claude" {
        matches!(kind, "user" | "assistant" | "summary")
    } else {
        matches!(kind, "response_item" | "event_msg" | "compacted")
    }
}
fn read(root: &Path, source: &str, id: &str, cursor: usize, limit: usize) -> Result<Value, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("invalid session id".into());
    }
    if !matches!(source, "claude" | "codex") || !(1..=500).contains(&limit) {
        return Err("source must be claude or codex; limit must be 1..500".into());
    }
    let bases = if source == "claude" {
        vec![root.join("projects")]
    } else {
        vec![root.join("sessions"), root.join("archived_sessions")]
    };
    let mut matches = Vec::new();
    for base in bases.into_iter().filter(|p| p.exists()) {
        for entry in walkdir::WalkDir::new(base) {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy();
            if entry.file_type().is_file()
                && (name == format!("{id}.jsonl")
                    || (source == "codex"
                        && (name.ends_with(&format!("-{id}.jsonl"))
                            || name.ends_with(&format!("-{id}.jsonl.zst")))))
            {
                matches.push(entry.path().to_owned());
            }
        }
    }
    if matches.len() != 1 {
        return Err(format!(
            "expected exactly one source conversation, found {}",
            matches.len()
        ));
    }
    let path = &matches[0];
    let mut messages = Vec::new();
    let mut page_bytes = 0;
    let mut next = None;
    for (index, line) in reader(path)?.lines().enumerate() {
        if index < cursor {
            continue;
        }
        let line = line.map_err(|e| e.to_string())?;
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => return Err(format!("invalid record {}: {e}", index)),
        };
        let kind = value["type"].as_str().unwrap_or("");
        if !relevant(source, kind) {
            continue;
        }
        if messages.len() == limit || (page_bytes + line.len() > 131_072 && !messages.is_empty()) {
            next = Some(index);
            break;
        }
        let content = if source == "claude" {
            value
                .get("message")
                .or(value.get("content"))
                .unwrap_or(&value)
                .clone()
        } else {
            value.get("payload").unwrap_or(&value).clone()
        };
        page_bytes += line.len();
        messages.push(json!({"record_index":index,"timestamp":value["timestamp"],"type":kind,"content":content}));
    }
    Ok(
        json!({"source":source,"session_id":id,"source_path":path,"messages":messages,"next_cursor":next}),
    )
}
/// Build the observational session retrieval command.
pub fn build() -> CommandDef {
    CommandDef::typed::<Args, Options, (), Value, _, _>(
        "session",
        |ctx: TypedContext<Args, Options, ()>| async move {
            let root = ctx
                .options
                .data_dir
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap()
                        .join(if ctx.options.source == "claude" {
                            ".claude"
                        } else {
                            ".codex"
                        })
                });
            if ctx.options.tail.is_some() && ctx.options.cursor.is_some() {
                return TypedResult::error(
                    "SESSION_CONTEXT_ERROR",
                    "tail and cursor are mutually exclusive",
                );
            }
            let cursor = if let Some(tail) = ctx.options.tail {
                if !(1..=500).contains(&tail) {
                    return TypedResult::error("SESSION_CONTEXT_ERROR", "tail must be 1..500");
                }
                // Resolve through the same exact-identity reader before opening its provenance path.
                match read(&root, &ctx.options.source, &ctx.args.id, 0, 1) {
                    Ok(first) => {
                        let path = first["source_path"].as_str().unwrap();
                        let mut offsets = VecDeque::new();
                        let lines = match reader(Path::new(path)) {
                            Ok(lines) => lines,
                            Err(e) => return TypedResult::error("SESSION_CONTEXT_ERROR", e),
                        };
                        for (index, line) in lines.lines().enumerate() {
                            let value: Value =
                                match line.map_err(|e| e.to_string()).and_then(|line| {
                                    serde_json::from_str(&line).map_err(|e| e.to_string())
                                }) {
                                    Ok(value) => value,
                                    Err(e) => {
                                        return TypedResult::error("SESSION_CONTEXT_ERROR", e)
                                    }
                                };
                            if relevant(&ctx.options.source, value["type"].as_str().unwrap_or("")) {
                                offsets.push_back(index);
                                if offsets.len() > tail {
                                    offsets.pop_front();
                                }
                            }
                        }
                        offsets.front().copied().unwrap_or(0)
                    }
                    Err(e) => return TypedResult::error("SESSION_CONTEXT_ERROR", e),
                }
            } else {
                ctx.options.cursor.unwrap_or(0)
            };
            match read(
                &root,
                &ctx.options.source,
                &ctx.args.id,
                cursor,
                ctx.options.limit.unwrap_or(100),
            ) {
                Ok(v) => TypedResult::ok(v),
                Err(e) => TypedResult::error("SESSION_CONTEXT_ERROR", e),
            }
        },
    )
    .description("Read one exact native conversation with provenance and pagination")
    .done()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_content_and_pagination() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("projects/p")).unwrap();
        std::fs::write(d.path().join("projects/p/abc.jsonl"),concat!(r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"keep this task"}]}}"#,"\n",r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"test","input":{"x":1}}]}}"#,"\n")).unwrap();
        let a = read(d.path(), "claude", "abc", 0, 1).unwrap();
        assert_eq!(
            a["messages"][0]["content"]["content"][0]["text"],
            "keep this task"
        );
        assert_eq!(a["next_cursor"], 1);
        let b = read(d.path(), "claude", "abc", 1, 1).unwrap();
        assert_eq!(b["messages"][0]["content"]["content"][0]["name"], "test");
        assert!(b["next_cursor"].is_null());
        assert!(read(d.path(), "claude", "../abc", 0, 1).is_err());
        assert!(read(d.path(), "claude", "missing", 0, 1).is_err());
    }
    #[test]
    fn compressed_codex_and_ambiguous_identity() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sessions/day")).unwrap();
        let path = d.path().join("sessions/day/rollout-date-abc.jsonl.zst");
        let mut encoder =
            zstd::stream::write::Encoder::new(std::fs::File::create(&path).unwrap(), 0).unwrap();
        encoder.write_all(b"{\"type\":\"response_item\",\"payload\":{\"role\":\"user\",\"content\":\"retained task\"}}\n").unwrap();
        encoder.finish().unwrap();
        let page = read(d.path(), "codex", "abc", 0, 1).unwrap();
        assert_eq!(page["messages"][0]["content"]["content"], "retained task");
        std::fs::write(d.path().join("sessions/rollout-other-abc.jsonl"), "{}\n").unwrap();
        assert!(read(d.path(), "codex", "abc", 0, 1)
            .unwrap_err()
            .contains("found 2"));
    }
    #[test]
    fn page_budget_keeps_whole_records_and_next_cursor() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("projects/p")).unwrap();
        let record = json!({"type":"user","message":"a".repeat(100_000)}).to_string() + "\n";
        std::fs::write(d.path().join("projects/p/abc.jsonl"), record.repeat(2)).unwrap();
        let page = read(d.path(), "claude", "abc", 0, 100).unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), 1);
        assert_eq!(page["next_cursor"], 1);
        assert_eq!(
            page["messages"][0]["content"].as_str().unwrap().len(),
            100_000
        );
    }
}
