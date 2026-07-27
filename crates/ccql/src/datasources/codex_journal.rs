use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    Active,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexJournalFile {
    pub path: PathBuf,
    pub state: JournalState,
    pub compressed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexJournalRecord {
    pub record_index: i64,
    pub start_offset: u64,
    pub end_offset: u64,
    pub value: Option<Value>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalProgress {
    pub last_complete_offset: u64,
    pub last_record_index: i64,
}

pub fn discover_codex_journals(codex_home: &Path) -> io::Result<Vec<CodexJournalFile>> {
    let mut journals = Vec::new();
    for (directory, state) in [
        (codex_home.join("sessions"), JournalState::Active),
        (codex_home.join("archived_sessions"), JournalState::Archived),
    ] {
        if !directory.exists() {
            continue;
        }
        for entry in WalkDir::new(directory) {
            let entry = entry.map_err(io::Error::other)?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.into_path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let compressed = name.ends_with(".jsonl.zst");
            if !compressed && !name.ends_with(".jsonl") {
                continue;
            }
            journals.push(CodexJournalFile {
                path,
                state,
                compressed,
            });
        }
    }
    journals.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(journals)
}

pub fn visit_journal_records(
    file: &CodexJournalFile,
    start_offset: u64,
    start_record_index: i64,
    mut visitor: impl FnMut(CodexJournalRecord) -> io::Result<()>,
) -> io::Result<JournalProgress> {
    let mut reader = open_reader(file, start_offset)?;

    let mut offset = start_offset;
    let mut record_index = start_record_index;
    let mut last_complete_offset = start_offset;
    let mut last_record_index = start_record_index - 1;
    loop {
        let mut line = Vec::new();
        let bytes_read = reader.read_until(b'\n', &mut line)?;
        if bytes_read == 0 {
            break;
        }
        let end_offset = offset + bytes_read as u64;
        if line.last() != Some(&b'\n') {
            break;
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let parsed = serde_json::from_slice::<Value>(&line);
        let (value, parse_error) = match parsed {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error.to_string())),
        };
        visitor(CodexJournalRecord {
            record_index,
            start_offset: offset,
            end_offset,
            value,
            parse_error,
        })?;
        offset = end_offset;
        last_complete_offset = end_offset;
        last_record_index = record_index;
        record_index += 1;
    }

    Ok(JournalProgress {
        last_complete_offset,
        last_record_index,
    })
}

pub fn read_first_journal_record(
    file: &CodexJournalFile,
) -> io::Result<Option<CodexJournalRecord>> {
    let mut reader = open_reader(file, 0)?;
    let mut line = Vec::new();
    let bytes_read = reader.read_until(b'\n', &mut line)?;
    if bytes_read == 0 || line.last() != Some(&b'\n') {
        return Ok(None);
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let parsed = serde_json::from_slice::<Value>(&line);
    let (value, parse_error) = match parsed {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(Some(CodexJournalRecord {
        record_index: 0,
        start_offset: 0,
        end_offset: bytes_read as u64,
        value,
        parse_error,
    }))
}

fn open_reader(file: &CodexJournalFile, start_offset: u64) -> io::Result<Box<dyn BufRead>> {
    if file.compressed {
        if start_offset != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "compressed journals can only be read from offset 0",
            ));
        }
        let decoder = zstd::stream::read::Decoder::new(File::open(&file.path)?)?;
        Ok(Box::new(BufReader::new(decoder)))
    } else {
        let mut input = File::open(&file.path)?;
        input.seek(SeekFrom::Start(start_offset))?;
        Ok(Box::new(BufReader::new(input)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn discovers_active_plain_and_archived_compressed_journals() {
        let temp = tempfile::tempdir().expect("temp");
        let active = temp.path().join("sessions/2026/07/27/rollout-active.jsonl");
        let archived = temp
            .path()
            .join("archived_sessions/rollout-archived.jsonl.zst");
        write(&active, b"{}\n");
        write(&archived, b"compressed");
        write(&temp.path().join("sessions/ignore.txt"), b"ignore");

        let journals = discover_codex_journals(temp.path()).expect("discover");

        assert_eq!(
            journals,
            vec![
                CodexJournalFile {
                    path: archived,
                    state: JournalState::Archived,
                    compressed: true,
                },
                CodexJournalFile {
                    path: active,
                    state: JournalState::Active,
                    compressed: false,
                },
            ]
        );
    }

    #[test]
    fn plain_reader_ignores_an_incomplete_tail_until_it_gets_a_newline() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("sessions/rollout-tail.jsonl");
        let complete = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\"}}\n"
        );
        let contents = format!("{complete}{{\"type\":\"torn\"");
        write(&path, contents.as_bytes());
        let file = CodexJournalFile {
            path,
            state: JournalState::Active,
            compressed: false,
        };
        let mut records = Vec::new();

        let progress = visit_journal_records(&file, 0, 0, |record| {
            records.push(record);
            Ok(())
        })
        .expect("read");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record_index, 0);
        assert_eq!(records[1].record_index, 1);
        assert_eq!(progress.last_complete_offset, complete.len() as u64);
        assert_eq!(progress.last_record_index, 1);
    }

    #[test]
    fn compressed_reader_emits_the_same_records_as_plain_jsonl() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("archived_sessions/rollout-zstd.jsonl.zst");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let output = fs::File::create(&path).expect("create");
        let mut encoder = zstd::stream::write::Encoder::new(output, 0).expect("encoder");
        encoder
            .write_all(
                concat!(
                    "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-z\"}}\n",
                    "{\"type\":\"compacted\",\"payload\":{\"message\":\"summary\"}}\n"
                )
                .as_bytes(),
            )
            .expect("compress");
        encoder.finish().expect("finish");
        let file = CodexJournalFile {
            path,
            state: JournalState::Archived,
            compressed: true,
        };
        let mut types = Vec::new();

        let progress = visit_journal_records(&file, 0, 0, |record| {
            types.push(
                record.value.as_ref().unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
            Ok(())
        })
        .expect("read");

        assert_eq!(types, vec!["session_meta", "compacted"]);
        assert_eq!(progress.last_record_index, 1);
        assert!(progress.last_complete_offset > 0);
    }
}
