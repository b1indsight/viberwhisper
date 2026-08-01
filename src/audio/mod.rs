use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::config::{AudioSection, ChunkingSection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLimits {
    max_duration_secs: u32,
    max_size_bytes: u64,
}

impl ChunkLimits {
    pub(crate) fn from_section(section: &ChunkingSection) -> Self {
        Self {
            max_duration_secs: section.max_duration_secs,
            max_size_bytes: section.max_size_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    mic_gain: f32,
    chunk_limits: ChunkLimits,
}

impl AudioConfig {
    pub(crate) fn from_sections(audio: &AudioSection, chunking: &ChunkingSection) -> Self {
        Self {
            mic_gain: audio.mic_gain,
            chunk_limits: ChunkLimits::from_section(chunking),
        }
    }
}

pub mod recorder;
pub mod splitter;
pub use recorder::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
pub use splitter::split_wav;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_temp_wav_path(prefix: &str) -> Result<PathBuf, std::time::SystemTimeError> {
    unique_temp_wav_path_in(Path::new("./tmp"), prefix)
}

fn unique_temp_wav_path_in(
    dir: &Path,
    prefix: &str,
) -> Result<PathBuf, std::time::SystemTimeError> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!(
        "{prefix}_{}_{}_{}.wav",
        std::process::id(),
        timestamp,
        sequence
    )))
}

fn unique_temp_session_dir(session_id: u64) -> Result<PathBuf, std::time::SystemTimeError> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(PathBuf::from(format!(
        "./tmp/viberwhisper-session-{session_id}-{}-{timestamp}-{sequence}",
        std::process::id()
    )))
}

/// Remove a temporary audio file and its session directory once it becomes empty.
pub(crate) fn remove_temp_audio_file(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    let result = std::fs::remove_file(path);

    if let Some(parent) = path.parent() {
        let is_session_dir = parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("viberwhisper-session-"));
        let is_empty = is_session_dir
            && std::fs::read_dir(parent)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
        if is_empty {
            let _ = std::fs::remove_dir(parent);
        }
    }

    result
}
