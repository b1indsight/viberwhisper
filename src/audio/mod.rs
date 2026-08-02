use crate::core::config::{AudioSection, ChunkingSection};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    mic_gain: f32,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
}

impl AudioConfig {
    pub(crate) fn from_sections(audio: &AudioSection, chunking: &ChunkingSection) -> Self {
        Self {
            mic_gain: audio.mic_gain,
            max_chunk_duration_secs: chunking.max_duration_secs,
            max_chunk_size_bytes: chunking.max_size_bytes,
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
