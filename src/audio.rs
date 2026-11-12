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
pub(crate) mod segment;
mod signal;
mod vad;
pub mod wav_file;
pub use chunk::WavChunk;
pub(crate) use chunk::max_frames_per_chunk;
pub(crate) use preprocess::contains_speech_for_dictation;
pub(crate) use preprocess::prepare_for_transcription;
pub(crate) use recorder::replay_wav;
pub use recorder::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
pub(crate) use signal::contains_audible_window;
pub use wav_file::WavChunkReader;

/// A recording owns one resampler, including filter delay and the final partial input block.
pub(crate) struct PcmResampler {
    rate: u32,
    output_rate: u32,
    filter: Option<FftFixedInOut<f32>>,
    pending: VecDeque<f32>,
    previous_input: Vec<f32>,
    skip: usize,
    input_count: usize,
    output_count: usize,
}

impl PcmResampler {
    pub fn new(rate: u32, output_rate: u32) -> Result<Self> {
        ensure!(
            (8_000..=192_000).contains(&rate) && (8_000..=192_000).contains(&output_rate),
            "unsupported PCM sample rate"
        );
        let filter = if rate == output_rate {
            None
        } else {
            Some(FftFixedInOut::new(
                rate as usize,
                output_rate as usize,
                1024,
                1,
            )?)
        };
        let skip = filter.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            rate,
            output_rate,
            filter,
            pending: VecDeque::new(),
            previous_input: Vec::new(),
            skip,
            input_count: 0,
            output_count: 0,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Flush a boundary's analysis tail without inserting silence into the live stream.
    /// rubato's fixed FFT overlap depends only on the last input block, so replaying
    /// that block restores the filter without cloning private state or retaining audio history.
    pub(super) fn preview_finish(&self, extra_source: &[i16]) -> Result<Vec<i16>> {
        if self.filter.is_none() {
            return Ok(extra_source.to_vec());
        }
        let mut preview = Self::new(self.rate, self.output_rate)?;
        if !self.previous_input.is_empty() {
            preview
                .filter
                .as_mut()
                .unwrap()
                .process(&[&self.previous_input], None)?;
        }
        preview.pending.clone_from(&self.pending);
        preview.skip = self.skip;
        preview.input_count = self.input_count;
        preview.output_count = self.output_count;
        preview.push(extra_source, true)
    }

    pub fn push(&mut self, samples: &[i16], finish: bool) -> Result<Vec<i16>> {
        self.input_count += samples.len();
        let Some(filter) = &mut self.filter else {
            return Ok(samples.to_vec());
        };
        self.pending.extend(samples.iter().map(|s| f32::from(*s)));
        let expected = self.input_count * self.output_rate as usize / self.rate as usize;
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
                filter.process(&[&input], None)?
            } else {
                filter.process_partial(Some(&[&input]), None)?
            };
            self.previous_input.clone_from(&input);
            self.previous_input.resize(size, 0.0);
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
        for output_rate in [16_000, 24_000] {
            let whole = PcmResampler::new(44_100, output_rate)
                .unwrap()
                .push(&samples, true)
                .unwrap();
            let mut stream = PcmResampler::new(44_100, output_rate).unwrap();
            let mut parts = Vec::new();
            for frame in samples.chunks(317) {
                parts.extend(stream.push(frame, false).unwrap());
            }
            parts.extend(stream.push(&[], true).unwrap());
            assert_eq!(parts.len(), samples.len() * output_rate as usize / 44_100);
            assert_eq!(parts, whole);
        }
    }

    #[test]
    fn boundary_preview_matches_flush_without_changing_the_stream() {
        // A cut can leave both filter delay and an incomplete input block. Previewing
        // that tail must not inject the cut's synthetic silence into the next segment.
        let pcm: Vec<i16> = (0..12_000)
            .map(|i| ((i * 7919 % 60_001) - 30_000) as i16)
            .collect();
        for rate in [8_000, 16_000, 24_000, 44_100, 48_000, 192_000] {
            let size = PcmResampler::new(rate, 16_000)
                .unwrap()
                .filter
                .as_ref()
                .map_or(1024, Resampler::input_frames_next);
            for cut in [0, 1, size - 1, size, size + 1, size * 2 + 17] {
                for extra in [0, 1, 317, 1500] {
                    let mut live = PcmResampler::new(rate, 16_000).unwrap();
                    let mut terminal = PcmResampler::new(rate, 16_000).unwrap();
                    let mut uninterrupted = PcmResampler::new(rate, 16_000).unwrap();
                    live.push(&pcm[..cut], false).unwrap();
                    terminal.push(&pcm[..cut], false).unwrap();
                    uninterrupted.push(&pcm[..cut], false).unwrap();
                    let tail = &pcm[cut..cut + extra];
                    let expected = terminal.push(tail, true).unwrap();
                    assert_eq!(
                        live.preview_finish(tail).unwrap(),
                        expected,
                        "{rate}/{cut}/{extra}"
                    );
                    assert_eq!(live.preview_finish(tail).unwrap(), expected);
                    assert_eq!(
                        live.push(&pcm[cut..], true).unwrap(),
                        uninterrupted.push(&pcm[cut..], true).unwrap(),
                        "preview changed continuation at {rate}/{cut}"
                    );
                }
            }
        }
    }
}
