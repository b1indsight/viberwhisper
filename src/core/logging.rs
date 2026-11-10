//! Process logging: bounded console verbosity and opt-in, file-only model diagnostics.

use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

#[cfg(feature = "diagnostic-logs")]
use {
    anyhow::Context,
    chrono::{Local, NaiveDate},
    std::{
        fs::{self, File, OpenOptions},
        io::{self, Write},
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    },
};

/// Initializes both process entry points, surfacing diagnostic path errors at startup.
pub(super) fn init() -> anyhow::Result<()> {
    let console = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_filter(filter_fn(|metadata| {
            metadata.target() != "viberwhisper::model" && *metadata.level() <= Level::INFO
        }))
        .with_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")),
        );
    let subscriber = tracing_subscriber::registry().with(console);
    #[cfg(feature = "diagnostic-logs")]
    let directory = super::config::config_dir()
        .context("configuration directory unavailable for diagnostic logging")?;
    #[cfg(feature = "diagnostic-logs")]
    let subscriber = {
        let writer =
            DailyWriter::new(directory.clone(), Local::now().date_naive()).with_context(|| {
                format!(
                    "failed to initialize diagnostic logging in {}",
                    directory.display()
                )
            })?;
        subscriber.with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .log_internal_errors(true)
                .with_writer(writer)
                .with_filter(filter_fn(|metadata| {
                    (metadata.target() == "viberwhisper"
                        || metadata.target().starts_with("viberwhisper::"))
                        && *metadata.level() <= Level::DEBUG
                })),
        )
    };
    subscriber.try_init()?;
    #[cfg(feature = "diagnostic-logs")]
    tracing::debug!(
        version = env!("CARGO_PKG_VERSION"),
        platform = std::env::consts::OS,
        pid = std::process::id(),
        log_directory = %directory.display(),
        "Diagnostic file logging enabled; files may contain sensitive text"
    );
    Ok(())
}

#[cfg(feature = "diagnostic-logs")]
pub(crate) fn next_request_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Logs only a copy of selected model fields; never pass audio, headers, or URLs.
#[cfg(feature = "diagnostic-logs")]
pub(crate) fn model_event(
    protocol: &str,
    request_id: impl std::fmt::Display,
    phase: &str,
    mut payload: serde_json::Value,
    key: Option<&str>,
) {
    fn redact(value: &mut serde_json::Value, key: &str) {
        match value {
            serde_json::Value::String(text) => *text = text.replace(key, "[REDACTED]"),
            serde_json::Value::Array(values) => {
                values.iter_mut().for_each(|value| redact(value, key))
            }
            serde_json::Value::Object(values) => {
                values.values_mut().for_each(|value| redact(value, key))
            }
            _ => {}
        }
    }
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        redact(&mut payload, key);
    }
    tracing::debug!(target: "viberwhisper::model", protocol, request_id = %request_id, phase,
        pid = std::process::id(), payload = %payload, "Model diagnostic");
}

/// Owns the one local-date file. A formatted tracing event is written under one lock.
#[cfg(feature = "diagnostic-logs")]
struct DailyWriter {
    directory: PathBuf,
    current: Mutex<Option<(NaiveDate, File)>>,
}

#[cfg(feature = "diagnostic-logs")]
impl DailyWriter {
    fn new(directory: PathBuf, today: NaiveDate) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        let writer = Self {
            directory,
            current: Mutex::new(None),
        };
        writer.write_on(|| today, &[])?;
        Ok(writer)
    }

    fn write_on(&self, date: impl FnOnce() -> NaiveDate, bytes: &[u8]) -> io::Result<usize> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| io::Error::other("diagnostic log lock poisoned"))?;
        // Read the clock under the lock so queued pre-midnight events cannot
        // roll the file backwards after a post-midnight writer has acquired it.
        let today = date();
        if current.as_ref().map(|(date, _)| *date) != Some(today) {
            // Drop the old handle before cleanup: Windows cannot remove an open file.
            *current = None;
            for entry in fs::read_dir(&self.directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(date) = name
                    .to_str()
                    .and_then(|name| name.strip_prefix("viberwhisper."))
                    .and_then(|name| name.strip_suffix(".jsonl"))
                else {
                    continue;
                };
                if let Ok(date_value) = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    && date_value.to_string() == date
                    && date_value != today
                    && entry.file_type()?.is_file()
                {
                    fs::remove_file(entry.path())?;
                }
            }
            let path = self.directory.join(format!("viberwhisper.{today}.jsonl"));
            if let Ok(metadata) = fs::symlink_metadata(&path)
                && !metadata.is_file()
            {
                return Err(io::Error::other(format!(
                    "diagnostic log is not a regular file: {}",
                    path.display()
                )));
            }
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            *current = Some((today, options.open(path)?));
        }
        current.as_mut().unwrap().1.write_all(bytes)?;
        Ok(bytes.len())
    }
}

