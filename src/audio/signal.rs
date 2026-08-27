use std::io::Cursor;

use hound::{SampleFormat, WavReader};

use super::chunk::WavChunk;

const ANALYSIS_WINDOW_MS: u64 = 50;
// RMS -50 dBFS expressed as mean square: 10 ^ (-50 / 10).
const ACTIVE_WINDOW_POWER: f64 = 0.000_01;

pub(crate) fn contains_audible_window(chunk: &WavChunk) -> Result<bool, hound::Error> {
    let mut reader = WavReader::new(Cursor::new(chunk.shared_bytes()))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 || spec.bits_per_sample == 0 {
        return Ok(true);
    }

    let window_samples = u64::from(spec.sample_rate)
        .saturating_mul(ANALYSIS_WINDOW_MS)
        .div_ceil(1_000)
        .max(1)
        .saturating_mul(u64::from(spec.channels));
    let Ok(window_samples) = usize::try_from(window_samples) else {
        return Ok(true);
    };

    match spec.sample_format {
        SampleFormat::Int => {
            let full_scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            any_active_window(
                reader
                    .samples::<i32>()
                    .map(|sample| sample.map(|value| f64::from(value) / full_scale)),
                window_samples,
            )
        }
        SampleFormat::Float => any_active_window(
            reader.samples::<f32>().map(|sample| sample.map(f64::from)),
            window_samples,
        ),
    }
}

fn any_active_window(
    samples: impl Iterator<Item = Result<f64, hound::Error>>,
    window_samples: usize,
) -> Result<bool, hound::Error> {
    let mut power_sum = 0.0;
    let mut sample_count = 0usize;

    for sample in samples {
        let sample = sample?;
        if !sample.is_finite() {
            return Ok(true);
        }
        power_sum += sample * sample;
        sample_count += 1;

        if sample_count == window_samples {
            if window_is_active(power_sum, sample_count) {
                return Ok(true);
            }
            power_sum = 0.0;
            sample_count = 0;
        }
    }

    Ok(sample_count > 0 && window_is_active(power_sum, sample_count))
}

fn window_is_active(power_sum: f64, sample_count: usize) -> bool {
    power_sum / sample_count as f64 > ACTIVE_WINDOW_POWER
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use hound::{SampleFormat, WavSpec, WavWriter};

    use super::*;
    use crate::audio::chunk::encode_i16_wav;

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
    fn silence_and_subthreshold_windows_are_suppressed() {
        let digital_silence = encode_i16_wav(&vec![0; 200], 1_000).unwrap();
        let below_threshold = encode_i16_wav(&vec![103; 200], 1_000).unwrap();

        assert!(!contains_audible_window(&digital_silence).unwrap());
        assert!(!contains_audible_window(&below_threshold).unwrap());
    }

    #[test]
    fn one_audible_fifty_millisecond_window_opens_the_gate() {
        let mut samples = vec![0; 150];
        samples[50..100].fill(104);
        let chunk = encode_i16_wav(&samples, 1_000).unwrap();

        assert!(contains_audible_window(&chunk).unwrap());
    }

    #[test]
    fn float_wav_uses_normalized_energy() {
        let chunk = encode_f32(&vec![0.004; 2_400], 48_000);

        assert!(contains_audible_window(&chunk).unwrap());
    }

    #[test]
    fn malformed_wav_is_not_classified_as_silence() {
        let malformed = WavChunk::from_encoded_bytes(b"not a wav".to_vec());

        assert!(contains_audible_window(&malformed).is_err());
    }
}
