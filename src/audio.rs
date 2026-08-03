use crate::core::config::AudioSection;

// Keep API payloads below common service limits while producing chunks often enough for live STT.
pub(crate) const MAX_CHUNK_DURATION_SECS: u32 = 30;
pub(crate) const MAX_CHUNK_SIZE_BYTES: u64 = 23 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    mic_gain: f32,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
}

impl AudioConfig {
    pub(crate) fn from_section(audio: &AudioSection) -> Self {
        Self {
            mic_gain: audio.mic_gain,
            max_chunk_duration_secs: MAX_CHUNK_DURATION_SECS,
            max_chunk_size_bytes: MAX_CHUNK_SIZE_BYTES,
        }
    }
}

pub mod chunk;
pub mod recorder;
pub mod wav_file;
pub use chunk::WavChunk;
pub(crate) use chunk::max_frames_per_chunk;
pub use recorder::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
pub use wav_file::WavChunkReader;