#[cfg(feature = "diagnostic-logs")]
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyWriter {
    type Writer = &'a Self;
    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

#[cfg(feature = "diagnostic-logs")]
impl Write for &DailyWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_on(|| Local::now().date_naive(), bytes)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("diagnostic log in {}: {error}", self.directory.display()),
                )
            })
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some((_, file)) = self
            .current
            .lock()
            .map_err(|_| io::Error::other("diagnostic log lock poisoned"))?
            .as_mut()
        {
            file.flush()?;
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "diagnostic-logs"))]
pub(crate) mod test_support {
    /// Scoped capture for local HTTP/WebSocket fixtures, never a global subscriber.
    pub(crate) fn capture() -> (tempfile::NamedTempFile, tracing::Dispatch) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_env_filter("off,viberwhisper::model=debug")
            .with_writer(std::sync::Mutex::new(file.reopen().unwrap()))
            .finish();
        (file, tracing::Dispatch::new(subscriber))
    }

    pub(crate) fn model_events(file: &tempfile::NamedTempFile) -> Vec<serde_json::Value> {
        std::fs::read_to_string(file.path())
            .unwrap()
            .lines()
            .filter_map(|line| {
                let mut event: serde_json::Value = serde_json::from_str(line).unwrap();
                if event["target"] != "viberwhisper::model" {
                    return None;
                }
                let mut fields = event["fields"].take();
                fields["payload"] =
                    serde_json::from_str(fields["payload"].as_str().unwrap()).unwrap();
                Some(fields)
            })
            .collect()
    }
}

#[cfg(all(test, feature = "diagnostic-logs"))]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::fs;

    #[test]
    fn daily_log_keeps_only_local_today_and_preserves_other_app_data() {
        // Startup and midnight must remove yesterday's log, not the adjacent
        // config/history/memory or similarly named files in the shared directory.
        let directory = tempfile::tempdir().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let today = directory.path().join("viberwhisper.2026-11-10.jsonl");
        let yesterday = directory.path().join("viberwhisper.2026-11-09.jsonl");
        let future = directory.path().join("viberwhisper.2026-11-12.jsonl");
        fs::write(&today, b"existing\n").unwrap();
        fs::write(&yesterday, b"old\n").unwrap();
        fs::write(&future, b"future\n").unwrap();
        let preserved = [
            "config.json",
            "history.jsonl",
            "dictation-memory.json",
            "viberwhisper.notes.jsonl",
            "viberwhisper.2026-02-30.jsonl",
            "viberwhisper.2026-11-09.jsonl.backup",
            "other.2026-11-09.jsonl",
        ];
        for name in preserved {
            fs::write(directory.path().join(name), b"do not change").unwrap();
        }
        let writer = DailyWriter::new(directory.path().to_owned(), day).unwrap();
        writer.write_on(|| day, b"first\n").unwrap();
        assert_eq!(fs::read(&today).unwrap(), b"existing\nfirst\n");
        assert!(!yesterday.exists());
        assert!(!future.exists());
        drop(writer);
        let writer = DailyWriter::new(directory.path().to_owned(), day).unwrap();
        writer.write_on(|| day, b"restart\n").unwrap();
        assert_eq!(fs::read(&today).unwrap(), b"existing\nfirst\nrestart\n");
        writer
            .write_on(|| day.succ_opt().unwrap(), b"tomorrow\n")
            .unwrap();
        assert!(!today.exists());
        assert_eq!(
            fs::read(directory.path().join("viberwhisper.2026-11-11.jsonl")).unwrap(),
            b"tomorrow\n"
        );
        // A backwards clock change must not leave a future-dated diagnostic file.
        writer.write_on(|| day, b"clock changed\n").unwrap();
        assert_eq!(fs::read(&today).unwrap(), b"clock changed\n");
        assert!(
            !directory
                .path()
                .join("viberwhisper.2026-11-11.jsonl")
                .exists()
        );
        for name in preserved {
            assert_eq!(
                fs::read(directory.path().join(name)).unwrap(),
                b"do not change"
            );
        }
        let blocker = directory.path().join("not-a-directory");
        fs::write(&blocker, b"keep").unwrap();
        assert!(DailyWriter::new(blocker.clone(), day).is_err());
        assert_eq!(fs::read(blocker).unwrap(), b"keep");
    }

    #[test]
    fn concurrent_events_remain_complete_json_lines() {
        use std::sync::Arc;
        let directory = tempfile::tempdir().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let writer = Arc::new(DailyWriter::new(directory.path().to_owned(), day).unwrap());
        let threads: Vec<_> = (0..4)
            .map(|thread| {
                let writer = writer.clone();
                std::thread::spawn(move || {
                    for item in 0..20 {
                        let line = format!("{{\"thread\":{thread},\"item\":{item}}}\n");
                        writer.write_on(|| day, line.as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let contents =
            fs::read_to_string(directory.path().join("viberwhisper.2026-11-10.jsonl")).unwrap();
        let events: std::collections::HashSet<_> = contents
            .lines()
            .map(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                (
                    event["thread"].as_u64().unwrap(),
                    event["item"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(events.len(), 80);
    }
}
