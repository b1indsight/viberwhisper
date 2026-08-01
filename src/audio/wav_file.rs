use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

use hound::{Sample, WavReader, WavSpec, WavWriter};

use super::chunk::{ChunkError, WavChunk};
use super::max_frames_per_chunk;

pub struct WavChunkReader {
    reader: WavReader<BufReader<File>>,
    spec: WavSpec,
    frames_per_chunk: Option<u64>,
    remaining_samples: u64,
    finished: bool,
}

impl WavChunkReader {
    pub fn open(
        path: &Path,
        max_chunk_duration_secs: u32,
        max_chunk_size_bytes: u64,
    ) -> Result<Self, ChunkError> {
        let reader = WavReader::open(path)?;
        let spec = reader.spec();
        let frames_per_chunk =
            max_frames_per_chunk(max_chunk_duration_secs, max_chunk_size_bytes, spec)?;
        let remaining_samples = u64::from(reader.len());

        Ok(Self {
            reader,
            spec,
            frames_per_chunk,
            remaining_samples,
            finished: false,
        })
    }

    pub fn chunks(&mut self) -> WavChunks<'_> {
        WavChunks { reader: self }
    }

    fn next_chunk(&mut self) -> Option<Result<WavChunk, ChunkError>> {
        if self.finished || self.remaining_samples == 0 || self.frames_per_chunk == Some(0) {
            self.finished = true;
            return None;
        }

        let sample_capacity = match self.frames_per_chunk {
            Some(frames) => frames.saturating_mul(u64::from(self.spec.channels)),
            None => self.remaining_samples,
        };
        let sample_count = self.remaining_samples.min(sample_capacity);
        let result = match self.spec.sample_format {
            hound::SampleFormat::Float => {
                encode_next_samples::<_, f32>(&mut self.reader, self.spec, sample_count)
            }
            hound::SampleFormat::Int => {
                encode_next_samples::<_, i32>(&mut self.reader, self.spec, sample_count)
            }
        };

        match result {
            Ok(chunk) => {
                self.remaining_samples -= sample_count;
                Some(Ok(chunk))
            }
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}

pub struct WavChunks<'a> {
    reader: &'a mut WavChunkReader,
}

impl Iterator for WavChunks<'_> {
    type Item = Result<WavChunk, ChunkError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.reader.next_chunk()
    }
}

fn encode_next_samples<R, S>(
    reader: &mut WavReader<R>,
    spec: WavSpec,
    sample_count: u64,
) -> Result<WavChunk, ChunkError>
where
    R: std::io::Read,
    S: Sample,
{
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(&mut cursor, spec)?;
        let mut samples = reader.samples::<S>();
        for _ in 0..sample_count {
            let sample = samples.next().ok_or(ChunkError::UnexpectedEndOfFile)??;
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }
    Ok(WavChunk::from_encoded_bytes(cursor.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::SampleFormat;

    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "viberwhisper-wav-reader-{}-{name}.wav",
            std::process::id()
        ))
    }

    fn write_int_wav(path: &Path, spec: WavSpec, sample_count: u32) {
        let mut writer = WavWriter::create(path, spec).unwrap();
        for index in 0..sample_count {
            writer.write_sample(index as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn write_float_wav(path: &Path, sample_count: u32) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 4,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::create(path, spec).unwrap();
        for index in 0..sample_count {
            writer
                .write_sample(index as f32 / sample_count as f32)
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    fn read_i16_samples(chunk: &WavChunk) -> Vec<i16> {
        WavReader::new(Cursor::new(chunk.bytes()))
            .unwrap()
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn chunks_stream_the_whole_file_in_order() {
        let path = test_path("ordered");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 4,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_int_wav(&path, spec, 10);

        let mut reader = WavChunkReader::open(&path, 1, 0).unwrap();
        let chunks = reader.chunks().collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks.iter().flat_map(read_i16_samples).collect::<Vec<_>>(),
            (0..10).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabled_limits_return_the_whole_file_as_one_chunk() {
        let path = test_path("unlimited");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_int_wav(&path, spec, 100);

        let mut reader = WavChunkReader::open(&path, 0, 0).unwrap();
        let chunks = reader.chunks().collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(read_i16_samples(&chunks[0]).len(), 100);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn too_small_size_capacity_yields_no_chunks() {
        let path = test_path("too-small");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_int_wav(&path, spec, 16_000);

        let mut reader = WavChunkReader::open(&path, 0, 44 + 8_000 * 2).unwrap();

        assert_eq!(reader.chunks().count(), 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn iterator_has_no_hard_chunk_count_limit() {
        let path = test_path("many");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 2,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_int_wav(&path, spec, 202);

        let mut reader = WavChunkReader::open(&path, 1, 0).unwrap();

        assert_eq!(
            reader
                .chunks()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len(),
            101
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn chunks_preserve_float_wav_format() {
        let path = test_path("float");
        write_float_wav(&path, 8);

        let mut reader = WavChunkReader::open(&path, 1, 0).unwrap();
        let chunks = reader.chunks().collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(chunks.len(), 2);
        for chunk in chunks {
            let reader = WavReader::new(Cursor::new(chunk.bytes())).unwrap();
            assert_eq!(reader.spec().sample_format, SampleFormat::Float);
            assert_eq!(reader.spec().bits_per_sample, 32);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn decoding_error_is_returned_once_then_iteration_stops() {
        let path = test_path("truncated");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 4,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_int_wav(&path, spec, 8);
        let truncated_len = std::fs::metadata(&path).unwrap().len() - 1;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(truncated_len)
            .unwrap();

        let mut reader = WavChunkReader::open(&path, 0, 0).unwrap();
        let mut chunks = reader.chunks();

        assert!(matches!(chunks.next(), Some(Err(_))));
        assert!(chunks.next().is_none());
        let _ = std::fs::remove_file(path);
    }
}
