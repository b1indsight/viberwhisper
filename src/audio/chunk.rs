use std::error::Error;
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

use hound::{WavSpec, WavWriter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavChunk {
    wav_bytes: Arc<[u8]>,
}

#[derive(Debug)]
pub enum ChunkError {
    InvalidSpec(&'static str),
    ArithmeticOverflow,
    NonFiniteSample,
    UnexpectedEndOfFile,
    Wav(hound::Error),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(field) => write!(f, "invalid WAV spec: {field} must be non-zero"),
            Self::ArithmeticOverflow => write!(f, "chunk capacity arithmetic overflow"),
            Self::NonFiniteSample => write!(f, "WAV contains a non-finite sample"),
            Self::UnexpectedEndOfFile => write!(f, "WAV input ended before its declared length"),
            Self::Wav(error) => write!(f, "WAV error: {error}"),
        }
    }
}

impl Error for ChunkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Wav(error) => Some(error),
            Self::InvalidSpec(_)
            | Self::ArithmeticOverflow
            | Self::NonFiniteSample
            | Self::UnexpectedEndOfFile => None,
        }
    }
}

impl From<hound::Error> for ChunkError {
    fn from(error: hound::Error) -> Self {
        Self::Wav(error)
    }
}

impl WavChunk {
    pub(crate) fn from_encoded_bytes(wav_bytes: Vec<u8>) -> Self {
        Self {
            wav_bytes: Arc::from(wav_bytes),
        }
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.wav_bytes
    }

    pub(crate) fn len(&self) -> usize {
        self.wav_bytes.len()
    }

    pub(crate) fn shared_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.wav_bytes)
    }
}

pub(crate) fn max_frames_per_chunk(
    max_duration_secs: u32,
    max_size_bytes: u64,
    output_spec: WavSpec,
) -> Result<Option<u64>, ChunkError> {
    validate_spec(output_spec)?;

    let duration_capacity = (max_duration_secs > 0)
        .then(|| u64::from(max_duration_secs) * u64::from(output_spec.sample_rate));
    let size_capacity = if max_size_bytes == 0 {
        None
    } else {
        let header_bytes = encoded_header_bytes(output_spec);
        let bytes_per_sample = u64::from(output_spec.bits_per_sample).div_ceil(8);
        let bytes_per_frame = u64::from(output_spec.channels)
            .checked_mul(bytes_per_sample)
            .ok_or(ChunkError::ArithmeticOverflow)?;
        let frames = max_size_bytes
            .checked_sub(header_bytes)
            .map(|payload_bytes| payload_bytes / bytes_per_frame)
            .unwrap_or(0);

        if frames.saturating_mul(2) <= u64::from(output_spec.sample_rate) {
            return Ok(Some(0));
        }
        Some(frames)
    };

    Ok(match (duration_capacity, size_capacity) {
        (Some(duration), Some(size)) => Some(duration.min(size)),
        (Some(duration), None) => Some(duration),
        (None, Some(size)) => Some(size),
        (None, None) => None,
    })
}

pub(crate) fn encode_i16_wav(samples: &[i16], sample_rate: u32) -> Result<WavChunk, ChunkError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    validate_spec(spec)?;

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }
    Ok(WavChunk::from_encoded_bytes(cursor.into_inner()))
}

fn validate_spec(spec: WavSpec) -> Result<(), ChunkError> {
    if spec.channels == 0 {
        return Err(ChunkError::InvalidSpec("channels"));
    }
    if spec.sample_rate == 0 {
        return Err(ChunkError::InvalidSpec("sample_rate"));
    }
    if spec.bits_per_sample == 0 {
        return Err(ChunkError::InvalidSpec("bits_per_sample"));
    }
    Ok(())
}

fn encoded_header_bytes(spec: WavSpec) -> u64 {
    if spec.channels > 2 || spec.bits_per_sample > 16 {
        68
    } else {
        44
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::SampleFormat;

    fn spec(channels: u16, sample_rate: u32, bits_per_sample: u16) -> WavSpec {
        WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format: SampleFormat::Int,
        }
    }

    #[test]
    fn disabled_limits_do_not_slice() {
        assert_eq!(
            max_frames_per_chunk(0, 0, spec(1, 16_000, 16)).unwrap(),
            None
        );
    }

    #[test]
    fn duration_and_size_limits_use_the_smaller_frame_capacity() {
        let one_second_mono_wav = 44 + 16_000 * 2;

        assert_eq!(
            max_frames_per_chunk(2, one_second_mono_wav, spec(1, 16_000, 16)).unwrap(),
            Some(16_000)
        );
    }

    #[test]
    fn size_capacity_of_half_a_second_or_less_produces_no_chunks() {
        let half_second_mono_wav = 44 + 8_000 * 2;

        assert_eq!(
            max_frames_per_chunk(0, half_second_mono_wav, spec(1, 16_000, 16)).unwrap(),
            Some(0)
        );
        assert_eq!(
            max_frames_per_chunk(0, half_second_mono_wav + 2, spec(1, 16_000, 16)).unwrap(),
            Some(8_001)
        );
    }

    #[test]
    fn size_limit_uses_the_header_emitted_for_extended_wav_specs() {
        let one_second_three_channel_wav = 68 + 48_000 * 3 * 2;

        assert_eq!(
            max_frames_per_chunk(0, one_second_three_channel_wav, spec(3, 48_000, 16)).unwrap(),
            Some(48_000)
        );
    }

    #[test]
    fn invalid_wav_spec_is_rejected() {
        assert!(max_frames_per_chunk(1, 0, spec(0, 16_000, 16)).is_err());
        assert!(max_frames_per_chunk(1, 0, spec(1, 0, 16)).is_err());
        assert!(max_frames_per_chunk(1, 0, spec(1, 16_000, 0)).is_err());
    }

    #[test]
    fn recorder_samples_are_encoded_as_a_complete_wav_payload() {
        let chunk = encode_i16_wav(&[1, -2, 3], 16_000).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(chunk.bytes())).unwrap();

        assert_eq!(reader.spec(), spec(1, 16_000, 16));
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![1, -2, 3]
        );
    }
}
