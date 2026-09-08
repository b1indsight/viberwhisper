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
    UnexpectedEndOfFile,
    Wav(hound::Error),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(field) => write!(f, "invalid WAV spec: {field} must be non-zero"),
            Self::ArithmeticOverflow => write!(f, "chunk capacity arithmetic overflow"),
            Self::UnexpectedEndOfFile => write!(f, "WAV input ended before its declared length"),
            Self::Wav(error) => write!(f, "WAV error: {error}"),
        }
    }
}

impl Error for ChunkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Wav(error) => Some(error),
            Self::InvalidSpec(_) | Self::ArithmeticOverflow | Self::UnexpectedEndOfFile => None,
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

/// Applies the fixed duration/size policy; zero capacity suppresses chunk output.
pub(crate) fn max_frames_per_chunk(output_spec: WavSpec) -> Result<u64, ChunkError> {
    validate_spec(output_spec)?;

    let duration_capacity =
        u64::from(super::MAX_CHUNK_DURATION_SECS) * u64::from(output_spec.sample_rate);
    let bytes_per_sample = u64::from(output_spec.bits_per_sample).div_ceil(8);
    let bytes_per_frame = u64::from(output_spec.channels) * bytes_per_sample;
    let size_capacity = super::MAX_CHUNK_SIZE_BYTES
        .saturating_sub(encoded_header_bytes(output_spec))
        / bytes_per_frame;

    if size_capacity * 2 <= u64::from(output_spec.sample_rate) {
        return Ok(0);
    }
    Ok(duration_capacity.min(size_capacity))
}

pub(crate) fn encode_i16_wav(samples: &[i16], sample_rate: u32) -> Result<WavChunk, ChunkError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    validate_spec(spec)?;

    let mut cursor = Cursor::new(Vec::with_capacity(encoded_capacity(
        spec,
        samples.len() as u64,
    )?));
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

pub(super) fn encoded_capacity(spec: WavSpec, sample_count: u64) -> Result<usize, ChunkError> {
    let bytes_per_sample = u64::from(spec.bits_per_sample).div_ceil(8);
    let bytes = sample_count
        .checked_mul(bytes_per_sample)
        .and_then(|payload| payload.checked_add(encoded_header_bytes(spec)))
        .ok_or(ChunkError::ArithmeticOverflow)?;
    usize::try_from(bytes).map_err(|_| ChunkError::ArithmeticOverflow)
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
    fn ordinary_audio_uses_the_fixed_thirty_second_capacity() {
        assert_eq!(max_frames_per_chunk(spec(1, 16_000, 16)).unwrap(), 480_000);
    }

    #[test]
    fn duration_and_size_limits_use_the_smaller_frame_capacity() {
        assert_eq!(
            max_frames_per_chunk(spec(1, 1_000_000, 16)).unwrap(),
            (23 * 1024 * 1024 - 44) / 2
        );
    }

    #[test]
    fn size_capacity_of_half_a_second_or_less_produces_no_chunks() {
        // A dense multi-channel format can exhaust the fixed byte budget within half a second.
        assert_eq!(max_frames_per_chunk(spec(32, 753_660, 16)).unwrap(), 0);
        assert_eq!(
            max_frames_per_chunk(spec(32, 753_659, 16)).unwrap(),
            376_830
        );
    }

    #[test]
    fn size_limit_uses_the_header_emitted_for_extended_wav_specs() {
        assert_eq!(
            max_frames_per_chunk(spec(3, 192_000, 32)).unwrap(),
            (23 * 1024 * 1024 - 68) / (3 * 4)
        );
    }

    #[test]
    fn invalid_wav_spec_is_rejected() {
        assert!(max_frames_per_chunk(spec(0, 16_000, 16)).is_err());
        assert!(max_frames_per_chunk(spec(1, 0, 16)).is_err());
        assert!(max_frames_per_chunk(spec(1, 16_000, 0)).is_err());
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
