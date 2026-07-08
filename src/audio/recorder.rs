use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use tracing::{debug, error, info, instrument, warn};

use super::unique_temp_wav_path;

pub struct AudioRecorder {
    recording: Arc<AtomicBool>,
    /// Receives sample blocks from the audio callback. The callback only does a
    /// non-blocking channel send, so it can never stall on the consumer holding
    /// a lock during large chunk copies.
    capture_rx: Option<mpsc::Receiver<Vec<i16>>>,
    /// Samples drained from the channel that have not been flushed to disk yet.
    /// Owned by the consumer side; no lock needed.
    pending: Vec<i16>,
    stream: Option<cpal::Stream>,
    sample_count: Arc<AtomicUsize>,
    gain: f32,
    sample_rate: u32,
    /// Number of samples already flushed to chunk files during the current recording.
    flushed_samples: usize,
    /// Number of complete chunks observed by the audio callback.
    ready_chunk_count: Arc<AtomicUsize>,
    /// WAV files generated during the current recording session.
    current_session_files: Vec<PathBuf>,
    /// Maximum samples per chunk (0 = unlimited). Computed from config at start_recording.
    chunk_max_samples: usize,
    /// Config: max chunk duration in seconds.
    max_chunk_duration_secs: u32,
    /// Config: max chunk size in bytes (including 44-byte WAV header).
    max_chunk_size_bytes: u64,
}

/// Shared logic for both I16 and F32 audio callbacks: hand mono samples to the
/// consumer via the channel and signal a flush when the chunk threshold is crossed.
fn push_mono_chunk(
    mono: Vec<i16>,
    capture_tx: &mpsc::Sender<Vec<i16>>,
    sample_count: &AtomicUsize,
    ready_chunk_count: &AtomicUsize,
    sample_rate: u32,
    chunk_max_samples: usize,
) {
    let len = mono.len();
    // A send only fails when the receiver is gone (recorder stopped/dropped);
    // the samples are irrelevant then.
    let _ = capture_tx.send(mono);
    let total = sample_count.fetch_add(len, Ordering::Relaxed) + len;
    if total % (sample_rate as usize / 2) < len {
        debug!(
            frames = total,
            seconds = total / sample_rate as usize,
            "Recording progress"
        );
    }
    if let Some(ready_chunks) = total.checked_div(chunk_max_samples) {
        ready_chunk_count.store(ready_chunks, Ordering::Release);
    }
}

/// Sample rate used for WAV files written for STT upload.
///
/// Whisper-style backends operate on 16 kHz mono internally, so uploading
/// device-rate audio (44.1/48 kHz) only inflates the transfer ~3x.
const TARGET_UPLOAD_SAMPLE_RATE: u32 = 16_000;

/// Downsample mono PCM with linear interpolation.
///
/// Linear interpolation is adequate for speech: the aliasing it admits sits
/// above 8 kHz where speech carries little energy, and the STT model discards
/// that band anyway.
fn resample_linear(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = pos - idx as f64;
        let a = samples[idx] as f64;
        let b = samples[(idx + 1).min(samples.len() - 1)] as f64;
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

/// Downsample to the upload rate when the device rate is higher; lower device
/// rates are kept as-is (upsampling adds bytes without adding information).
fn prepare_upload_samples(samples: &[i16], sample_rate: u32) -> (std::borrow::Cow<'_, [i16]>, u32) {
    if sample_rate > TARGET_UPLOAD_SAMPLE_RATE {
        (
            std::borrow::Cow::Owned(resample_linear(
                samples,
                sample_rate,
                TARGET_UPLOAD_SAMPLE_RATE,
            )),
            TARGET_UPLOAD_SAMPLE_RATE,
        )
    } else {
        (std::borrow::Cow::Borrowed(samples), sample_rate)
    }
}

/// Write `samples` as a 16-bit mono WAV file to `path`.
fn write_wav_to_path(
    path: &PathBuf,
    samples: &[i16],
    sample_rate: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)?;
    // Batch writer: one buffered pass instead of a per-sample Result check.
    let mut sample_writer = writer.get_i16_writer(samples.len() as u32);
    for &sample in samples {
        sample_writer.write_sample(sample);
    }
    sample_writer.flush()?;
    writer.finalize()?;
    Ok(())
}

