use crate::core::config::{ConfigDocument, fields};

// Keep API payloads below common service limits while producing chunks often enough for live STT.
const MAX_CHUNK_DURATION_SECS: u32 = 30;
const MAX_CHUNK_SIZE_BYTES: u64 = 23 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    input_device: Option<String>,
    mic_gain: f32,
}

impl AudioConfig {
    /// Selects the microphone device and gain settings.
    pub(crate) fn from_config(document: &ConfigDocument) -> Self {
        document.select(
            (fields::AudioInputDevice, fields::AudioMicGain),
            |(input_device, mic_gain)| Self {
                input_device,
                mic_gain,
            },
        )
    }
}

pub mod chunk;
pub mod recorder;
mod signal;
pub mod wav_file;
pub use chunk::WavChunk;
pub(crate) use chunk::max_frames_per_chunk;
pub use recorder::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
pub(crate) use signal::contains_audible_window;
pub use wav_file::WavChunkReader;
