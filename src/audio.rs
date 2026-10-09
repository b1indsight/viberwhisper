use anyhow::{Result, ensure};
use rubato::{FftFixedInOut, Resampler};
use std::collections::VecDeque;

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
mod loudness;
mod preprocess;
pub mod recorder;
mod signal;
mod vad;
pub mod wav_file;
pub use chunk::WavChunk;
pub(crate) use chunk::max_frames_per_chunk;
pub(crate) use preprocess::contains_speech_for_dictation;
pub(crate) use preprocess::prepare_for_transcription;
pub use recorder::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
pub(crate) use signal::contains_audible_window;
pub use wav_file::WavChunkReader;

/// A recording owns one resampler, including filter delay and the final partial input block.
pub(crate) struct PcmResampler {
    rate: u32,
    filter: Option<FftFixedInOut<f32>>,
    pending: VecDeque<f32>,
    skip: usize,
    input_count: usize,
    output_count: usize,
}

impl PcmResampler {
    pub fn new(rate: u32) -> Result<Self> {
        ensure!(
            (8_000..=192_000).contains(&rate),
            "unsupported dictation sample rate"
        );
        let filter = if rate == 24_000 {
            None
        } else {
            Some(FftFixedInOut::new(rate as usize, 24_000, 1024, 1)?)
        };
        let skip = filter.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            rate,
            filter,
            pending: VecDeque::new(),
            skip,
            input_count: 0,
            output_count: 0,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, samples: &[i16], finish: bool) -> Result<Vec<i16>> {
        self.input_count += samples.len();
        let Some(filter) = &mut self.filter else {
            return Ok(samples.to_vec());
        };
        self.pending.extend(samples.iter().map(|s| f32::from(*s)));
        let expected = self.input_count * 24_000 / self.rate as usize;
        let mut output = Vec::new();
        loop {
            let size = filter.input_frames_next();
            if self.pending.len() < size && !finish {
                break;
            }
            if finish && self.output_count >= expected {
                break;
            }
            let count = size.min(self.pending.len());
            let input: Vec<_> = self.pending.drain(..count).collect();
            let block = if count == 0 {
                filter.process_partial::<&[f32]>(None, None)?
            } else if count == size {
                filter.process(&[input], None)?
            } else {
                filter.process_partial(Some(&[input]), None)?
            };
            for sample in &block[0] {
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                if finish && self.output_count >= expected {
                    break;
                }
                output.push(sample.round().clamp(-32768.0, 32767.0) as i16);
                self.output_count += 1;
            }
        }
        Ok(output)
    }
}

/// Decode through the shared WAV validator, then adapt channels to mono PCM for streaming.
pub(crate) fn decode_mono(chunk: &WavChunk) -> Result<(u32, Vec<i16>)> {
    let decoded = preprocess::DecodedWav::read(chunk)?;
    let channels = usize::from(decoded.spec.channels);
    let samples = decoded
        .samples
        .chunks(channels)
        .map(|frame| {
            (frame.iter().sum::<f64>() / channels as f64 * 32768.0)
                .round()
                .clamp(-32768.0, 32767.0) as i16
        })
        .collect();
    Ok((decoded.spec.sample_rate, samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_boundaries_do_not_change_resampling_and_tail_is_not_lost() {
        let samples: Vec<i16> = (0..44_237)
            .map(|n| ((n as f32 * 0.06).sin() * 9000.0) as i16)
            .collect();
        let whole = PcmResampler::new(44_100)
            .unwrap()
            .push(&samples, true)
            .unwrap();
        let mut stream = PcmResampler::new(44_100).unwrap();
        let mut parts = Vec::new();
        for frame in samples.chunks(317) {
            parts.extend(stream.push(frame, false).unwrap());
        }
        parts.extend(stream.push(&[], true).unwrap());
        assert_eq!(parts.len(), samples.len() * 24_000 / 44_100);
        assert_eq!(parts, whole);
    }
}
