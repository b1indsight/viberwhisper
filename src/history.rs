use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::input::typer::TextTyper;

pub(crate) const MAX_HISTORY_BYTES: usize = 5 * 1024 * 1024;
pub(crate) const RECENT_HISTORY_LIMIT: usize = 5;
const REVERSE_SCAN_BYTES: usize = 4096;

type Result<T> = anyhow::Result<T>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryRecord {
    text: String,
    metadata: HistoryMetadata,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryMetadata {
    created_at_unix_ms: u64,
}

impl HistoryRecord {
    fn now(text: &str) -> Result<Self> {
        let created_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        Ok(Self {
            text: text.to_string(),
            metadata: HistoryMetadata { created_at_unix_ms },
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryStore {
    path: PathBuf,
    max_bytes: usize,
}

impl HistoryStore {
    pub(crate) fn discover() -> Result<Self> {
        let directory = crate::platform::config_dir()
            .ok_or_else(|| std::io::Error::other("application history directory is unavailable"))?;
        Ok(Self {
            path: directory.join("history.jsonl"),
            max_bytes: MAX_HISTORY_BYTES,
        })
    }

    #[cfg(test)]
    pub(crate) fn at(path: PathBuf, max_bytes: usize) -> Self {
        Self { path, max_bytes }
    }

    pub(crate) fn load_recent(&self) -> Result<Vec<String>> {
        self.repair_tail()?;
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut end = file.metadata()?.len();

        let mut recent = Vec::with_capacity(RECENT_HISTORY_LIMIT);
        while recent.len() < RECENT_HISTORY_LIMIT {
            let Some((start, line, _)) = last_line(&mut file, end)? else {
                break;
            };
            end = start;
            if line.is_empty() {
                continue;
            }

            match serde_json::from_slice::<HistoryRecord>(&line) {
                Ok(record) => recent.push(record.text),
                Err(error) => {
                    tracing::warn!(path = %self.path.display(), %error, "Stopped at invalid older history record");
                    break;
                }
            }
        }
        Ok(recent)
    }

    pub(crate) fn append(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Err(std::io::Error::other("history entries must not be empty").into());
        }
        self.append_record(HistoryRecord::now(text)?)
    }

    fn append_record(&self, record: HistoryRecord) -> Result<()> {
        let encoded = encode_record(&record)?;
        if encoded.len() > self.max_bytes {
            return Err(std::io::Error::other(format!(
                "history record exceeds the {}-byte limit",
                self.max_bytes
            ))
            .into());
        }

        let parent = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("history path has no parent directory"))?;
        fs::create_dir_all(parent)?;
        let needs_separator = self.repair_tail()?;
        let current_size = fs::metadata(&self.path).map_or(0, |metadata| metadata.len() as usize);

        if current_size + usize::from(needs_separator) + encoded.len() <= self.max_bytes {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            if needs_separator {
                file.write_all(b"\n")?;
            }
            file.write_all(&encoded)?;
            file.sync_data()?;
            return Ok(());
        }

        self.compact_and_append(&encoded)
    }

    /// Validates only the final JSONL record and removes it when it is incomplete or ill-typed.
    fn repair_tail(&self) -> Result<bool> {
        let mut file = match OpenOptions::new().read(true).write(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let size = file.metadata()?.len();
        let Some((start, bytes, terminated)) = last_line(&mut file, size)? else {
            return Ok(false);
        };

        if let Err(error) = serde_json::from_slice::<HistoryRecord>(&bytes) {
            file.set_len(start)?;
            file.sync_data()?;
            tracing::warn!(path = %self.path.display(), %error, "Removed invalid trailing history record");
            return Ok(false);
        }
        Ok(!terminated)
    }

    fn compact_and_append(&self, encoded: &[u8]) -> Result<()> {
        let mut existing = fs::read(&self.path)?;
        if !existing.is_empty() && !existing.ends_with(b"\n") {
            existing.push(b'\n');
        }

        let budget = self.max_bytes - encoded.len();
        let mut start = existing.len();
        while start > 0 {
            let previous = existing[..start - 1]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            if existing.len() - previous > budget {
                break;
            }
            start = previous;
        }

        let parent = self.path.parent().expect("history parent was validated");
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary
            .write_all(&existing[start..])
            .and_then(|()| temporary.write_all(encoded))?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path)?;
        Ok(())
    }
}

fn last_line(file: &mut File, original_end: u64) -> std::io::Result<Option<(u64, Vec<u8>, bool)>> {
    if original_end == 0 {
        return Ok(None);
    }

    let mut end = original_end;
    let mut byte = [0];
    file.seek(SeekFrom::Start(end - 1))?;
    file.read_exact(&mut byte)?;
    let terminated = byte[0] == b'\n';
    if terminated {
        end -= 1;
    }

    let mut cursor = end;
    let mut buffer = [0; REVERSE_SCAN_BYTES];
    let start = loop {
        if cursor == 0 {
            break 0;
        }
        let chunk_start = cursor.saturating_sub(REVERSE_SCAN_BYTES as u64);
        let length = (cursor - chunk_start) as usize;
        file.seek(SeekFrom::Start(chunk_start))?;
        file.read_exact(&mut buffer[..length])?;
        if let Some(index) = buffer[..length].iter().rposition(|byte| *byte == b'\n') {
            break chunk_start + index as u64 + 1;
        }
        cursor = chunk_start;
    };

    let mut bytes = vec![0; (end - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut bytes)?;
    Ok(Some((start, bytes, terminated)))
}

fn encode_record(record: &HistoryRecord) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) struct HistoryTyper {
    store: HistoryStore,
    inner: Arc<dyn TextTyper>,
    saved: Box<dyn Fn(String) + Send + Sync>,
}

impl HistoryTyper {
    pub(crate) fn new(
        store: HistoryStore,
        inner: Arc<dyn TextTyper>,
        saved: impl Fn(String) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            inner,
            saved: Box::new(saved),
        }
    }
}

impl TextTyper for HistoryTyper {
    fn type_text(&self, text: &str) -> anyhow::Result<()> {
        match self.store.append(text) {
            Ok(()) => (self.saved)(text.to_string()),
            Err(error) => tracing::error!(%error, "Failed to persist transcription history"),
        }
        self.inner.type_text(text)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    fn record(text: &str, created_at_unix_ms: u64) -> HistoryRecord {
        HistoryRecord {
            text: text.to_string(),
            metadata: HistoryMetadata { created_at_unix_ms },
        }
    }

    #[test]
    fn appends_jsonl_and_loads_only_the_five_newest_entries() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        let store = HistoryStore::at(path.clone(), 4096);

        let mut first = encode_record(&record("entry 1\n中文", 1)).unwrap();
        first.pop();
        fs::write(&path, first).unwrap();
        for index in 2..=7 {
            store
                .append_record(record(&format!("entry {index}\n中文"), index))
                .unwrap();
        }

        assert_eq!(
            store.load_recent().unwrap(),
            (3..=7)
                .rev()
                .map(|index| format!("entry {index}\n中文"))
                .collect::<Vec<_>>()
        );
        let last = fs::read_to_string(path).unwrap();
        let last: HistoryRecord = serde_json::from_str(last.lines().last().unwrap()).unwrap();
        assert_eq!(last.metadata.created_at_unix_ms, 7);
    }

    #[test]
    fn repairs_only_invalid_trailing_json_or_metadata() {
        for invalid in [
            b"{\"text\":\"partial".as_slice(),
            b"{\"text\":\"bad\",\"metadata\":{}}\n".as_slice(),
        ] {
            let directory = tempdir().unwrap();
            let path = directory.path().join("history.jsonl");
            let store = HistoryStore::at(path.clone(), 4096);
            let mut bytes = b"older invalid record\n".to_vec();
            bytes.extend(encode_record(&record("valid tail", 1)).unwrap());
            bytes.extend_from_slice(invalid);
            fs::write(&path, bytes).unwrap();

            store.append_record(record("next", 2)).unwrap();

            assert_eq!(store.load_recent().unwrap(), ["next", "valid tail"]);
            assert!(
                fs::read_to_string(path)
                    .unwrap()
                    .starts_with("older invalid record\n")
            );
        }
    }

    #[test]
    fn compaction_keeps_newest_complete_records_and_rejects_oversize_entries() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        let record_size = encode_record(&record(&"x".repeat(10), 1)).unwrap().len();
        let store = HistoryStore::at(path.clone(), record_size * 2);
        store.append_record(record(&"a".repeat(10), 1)).unwrap();
        store.append_record(record(&"b".repeat(10), 2)).unwrap();
        store.append_record(record(&"c".repeat(10), 3)).unwrap();

        assert_eq!(
            store.load_recent().unwrap(),
            ["c".repeat(10), "b".repeat(10)]
        );
        let before = fs::read(&path).unwrap();
        assert!(
            store
                .append_record(record(&"x".repeat(record_size * 2), 4))
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    struct RecordingTyper(Mutex<Vec<String>>);

    impl TextTyper for RecordingTyper {
        fn type_text(&self, text: &str) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn history_typer_saves_before_delivery() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        let inner = Arc::new(RecordingTyper(Mutex::new(Vec::new())));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&saved);
        let delivered = Arc::clone(&inner);
        let typer = HistoryTyper::new(HistoryStore::at(path, 1024), inner.clone(), move |text| {
            assert!(delivered.0.lock().unwrap().is_empty());
            captured.lock().unwrap().push(text);
        });

        typer.type_text("saved").unwrap();

        assert_eq!(*saved.lock().unwrap(), ["saved"]);
        assert_eq!(*inner.0.lock().unwrap(), ["saved"]);
    }
}
