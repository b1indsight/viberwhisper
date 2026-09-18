//! Shared upload preparation. Source WAVs remain immutable and failures preserve upload.

use std::io::Cursor;

use anyhow::{Result, ensure};
use hound::{SampleFormat, WavReader, WavSpec};
use tracing::{info, warn};

use super::{MAX_CHUNK_SIZE_BYTES, WavChunk, contains_audible_window, vad};

/// Returns `None` for silence/non-speech, otherwise one immutable payload for all retries.
pub(crate) fn prepare_for_transcription(chunk: &WavChunk) -> Option<WavChunk> {
    prepare_with_vad(chunk, vad::contains_speech)
}

fn prepare_with_vad(
    chunk: &WavChunk,
    classify: impl FnOnce(&DecodedWav) -> Result<bool>,
) -> Option<WavChunk> {
    match contains_audible_window(chunk) {
        Ok(false) => {
            info!("Skipping effectively silent audio chunk");
            return None;
        }
        Ok(true) => {}
        Err(error) => {
            warn!(%error, "Could not classify audio signal; preserving original upload");
            return Some(chunk.clone());
        }
    }
    let pcm = match DecodedWav::read(chunk) {
        Ok(pcm) => pcm,
        Err(error) => {
            warn!(%error, "Could not decode audio for preprocessing; preserving original upload");
            return Some(chunk.clone());
        }
    };
    match classify(&pcm) {
        Ok(false) => {
            info!("Skipping audio chunk without detected speech");
            return None;
        }
        Ok(true) => {}
        Err(error) => warn!(%error, "Voice detection unavailable; continuing transcription"),
    }
    Some(pcm.adjust(chunk).unwrap_or_else(|error| {
        warn!(%error, "Could not adjust audio loudness; preserving original upload");
        chunk.clone()
    }))
}

/// Fully validated, interleaved samples normalized to digital full scale.
pub(super) struct DecodedWav {
    pub(super) spec: WavSpec,
    pub(super) samples: Vec<f64>,
}

impl DecodedWav {
    pub(super) fn read(chunk: &WavChunk) -> Result<Self> {
        ensure!(
            chunk.len() as u64 <= MAX_CHUNK_SIZE_BYTES,
            "audio exceeds chunk byte limit"
        );
        let mut reader = WavReader::new(Cursor::new(chunk.shared_bytes()))?;
        let spec = reader.spec();
        ensure!(
            spec.channels > 0 && spec.sample_rate > 0,
            "invalid WAV channel count or sample rate"
        );
        let declared = reader.len() as usize;
        ensure!(
            declared.is_multiple_of(usize::from(spec.channels)),
            "incomplete audio frame"
        );
        // Do not trust header lengths to reserve unbounded memory on malformed input.
        ensure!(declared <= chunk.len(), "invalid WAV sample count");
        let samples = match spec.sample_format {
            SampleFormat::Int => {
                ensure!(
                    matches!(spec.bits_per_sample, 8 | 16 | 24 | 32),
                    "unsupported integer WAV depth"
                );
                let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| f64::from(v) / scale))
                    .collect::<Result<Vec<_>, _>>()?
            }
            SampleFormat::Float => {
                ensure!(spec.bits_per_sample == 32, "unsupported float WAV depth");
                reader
                    .samples::<f32>()
                    .map(|s| s.map(f64::from))
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        ensure!(samples.len() == declared, "truncated WAV samples");
        ensure!(
            samples.iter().all(|s| s.is_finite()),
            "non-finite audio sample"
        );
        Ok(Self { spec, samples })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunk::encode_i16_wav;

    #[test]
    #[ignore = "requires VIBERWHISPER_VAD_DATASET pointing to a private prompt-lab dataset"]
    fn ready_dataset_speech_survives_preprocessing() {
        use crate::prompt_lab::{DatasetStore, ReferenceStatus};
        let root = std::path::PathBuf::from(std::env::var_os("VIBERWHISPER_VAD_DATASET").unwrap());
        assert!(
            root.join("dataset.json").is_file(),
            "use an existing dataset"
        );
        let store = DatasetStore::open_or_create(root).unwrap();
        let samples = store.list_samples(Some(ReferenceStatus::Ready)).unwrap();
        assert!(!samples.is_empty());
        let mut rejected = Vec::new();
        let mut elapsed = Vec::new();
        for sample in &samples {
            let chunks =
                crate::audio::WavChunkReader::open(&store.root().join(&sample.audio.path)).unwrap();
            let mut survived = false;
            for chunk in chunks {
                let chunk = chunk.unwrap();
                let start = std::time::Instant::now();
                survived |= prepare_for_transcription(&chunk).is_some();
                elapsed.push(start.elapsed());
            }
            if !survived {
                rejected.push(sample.id.clone());
            }
        }
        elapsed.sort();
        eprintln!(
            "ready_samples={} rejected={} median={:?} max={:?}",
            samples.len(),
            rejected.len(),
            elapsed[elapsed.len() / 2],
            elapsed.last()
        );
        assert!(rejected.is_empty(), "ready speech rejected: {rejected:?}");
    }

    #[test]
    fn silence_precedes_vad_and_vad_failure_still_prepares_upload() {
        let silent = encode_i16_wav(&[0; 800], 16_000).unwrap();
        assert!(prepare_with_vad(&silent, |_| panic!("silence must skip inference")).is_none());
        let audible = encode_i16_wav(&[1_000; 800], 16_000).unwrap();
        assert!(prepare_with_vad(&audible, |_| Ok(false)).is_none());
        let prepared = prepare_with_vad(&audible, |_| anyhow::bail!("inference failed")).unwrap();
        assert_eq!(DecodedWav::read(&prepared).unwrap().samples.len(), 800);
        assert!(DecodedWav::read(&prepared).unwrap().samples[0] > 0.09);
    }
}
