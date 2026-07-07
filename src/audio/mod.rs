use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod recorder;
pub mod splitter;
pub use recorder::{AudioRecorder, StopResult};
#[allow(unused_imports)]
pub use splitter::{TmpChunk, split_wav};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Directory for transient recordings and chunk files.
///
/// Lives under the system temp directory rather than `./tmp` so the app also
/// works when launched from Finder/Explorer, where the working directory is
/// not the project directory (and may not even be writable).
pub(crate) fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("viberwhisper")
}

fn unique_temp_wav_path(prefix: &str) -> Result<PathBuf, std::time::SystemTimeError> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(temp_dir().join(format!(
        "{prefix}_{}_{}_{}.wav",
        std::process::id(),
        timestamp,
        sequence
    )))
}
