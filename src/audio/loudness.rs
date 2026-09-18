//! Constant gain per chunk, measured over energy-active windows with peak protection.

use std::io::Cursor;

use anyhow::{Result, ensure};
use hound::{SampleFormat, WavWriter};
use tracing::debug;

use super::signal::{ACTIVE_WINDOW_POWER, ANALYSIS_WINDOW_MS};
use super::{MAX_CHUNK_SIZE_BYTES, WavChunk, preprocess::DecodedWav};

impl DecodedWav {
    /// Adjusts only upload bytes, preserving format, channel balance and source ownership.
    pub(super) fn adjust(&self, original: &WavChunk) -> Result<WavChunk> {
        let window = usize::try_from(
            u64::from(self.spec.sample_rate)
                .saturating_mul(ANALYSIS_WINDOW_MS)
                .div_ceil(1_000)
                .saturating_mul(u64::from(self.spec.channels)),
        )?;
        let mut active_power = 0.0;
        let mut active_samples = 0;
        let mut peak = 0.0_f64;
        for samples in self.samples.chunks(window) {
            let power = samples.iter().map(|s| s * s).sum::<f64>();
            peak = samples.iter().fold(peak, |peak, s| peak.max(s.abs()));
            if power / samples.len() as f64 > ACTIVE_WINDOW_POWER {
                active_power += power;
                active_samples += samples.len();
            }
        }
        if active_samples == 0 || peak == 0.0 {
            return Ok(original.clone());
        }
        let rms = (active_power / active_samples as f64).sqrt();
        let ceiling = 10_f64.powf(-1.0 / 20.0);
        let gain = (0.1 / rms)
            .max(1.0)
            .min(10_f64.powf(12.0 / 20.0))
            .min(ceiling / peak);
        if gain == 1.0 {
            return Ok(original.clone());
        }
        let mut cursor = Cursor::new(Vec::with_capacity(original.len()));
        let mut writer = WavWriter::new(&mut cursor, self.spec)?;
        match self.spec.sample_format {
            SampleFormat::Int => {
                let scale = 2_f64.powi(i32::from(self.spec.bits_per_sample) - 1);
                // Truncate toward zero so quantization cannot cross the peak ceiling.
                for sample in &self.samples {
                    writer.write_sample((sample * gain * scale) as i32)?;
                }
            }
            SampleFormat::Float => {
                // Leave one f32 epsilon of headroom for conversion rounding.
                let limit = (ceiling as f32) - f32::EPSILON;
                for sample in &self.samples {
                    writer.write_sample(((sample * gain) as f32).clamp(-limit, limit))?;
                }
            }
        }
        writer.finalize()?;
        let bytes = cursor.into_inner();
        ensure!(
            bytes.len() as u64 <= MAX_CHUNK_SIZE_BYTES,
            "adjusted WAV exceeds chunk byte limit"
        );
        debug!(gain_db = 20.0 * gain.log10(), "Adjusted upload loudness");
        Ok(WavChunk::from_encoded_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunk::encode_i16_wav;

    #[test]
    fn pauses_do_not_change_gain_and_original_bytes_are_immutable() {
        let speech = vec![1_000; 1_600];
        let original = encode_i16_wav(&speech, 16_000).unwrap();
        let bytes = original.shared_bytes();
        let short = DecodedWav::read(&original)
            .unwrap()
            .adjust(&original)
            .unwrap();
        let mut padded = speech;
        padded.resize(16_000, 0);
        let long = encode_i16_wav(&padded, 16_000).unwrap();
        let long = DecodedWav::read(&long).unwrap().adjust(&long).unwrap();
        let short_pcm = DecodedWav::read(&short).unwrap();
        let long_pcm = DecodedWav::read(&long).unwrap();
        assert!((short_pcm.samples[0] - 0.1).abs() < 0.0001);
        assert_eq!(short_pcm.samples, long_pcm.samples[..1_600]);
        assert!(long_pcm.samples[1_600..].iter().all(|x| *x == 0.0));
        assert_eq!(original.shared_bytes(), bytes);
    }

    #[test]
    fn boost_is_bounded_and_peaks_never_clip() {
        let quiet = encode_i16_wav(&vec![200; 800], 16_000).unwrap();
        let adjusted = DecodedWav::read(&quiet).unwrap().adjust(&quiet).unwrap();
        let level = DecodedWav::read(&adjusted).unwrap().samples[0];
        assert!((level / (200.0 / 32768.0) - 10_f64.powf(12.0 / 20.0)).abs() < 0.01);
        let mut samples = vec![200; 800];
        samples[0] = i16::MIN;
        let peaked = encode_i16_wav(&samples, 16_000).unwrap();
        let adjusted = DecodedWav::read(&peaked).unwrap().adjust(&peaked).unwrap();
        assert!(
            DecodedWav::read(&adjusted)
                .unwrap()
                .samples
                .iter()
                .all(|x| x.abs() <= 10_f64.powf(-1.0 / 20.0))
        );
    }

    #[test]
    fn silence_and_already_suitable_audio_keep_original_bytes() {
        for value in [0, 6_000] {
            let original = encode_i16_wav(&vec![value; 800], 16_000).unwrap();
            let adjusted = DecodedWav::read(&original)
                .unwrap()
                .adjust(&original)
                .unwrap();
            assert!(std::sync::Arc::ptr_eq(
                &original.shared_bytes(),
                &adjusted.shared_bytes()
            ));
        }
    }

    #[test]
    fn float_stereo_retains_format_frames_and_channel_balance() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for _ in 0..2_400 {
            writer.write_sample(0.02_f32).unwrap();
            writer.write_sample(-0.01_f32).unwrap();
        }
        writer.finalize().unwrap();
        let original = WavChunk::from_encoded_bytes(cursor.into_inner());
        let adjusted = DecodedWav::read(&original)
            .unwrap()
            .adjust(&original)
            .unwrap();
        let pcm = DecodedWav::read(&adjusted).unwrap();
        assert_eq!(pcm.spec, spec);
        assert_eq!(pcm.samples.len(), 4_800);
        assert!((pcm.samples[0] / pcm.samples[1] + 2.0).abs() < 1e-6);
    }

    #[test]
    fn malformed_and_nonfinite_audio_cannot_be_adjusted() {
        assert!(DecodedWav::read(&WavChunk::from_encoded_bytes(b"bad wav".to_vec())).is_err());
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        writer.write_sample(f32::NAN).unwrap();
        writer.finalize().unwrap();
        assert!(DecodedWav::read(&WavChunk::from_encoded_bytes(cursor.into_inner())).is_err());
    }

    #[test]
    fn offline_integer_depths_preserve_format_and_quantized_peak_bounds() {
        for bits in [8, 24, 32] {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 8_000,
                bits_per_sample: bits,
                sample_format: hound::SampleFormat::Int,
            };
            let mut cursor = Cursor::new(Vec::new());
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            let scale = 2_f64.powi(i32::from(bits) - 1);
            for _ in 0..400 {
                writer.write_sample((scale * 0.04) as i32).unwrap();
            }
            writer.finalize().unwrap();
            let original = WavChunk::from_encoded_bytes(cursor.into_inner());
            let adjusted = DecodedWav::read(&original)
                .unwrap()
                .adjust(&original)
                .unwrap();
            let pcm = DecodedWav::read(&adjusted).unwrap();
            assert_eq!(pcm.spec, spec);
            assert_eq!(pcm.samples.len(), 400);
            assert!((pcm.samples[0] - 0.1).abs() <= 1.0 / scale);
        }
    }
}