impl AudioRecorder {
    #[cfg(test)]
    pub fn new(gain: f32) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_config(gain, 30, 23 * 1024 * 1024)
    }

    /// Create a recorder with chunk-splitting config.
    ///
    /// - `max_chunk_duration_secs`: flush a chunk every N seconds; 0 = no duration limit.
    /// - `max_chunk_size_bytes`: flush when the uncompressed PCM + 44-byte header exceeds
    ///   this size; 0 = no size limit.
    pub fn with_config(
        gain: f32,
        max_chunk_duration_secs: u32,
        max_chunk_size_bytes: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        let default_device_name = host
            .default_input_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_else(|| "(none)".to_string());
        info!(device = %default_device_name, "Default input device");

        match host.input_devices() {
            Ok(devices) => {
                for (i, device) in devices.enumerate() {
                    let name = device.name().unwrap_or_else(|_| "(unknown)".to_string());
                    debug!(index = i, name = %name, "Available input device");
                }
            }
            Err(e) => warn!(error = %e, "Failed to enumerate input devices"),
        }

        info!(gain = gain, "Mic gain set");

        Ok(AudioRecorder {
            recording: Arc::new(AtomicBool::new(false)),
            capture_rx: None,
            pending: Vec::new(),
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain,
            sample_rate: 44100,
            flushed_samples: 0,
            ready_chunk_count: Arc::new(AtomicUsize::new(0)),
            current_session_files: Vec::new(),
            chunk_max_samples: 0,
            max_chunk_duration_secs,
            max_chunk_size_bytes,
        })
    }

    #[instrument(skip(self))]
    pub fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.recording.load(Ordering::Relaxed) {
            debug!("Already recording, ignoring duplicate start request");
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No input device available")?;
        let config = device.default_input_config()?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        info!(sample_rate = sample_rate, channels = channels, format = ?sample_format, "Starting recording");

        self.sample_rate = sample_rate;
        self.flushed_samples = 0;
        self.current_session_files.clear();
        self.ready_chunk_count.store(0, Ordering::Release);

        // Compute max samples per chunk from config.
        const WAV_HEADER_BYTES: u64 = 44;
        let bytes_per_sample = 2u64; // i16 = 2 bytes (mono after downmix)
        let max_by_duration: usize = if self.max_chunk_duration_secs > 0 {
            self.max_chunk_duration_secs as usize * sample_rate as usize
        } else {
            usize::MAX
        };
        let max_by_size: usize = if self.max_chunk_size_bytes > WAV_HEADER_BYTES {
            ((self.max_chunk_size_bytes - WAV_HEADER_BYTES) / bytes_per_sample) as usize
        } else if self.max_chunk_size_bytes > 0 {
            0
        } else {
            usize::MAX
        };
        self.chunk_max_samples = max_by_duration.min(max_by_size);

        let recording = Arc::clone(&self.recording);
        let sample_count = Arc::clone(&self.sample_count);
        let ready_chunk_count = Arc::clone(&self.ready_chunk_count);
        let chunk_max_samples = self.chunk_max_samples;
        let gain = self.gain;

        let (capture_tx, capture_rx) = mpsc::channel::<Vec<i16>>();
        self.capture_rx = Some(capture_rx);
        self.pending.clear();
        sample_count.store(0, Ordering::Relaxed);

        let stream = match sample_format {
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if recording.load(Ordering::Relaxed) {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg =
                                    ch.iter().map(|&s| s as f32).sum::<f32>() / channels as f32;
                                (avg * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16
                            })
                            .collect();
                        push_mono_chunk(
                            mono,
                            &capture_tx,
                            &sample_count,
                            &ready_chunk_count,
                            sample_rate,
                            chunk_max_samples,
                        );
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            ),
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if recording.load(Ordering::Relaxed) {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg = ch.iter().sum::<f32>() / channels as f32;
                                (avg * gain).clamp(-1.0, 1.0) * i16::MAX as f32
                            })
                            .map(|s| s as i16)
                            .collect();
                        push_mono_chunk(
                            mono,
                            &capture_tx,
                            &sample_count,
                            &ready_chunk_count,
                            sample_rate,
                            chunk_max_samples,
                        );
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            ),
            _ => {
                self.recording.store(false, Ordering::Relaxed);
                return Err("Unsupported sample format".into());
            }
        }?;

        // Enable callbacks immediately before playback. Roll back the state if
        // the backend rejects playback so the next start request can retry.
        self.recording.store(true, Ordering::Relaxed);
        if let Err(error) = stream.play() {
            self.recording.store(false, Ordering::Relaxed);
            return Err(error.into());
        }
        self.stream = Some(stream);

        info!("Recording started");
        Ok(())
    }

    /// Poll for a completed chunk to transcribe in the background.
    ///
    /// Returns `Some(path)` when a new chunk has been written to disk and is ready
    /// for background transcription. Returns `None` when no chunk is ready yet.
    ///
    /// This should be called periodically from the main event loop while recording.
    pub fn take_ready_chunk(&mut self) -> Option<String> {
        if self.chunk_max_samples == 0 {
            return None;
        }

        let flushed_chunk_count = self.flushed_samples / self.chunk_max_samples;
        let ready_chunk_count = self.ready_chunk_count.load(Ordering::Acquire);
        if ready_chunk_count <= flushed_chunk_count {
            return None;
        }

        self.drain_captured_samples();

        let chunk_end = self.flushed_samples + self.chunk_max_samples;
        let chunk_index = flushed_chunk_count;
        if self.pending.len() < self.chunk_max_samples {
            debug!(
                total_samples = self.pending.len(),
                chunk_size = self.chunk_max_samples,
                "Chunk count is ahead of buffered samples; retrying later"
            );
            return None;
        }
        let chunk_samples = self.pending[..self.chunk_max_samples].to_vec();

        match self.write_chunk(&chunk_samples, chunk_index) {
            Ok(path) => {
                self.pending.drain(..self.chunk_max_samples);
                self.flushed_samples = chunk_end;
                Some(path)
            }
            Err(e) => {
                warn!(error = %e, "Failed to write in-recording chunk; will retry next cycle");
                None
            }
        }
    }

    /// Write PCM samples to a WAV file under the app temp dir and return the path.
    fn write_chunk(
        &mut self,
        samples: &[i16],
        chunk_index: usize,
    ) -> Result<String, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(super::temp_dir())?;
        let path = unique_temp_wav_path(&format!("chunk_live_{chunk_index:04}"))?;

        let (upload_samples, upload_rate) = prepare_upload_samples(samples, self.sample_rate);
        write_wav_to_path(&path, &upload_samples, upload_rate)?;

        let path_str = path.to_string_lossy().to_string();
        info!(path = %path_str, index = chunk_index, samples = upload_samples.len(), sample_rate = upload_rate, "Live chunk written");
        self.current_session_files.push(path);
        Ok(path_str)
    }

    #[instrument(skip(self))]
    pub fn stop_recording(&mut self) -> Result<StopResult, Box<dyn std::error::Error>> {
        if !self.recording.load(Ordering::Relaxed) {
            debug!("Not recording, ignoring stop request");
            return Err("Not currently recording".into());
        }

        debug!("Stopping recording");
        self.recording.store(false, Ordering::Relaxed);

        // Wait for pending callbacks to complete
        thread::sleep(Duration::from_millis(200));

        drop(self.stream.take());
        debug!("Stream stopped");

        // Collect everything the callback produced; then close the channel so a
        // late callback (should not happen after the stream drop) sends into void.
        self.drain_captured_samples();
        self.capture_rx = None;

        let tail_samples = std::mem::take(&mut self.pending);
        debug!(samples = tail_samples.len(), "Buffer size");
        let chunk_index = if self.flushed_samples > 0 && self.chunk_max_samples > 0 {
            self.flushed_samples / self.chunk_max_samples
        } else {
            0
        };
        let wrote_live_chunks = self.flushed_samples > 0;

        if tail_samples.is_empty() {
            if wrote_live_chunks {
                // All audio was already flushed to live chunks; there is no tail
                // to write, but the session still has transcribable chunks.
                self.cleanup_old_recordings(&super::temp_dir(), 10);
                return Ok(StopResult::ChunksOnly);
            }
            return Err("No audio data recorded".into());
        }

        // If chunking is enabled and enough unflushed audio remains, catch up by
        // writing every complete chunk before the final tail.
        if self.chunk_max_samples > 0 && tail_samples.len() >= self.chunk_max_samples {
            let paths = self.write_chunk_sequence(&tail_samples, chunk_index)?;
            self.cleanup_old_recordings(&super::temp_dir(), 10);
            return Ok(StopResult::ChunkFiles(paths));
        }

        // Write tail (or full recording if no prior chunks and no stop-time catch-up is needed).
        let path = if !wrote_live_chunks {
            // No chunking happened — write the original-style single file.
            self.write_full_recording(&tail_samples)?
        } else {
            self.write_chunk(&tail_samples, chunk_index)?
        };

        self.cleanup_old_recordings(&super::temp_dir(), 10);

        if !wrote_live_chunks {
            Ok(StopResult::SingleFile(path))
        } else {
            Ok(StopResult::TailChunk(path))
        }
    }

    /// Move every sample block the audio callback has produced so far into the
    /// local pending buffer. Non-blocking.
    fn drain_captured_samples(&mut self) {
        if let Some(rx) = &self.capture_rx {
            while let Ok(block) = rx.try_recv() {
                self.pending.extend_from_slice(&block);
            }
        }
    }

    fn write_chunk_sequence(
        &mut self,
        samples: &[i16],
        start_chunk_index: usize,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut paths = Vec::new();
        for (offset, chunk) in samples.chunks(self.chunk_max_samples).enumerate() {
            paths.push(self.write_chunk(chunk, start_chunk_index + offset)?);
        }
        Ok(paths)
    }

    /// Write the entire buffer as a single WAV file (legacy path, no chunking).
    fn write_full_recording(
        &mut self,
        buffer: &[i16],
    ) -> Result<String, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(super::temp_dir())?;
        let path = unique_temp_wav_path("recording")?;
        let filename = path.to_string_lossy().to_string();
        debug!(path = %filename, "Saving recording");

        let (upload_samples, upload_rate) = prepare_upload_samples(buffer, self.sample_rate);
        write_wav_to_path(&path, &upload_samples, upload_rate)?;

        info!(path = %filename, "Recording saved");
        self.current_session_files.push(path);
        Ok(filename)
    }

    fn cleanup_old_recordings(&self, dir: &std::path::Path, keep: usize) {
        let current_files: HashSet<OsString> = self
            .current_session_files
            .iter()
            .filter_map(|path| path.file_name().map(|name| name.to_owned()))
            .collect();
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "wav")
                        .unwrap_or(false)
                })
                .filter(|e| {
                    e.path()
                        .file_name()
                        .is_none_or(|name| !current_files.contains(name))
                })
                .collect(),
            Err(_) => return,
        };

        if entries.len() <= keep {
            return;
        }

        entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());

        for entry in &entries[..entries.len() - keep] {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                warn!(path = ?entry.path(), error = %e, "Failed to delete old recording");
            } else {
                debug!(path = ?entry.path(), "Deleted old recording");
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }
}

