use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hound::{WavReader, WavSpec, WavWriter};
use tracing::{debug, info, warn};

/// A temporary WAV chunk file that deletes itself when dropped.
pub struct TmpChunk {
    pub path: PathBuf,
    /// Zero-based index of this chunk within the split sequence.
    pub index: usize,
}

impl TmpChunk {
    pub fn new(path: PathBuf, index: usize) -> Self {
        Self { path, index }
    }

    pub fn path_str(&self) -> &str {
        self.path.to_str().unwrap_or("")
    }
}

impl Drop for TmpChunk {
    fn drop(&mut self) {
        if self.path.exists() {
            if let Err(e) = std::fs::remove_file(&self.path) {
                warn!(path = ?self.path, error = %e, "Failed to delete tmp chunk");
            } else {
                debug!(path = ?self.path, "Deleted tmp chunk");
            }
        }
    }
}

/// Split a WAV file into chunks limited by duration and/or size.
///
/// Returns an **empty** `Vec` when the file already fits within the limits
/// (i.e. no split is needed — the caller should use the original file).
///
/// - `max_chunk_duration_secs`: max seconds per chunk; 0 means no duration limit.
/// - `max_chunk_size_bytes`:    max bytes per chunk including the 44-byte WAV header; 0 means no size limit.
pub fn split_wav(
    path: &str,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
) -> Result<Vec<TmpChunk>, Box<dyn std::error::Error>> {
    let mut reader = WavReader::open(path)?;
    let spec = reader.spec();

    const WAV_HEADER_BYTES: u64 = 44;
    let bytes_per_sample = (spec.bits_per_sample / 8) as u64;
    let channels = spec.channels as u64;
    let sample_rate = spec.sample_rate as u64;

    // Max samples per chunk from the duration limit
    let max_samples_from_duration: u64 = if max_chunk_duration_secs > 0 {
        max_chunk_duration_secs as u64 * sample_rate * channels
    } else {
        u64::MAX
    };

    // Max samples per chunk from the byte-size limit
    let max_samples_from_size: u64 = if max_chunk_size_bytes > WAV_HEADER_BYTES {
        (max_chunk_size_bytes - WAV_HEADER_BYTES) / bytes_per_sample
    } else if max_chunk_size_bytes > 0 {
        0
    } else {
        u64::MAX
    };

    let chunk_max_samples = max_samples_from_duration.min(max_samples_from_size);

    // No limit set or file fits in one chunk — skip splitting
    if chunk_max_samples == u64::MAX {
        return Ok(vec![]);
    }
    let total_samples = reader.len() as u64;
    if total_samples <= chunk_max_samples {
        return Ok(vec![]);
    }

    // Derive a short source name for temp filenames
    let source = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

    std::fs::create_dir_all("./tmp")?;

    let chunk_spec = WavSpec {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: spec.bits_per_sample,
        sample_format: spec.sample_format,
    };

    let mut chunks: Vec<TmpChunk> = Vec::new();
    let mut samples_iter = reader.samples::<i16>();
    let mut chunk_index: usize = 0;

    loop {
        let chunk_path = PathBuf::from(format!(
            "./tmp/chunk_{}_{}_{}.wav",
            source, chunk_index, timestamp
        ));

        let mut writer = WavWriter::create(&chunk_path, chunk_spec)?;
        let mut samples_written: u64 = 0;

        while samples_written < chunk_max_samples {
            match samples_iter.next() {
                Some(Ok(sample)) => {
                    if let Err(e) = writer.write_sample(sample) {
                        let _ = std::fs::remove_file(&chunk_path);
                        return Err(e.into());
                    }
                    samples_written += 1;
                }
                Some(Err(e)) => {
                    let _ = std::fs::remove_file(&chunk_path);
                    return Err(e.into());
                }
                None => break,
            }
        }

        if let Err(e) = writer.finalize() {
            let _ = std::fs::remove_file(&chunk_path);
            return Err(e.into());
        }

        if samples_written == 0 {
            // No samples written — nothing left; clean up empty file and stop.
            let _ = std::fs::remove_file(&chunk_path);
            break;
        }

        info!(
            path = %chunk_path.display(),
            index = chunk_index,
            samples = samples_written,
            "WAV chunk written"
        );
        chunks.push(TmpChunk::new(chunk_path, chunk_index));
        chunk_index += 1;

        if chunks.len() > 100 {
            return Err(format!(
                "split_wav: chunk count exceeded 100 (source: {}). \
                 Use a longer chunk duration or check the input file size.",
                source
            )
            .into());
        }
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{WavReader, WavSpec, WavWriter};

    fn write_test_wav(path: &str, sample_rate: u32, num_samples: u32) {
        std::fs::create_dir_all("./tmp").unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec).unwrap();
        for i in 0..num_samples {
            writer.write_sample(i as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn test_short_audio_no_split() {
        let path = "./tmp/test_short_nosplit.wav";
        write_test_wav(path, 16000, 1000); // 0.0625 s
        let chunks = split_wav(path, 30, 23 * 1024 * 1024).unwrap();
        assert!(chunks.is_empty(), "Short audio should not be split");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_split_by_duration() {
        let path = "./tmp/test_split_duration.wav";
        // 5 s at 16 kHz = 80 000 samples; 2 s chunks → 3 chunks (32k, 32k, 16k)
        write_test_wav(path, 16000, 80000);
        let chunks = split_wav(path, 2, 0).unwrap();
        assert_eq!(
            chunks.len(),
            3,
            "5s audio split into 2s chunks should produce 3 chunks"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_each_chunk_is_valid_wav() {
        let path = "./tmp/test_valid_chunks.wav";
        write_test_wav(path, 16000, 64000); // 4 s
        let chunks = split_wav(path, 1, 0).unwrap(); // 1 s → 4 chunks
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            let reader = WavReader::open(&chunk.path);
            assert!(
                reader.is_ok(),
                "Chunk should be a valid WAV: {:?}",
                chunk.path
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_chunks_cover_all_samples() {
        let path = "./tmp/test_coverage.wav";
        let total_samples = 50000u32;
        write_test_wav(path, 16000, total_samples);
        let chunks = split_wav(path, 1, 0).unwrap(); // 1 s = 16 000 samples
        let counted: u32 = chunks
            .iter()
            .map(|c| WavReader::open(&c.path).unwrap().len())
            .sum();
        assert_eq!(counted, total_samples, "All samples must appear in chunks");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_tmp_chunk_drop_deletes_file() {
        std::fs::create_dir_all("./tmp").unwrap();
        let path = PathBuf::from("./tmp/test_drop_chunk.tmp");
        std::fs::write(&path, b"test").unwrap();
        assert!(path.exists());
        {
            let _chunk = TmpChunk::new(path.clone(), 0);
        } // Drop here
        assert!(!path.exists(), "TmpChunk should delete file on drop");
    }

    #[test]
    fn test_chunk_count_limit() {
        let path = "./tmp/test_chunk_limit.wav";
        // 101 chunks of 1 sample each at 16 kHz → split at 1 sample/chunk → 101 chunks → error
        // We need > 100 chunks: 202 samples split at 2 samples each = 101 chunks
        write_test_wav(path, 16000, 202);
        // max_duration_secs=0 (use size), max_chunk_size_bytes = 44 + 2*2 = 48 → 2 samples/chunk → 101 chunks
        let max_bytes: u64 = 44 + 2 * 2;
        let result = split_wav(path, 0, max_bytes);
        assert!(result.is_err(), "Should fail when chunk count exceeds 100");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_split_by_size() {
        let path = "./tmp/test_split_size.wav";
        // 16-bit mono at 16 kHz: 2 bytes/sample
        // 10 000 samples × 2 bytes = 20 000 bytes of PCM + 44 header = 20 044 bytes total
        // Split at 10 044 bytes → each chunk holds (10 044 - 44) / 2 = 5 000 samples → 2 chunks
        write_test_wav(path, 16000, 10000);
        let max_bytes: u64 = 44 + 5000 * 2; // exactly 5000 samples per chunk
        let chunks = split_wav(path, 0, max_bytes).unwrap();
        assert_eq!(
            chunks.len(),
            2,
            "10 000 samples split into 5 000-sample chunks should give 2 chunks"
        );
        let _ = std::fs::remove_file(path);
    }
}
