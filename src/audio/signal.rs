use std::io::Cursor;

use hound::{SampleFormat, WavReader};

use super::chunk::{ChunkError, WavChunk};

const ANALYSIS_WINDOW_MS: u64 = 20;
const MIN_ACTIVE_DURATION_MS: u64 = 100;
// RMS -50 dBFS expressed as mean square: 10 ^ (-50 / 10).
const ACTIVE_WINDOW_POWER: f64 = 0.000_01;

pub(crate) fn contains_sustained_signal(chunk: &WavChunk) -> Result<bool, ChunkError> {
    let mut reader = WavReader::new(Cursor::new(chunk.shared_bytes()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(ChunkError::InvalidSpec("channels"));
    }
    if spec.sample_rate == 0 {
        return Err(ChunkError::InvalidSpec("sample_rate"));
    }
    if spec.bits_per_sample == 0 {
        return Err(ChunkError::InvalidSpec("bits_per_sample"));
    }

    let channels = usize::from(spec.channels);
    let window_frames = duration_frames(spec.sample_rate, ANALYSIS_WINDOW_MS)?;
    let required_active_frames = duration_frames(spec.sample_rate, MIN_ACTIVE_DURATION_MS)?;

    match spec.sample_format {
        SampleFormat::Int => {
            let full_scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            scan_normalized_samples(
                reader.samples::<i32>().map(|sample| {
                    sample
                        .map(|value| f64::from(value) / full_scale)
                        .map_err(ChunkError::from)
                }),
                channels,
                window_frames,
                required_active_frames,
            )
        }
        SampleFormat::Float => scan_normalized_samples(
            reader
                .samples::<f32>()
                .map(|sample| sample.map(f64::from).map_err(ChunkError::from)),
            channels,
            window_frames,
            required_active_frames,
        ),
    }
}

fn duration_frames(sample_rate: u32, duration_ms: u64) -> Result<usize, ChunkError> {
    let frames = u64::from(sample_rate)
        .checked_mul(duration_ms)
        .ok_or(ChunkError::ArithmeticOverflow)?
        .div_ceil(1_000)
        .max(1);
    usize::try_from(frames).map_err(|_| ChunkError::ArithmeticOverflow)
}

fn scan_normalized_samples(
    samples: impl Iterator<Item = Result<f64, ChunkError>>,
    channels: usize,
    window_frames: usize,
    required_active_frames: usize,
) -> Result<bool, ChunkError> {
    let window_samples = window_frames
        .checked_mul(channels)
        .ok_or(ChunkError::ArithmeticOverflow)?;
    let mut active_frames = 0usize;
    let mut power_sum = 0.0;
    let mut sample_count = 0usize;

    for sample in samples {
        let sample = sample?;
        if !sample.is_finite() {
            return Err(ChunkError::NonFiniteSample);
        }
        power_sum += sample * sample;
        sample_count += 1;

        if sample_count == window_samples {
            if window_is_active(power_sum, sample_count) {
                active_frames += window_frames;
                if active_frames >= required_active_frames {
                    return Ok(true);
                }
            }
            power_sum = 0.0;
            sample_count = 0;
        }
    }

    if !sample_count.is_multiple_of(channels) {
        return Err(ChunkError::UnexpectedEndOfFile);
    }
    if sample_count > 0 && window_is_active(power_sum, sample_count) {
        active_frames += sample_count / channels;
    }

    Ok(active_frames >= required_active_frames)
}

fn window_is_active(power_sum: f64, sample_count: usize) -> bool {
    power_sum / sample_count as f64 > ACTIVE_WINDOW_POWER
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use hound::{SampleFormat, WavSpec, WavWriter};

    use super::*;

    fn encode_i16(samples: &[i16], channels: u16, sample_rate: u32) -> WavChunk {
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(&mut cursor, spec).unwrap();
            for &sample in samples {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        WavChunk::from_encoded_bytes(cursor.into_inner())
    }

    fn encode_i24(samples: &[i32], sample_rate: u32) -> WavChunk {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 24,
            sample_format: SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(&mut cursor, spec).unwrap();
            for &sample in samples {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        WavChunk::from_encoded_bytes(cursor.into_inner())
    }

    fn encode_f32(samples: &[f32], sample_rate: u32) -> WavChunk {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(&mut cursor, spec).unwrap();
            for &sample in samples {
                writer.write_sample(sample).unwrap();
            }
            writer.finalize().unwrap();
        }
        WavChunk::from_encoded_bytes(cursor.into_inner())
    }

    #[test]
    fn silence_and_subthreshold_audio_are_suppressed() {
        let digital_silence = encode_i16(&vec![0; 200], 1, 1_000);
        let below_threshold = encode_i16(&vec![103; 200], 1, 1_000);

        assert!(!contains_sustained_signal(&digital_silence).unwrap());
        assert!(!contains_sustained_signal(&below_threshold).unwrap());
    }

    #[test]
    fn signal_must_reach_the_minimum_active_duration() {
        let too_short = encode_i16(&[104; 99], 1, 1_000);
        let long_enough = encode_i16(&[104; 100], 1, 1_000);

        assert!(!contains_sustained_signal(&too_short).unwrap());
        assert!(contains_sustained_signal(&long_enough).unwrap());
    }

    #[test]
    fn an_isolated_impulse_does_not_open_the_gate() {
        let mut samples = vec![0; 200];
        samples[0] = i16::MAX;

        assert!(!contains_sustained_signal(&encode_i16(&samples, 1, 1_000)).unwrap());
    }

    #[test]
    fn float_wide_integer_and_stereo_wavs_use_normalized_frame_duration() {
        let float = encode_f32(&vec![0.004; 4_800], 48_000);
        let wide_integer = encode_i24(&vec![30_000; 1_600], 16_000);
        let stereo = encode_i16(&[200, 0].repeat(100), 2, 1_000);

        assert!(contains_sustained_signal(&float).unwrap());
        assert!(contains_sustained_signal(&wide_integer).unwrap());
        assert!(contains_sustained_signal(&stereo).unwrap());
    }

    #[test]
    fn malformed_wav_is_not_classified_as_silence() {
        let malformed = WavChunk::from_encoded_bytes(b"not a wav".to_vec());

        assert!(contains_sustained_signal(&malformed).is_err());
    }
}