/// Result returned by `stop_recording`.
pub enum StopResult {
    /// No chunking occurred; the entire recording is in this single WAV file.
    SingleFile(String),
    /// Some chunks were flushed during recording; this is the final tail chunk.
    TailChunk(String),
    /// One or more chunks were written while stopping to catch up with unflushed audio.
    ChunkFiles(Vec<String>),
    /// All audio was flushed to chunks during recording; no tail remains.
    ChunksOnly,
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests touch the real audio stack via `cpal`. On Windows CI they
    // have been observed to pass and then crash the test process during exit,
    // so keep them on platforms where teardown is stable.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_audio_recorder_creation() {
        let recorder = AudioRecorder::new(1.0);
        assert!(recorder.is_ok());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_recorder_not_recording_initially() {
        let recorder = AudioRecorder::new(1.0).unwrap();
        assert!(!recorder.is_recording());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_recorder_with_config() {
        let recorder = AudioRecorder::with_config(1.0, 30, 23 * 1024 * 1024);
        assert!(recorder.is_ok());
        let r = recorder.unwrap();
        assert_eq!(r.max_chunk_duration_secs, 30);
        assert_eq!(r.max_chunk_size_bytes, 23 * 1024 * 1024);
    }

    #[test]
    fn test_stop_result_variants_exist() {
        // Just verify the enum compiles and variants are accessible.
        let _single = StopResult::SingleFile("path".to_string());
        let _tail = StopResult::TailChunk("path".to_string());
        let _chunks = StopResult::ChunkFiles(vec!["path".to_string()]);
        let _chunks = StopResult::ChunksOnly;
    }

    fn recorder_for_buffer(samples: Vec<i16>, chunk_max_samples: usize) -> AudioRecorder {
        AudioRecorder {
            recording: Arc::new(AtomicBool::new(true)),
            capture_rx: None,
            pending: samples,
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain: 1.0,
            sample_rate: 16000,
            flushed_samples: 0,
            ready_chunk_count: Arc::new(AtomicUsize::new(0)),
            current_session_files: Vec::new(),
            chunk_max_samples,
            max_chunk_duration_secs: 0,
            max_chunk_size_bytes: 0,
        }
    }

    #[test]
    fn test_take_ready_chunk_flushes_each_ready_chunk() {
        let samples: Vec<i16> = (0..30).collect();
        let mut recorder = recorder_for_buffer(samples, 10);
        recorder.ready_chunk_count.store(3, Ordering::Release);

        let first = recorder.take_ready_chunk();
        let second = recorder.take_ready_chunk();
        let third = recorder.take_ready_chunk();
        let fourth = recorder.take_ready_chunk();

        assert!(first.is_some());
        assert!(second.is_some());
        assert!(third.is_some());
        assert!(fourth.is_none());
        assert_eq!(recorder.flushed_samples, 30);
        assert_eq!(recorder.current_session_files.len(), 3);
        assert!(
            recorder.pending.is_empty(),
            "flushed samples should be released from memory"
        );

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_resample_linear_scales_length() {
        let samples: Vec<i16> = vec![0; 44_100];
        let out = resample_linear(&samples, 44_100, 16_000);
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn test_resample_linear_preserves_constant_signal() {
        let samples: Vec<i16> = vec![1234; 4_410];
        let out = resample_linear(&samples, 44_100, 16_000);
        assert!(out.iter().all(|&s| s == 1234));
    }

    #[test]
    fn test_resample_linear_same_rate_is_identity() {
        let samples: Vec<i16> = (0..100).collect();
        assert_eq!(resample_linear(&samples, 16_000, 16_000), samples);
    }

    #[test]
    fn test_prepare_upload_samples_keeps_low_rates() {
        let samples: Vec<i16> = (0..100).collect();
        let (out, rate) = prepare_upload_samples(&samples, 8_000);
        assert_eq!(rate, 8_000);
        assert_eq!(out.as_ref(), samples.as_slice());
    }

    #[test]
    fn test_write_chunk_downsamples_high_rate_audio() {
        let mut recorder = recorder_for_buffer(Vec::new(), 0);
        recorder.sample_rate = 44_100;

        let samples: Vec<i16> = vec![0; 44_100]; // 1 second at device rate
        let path = recorder.write_chunk(&samples, 0).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.len(), 16_000); // still ~1 second at the upload rate

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_take_ready_chunk_drains_callback_channel() {
        // Samples arrive through the channel exactly as the audio callback
        // would deliver them, in several small blocks.
        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        let (tx, rx) = mpsc::channel::<Vec<i16>>();
        recorder.capture_rx = Some(rx);
        for block in [
            (0..4).collect::<Vec<i16>>(),
            (4..10).collect(),
            (10..12).collect(),
        ] {
            tx.send(block).unwrap();
        }
        recorder.ready_chunk_count.store(1, Ordering::Release);

        let path = recorder.take_ready_chunk();

        assert!(path.is_some());
        assert_eq!(recorder.flushed_samples, 10);
        // The partial second chunk stays pending.
        assert_eq!(recorder.pending, vec![10, 11]);

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_chunk_paths_are_unique() {
        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        let first = recorder.write_chunk(&[1, 2, 3], 0).unwrap();
        let second = recorder.write_chunk(&[4, 5, 6], 0).unwrap();

        assert_ne!(first, second);

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_cleanup_old_recordings_keeps_current_session_files() {
        let dir =
            std::env::temp_dir().join(format!("viberwhisper-audio-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("current.wav");
        let old = dir.join("old.wav");
        std::fs::write(&current, b"current").unwrap();
        std::fs::write(&old, b"old").unwrap();

        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        recorder.current_session_files.push(current.clone());
        recorder.cleanup_old_recordings(&dir, 0);

        assert!(current.exists());
        assert!(!old.exists());

        let _ = std::fs::remove_file(current);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn test_stop_recording_splits_unflushed_ready_chunks() {
        let samples: Vec<i16> = (0..25).collect();
        let mut recorder = recorder_for_buffer(samples, 10);

        let result = recorder.stop_recording().unwrap();

        match result {
            StopResult::ChunkFiles(paths) => {
                assert_eq!(paths.len(), 3);
                assert_eq!(recorder.current_session_files.len(), 3);
            }
            _ => panic!("expected stop-time chunk catch-up"),
        }

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_stop_recording_all_audio_flushed_returns_chunks_only() {
        // Everything was already written as live chunks; the buffer is empty.
        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        recorder.flushed_samples = 30;

        let result = recorder.stop_recording().unwrap();

        assert!(matches!(result, StopResult::ChunksOnly));
    }

    #[test]
    fn test_stop_recording_no_audio_at_all_is_an_error() {
        // Nothing was flushed and nothing is buffered — a genuinely empty recording.
        let mut recorder = recorder_for_buffer(Vec::new(), 10);

        assert!(recorder.stop_recording().is_err());
    }

    #[test]
    fn test_stop_recording_catches_up_after_live_chunk() {
        let samples: Vec<i16> = (0..35).collect();
        let mut recorder = recorder_for_buffer(samples, 10);
        recorder.ready_chunk_count.store(1, Ordering::Release);

        let first = recorder.take_ready_chunk();
        assert!(first.is_some());

        let result = recorder.stop_recording().unwrap();

        match result {
            StopResult::ChunkFiles(paths) => {
                assert_eq!(paths.len(), 3);
                assert_eq!(recorder.current_session_files.len(), 4);
            }
            _ => panic!("expected remaining audio to be chunked"),
        }

        for path in recorder.current_session_files {
            let _ = std::fs::remove_file(path);
        }
    }
}
