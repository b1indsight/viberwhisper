use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use tracing::{debug, error, info, instrument, warn};

pub struct AudioRecorder {
    recording: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<i16>>>,
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

/// Shared logic for both I16 and F32 audio callbacks: append mono samples to the
/// buffer and signal a flush when the chunk threshold is crossed.
fn push_mono_chunk(
    mono: Vec<i16>,
    buffer: &Mutex<Vec<i16>>,
    sample_count: &AtomicUsize,
    ready_chunk_count: &AtomicUsize,
    sample_rate: u32,
    chunk_max_samples: usize,
) {
    let len = mono.len();
    buffer.lock().unwrap().extend_from_slice(&mono);
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
    for &sample in samples {
        writer.write_sample(sample)?;
    }
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
            buffer: Arc::new(Mutex::new(Vec::new())),
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
        let buffer = Arc::clone(&self.buffer);
        let sample_count = Arc::clone(&self.sample_count);
        let ready_chunk_count = Arc::clone(&self.ready_chunk_count);
        let chunk_max_samples = self.chunk_max_samples;
        let gain = self.gain;

        buffer.lock().unwrap().clear();
        sample_count.store(0, Ordering::Relaxed);

        // Set to true before starting stream to avoid dropping initial frames
        self.recording.store(true, Ordering::Relaxed);

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
                            &buffer,
                            &sample_count,
                            &ready_chunk_count,
                            sample_rate,
                            chunk_max_samples,
                        );
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            )?,
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
                            &buffer,
                            &sample_count,
                            &ready_chunk_count,
                            sample_rate,
                            chunk_max_samples,
                        );
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            )?,
            _ => {
                self.recording.store(false, Ordering::Relaxed);
                return Err("Unsupported sample format".into());
            }
        };

        stream.play()?;
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

        let chunk_end = self.flushed_samples + self.chunk_max_samples;
        let chunk_index = flushed_chunk_count;
        let chunk_samples = {
            let buffer = self.buffer.lock().unwrap();
            let total_samples = buffer.len();
            if total_samples < chunk_end {
                debug!(
                    total_samples = total_samples,
                    chunk_end = chunk_end,
                    "Chunk count is ahead of buffered samples; retrying later"
                );
                return None;
            }
            buffer[self.flushed_samples..chunk_end].to_vec()
        };

        match self.write_chunk(&chunk_samples, chunk_index) {
            Ok(path) => {
                self.flushed_samples = chunk_end;
                Some(path)
            }
            Err(e) => {
                warn!(error = %e, "Failed to write in-recording chunk; will retry next cycle");
                None
            }
        }
    }

    /// Write PCM samples to a WAV file under ./tmp/ and return the path.
    fn write_chunk(
        &mut self,
        samples: &[i16],
        chunk_index: usize,
    ) -> Result<String, Box<dyn std::error::Error>> {
        std::fs::create_dir_all("./tmp")?;
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let path = PathBuf::from(format!(
            "./tmp/chunk_live_{:04}_{}.wav",
            chunk_index, timestamp
        ));

        write_wav_to_path(&path, samples, self.sample_rate)?;

        let path_str = path.to_string_lossy().to_string();
        info!(path = %path_str, index = chunk_index, samples = samples.len(), "Live chunk written");
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

        let (buffer_len, tail_samples, chunk_index, wrote_live_chunks) = {
            let buffer = self.buffer.lock().unwrap();
            debug!(samples = buffer.len(), "Buffer size");

            if buffer.is_empty() {
                return Err("No audio data recorded".into());
            }

            let tail_samples = buffer[self.flushed_samples..].to_vec();
            let chunk_index = if self.flushed_samples > 0 && self.chunk_max_samples > 0 {
                self.flushed_samples / self.chunk_max_samples
            } else {
                0
            };
            (
                buffer.len(),
                tail_samples,
                chunk_index,
                self.flushed_samples > 0,
            )
        };

        if buffer_len == 0 {
            return Err("No audio data recorded".into());
        }

        // If we have already flushed some chunks, write the tail (remaining samples).
        // If no chunks were flushed, write the whole buffer as a single WAV.
        if tail_samples.is_empty() {
            // All audio was already flushed to chunks; nothing left.
            self.cleanup_old_recordings("./tmp", 10);
            return Ok(StopResult::ChunksOnly);
        }

        // Write tail (or full recording if no prior chunks).
        let path = if !wrote_live_chunks {
            // No chunking happened — write the original-style single file.
            self.write_full_recording(&tail_samples)?
        } else {
            self.write_chunk(&tail_samples, chunk_index)?
        };

        self.cleanup_old_recordings("./tmp", 10);

        if !wrote_live_chunks {
            Ok(StopResult::SingleFile(path))
        } else {
            Ok(StopResult::TailChunk(path))
        }
    }

    /// Write the entire buffer as a single WAV file (legacy path, no chunking).
    fn write_full_recording(
        &mut self,
        buffer: &[i16],
    ) -> Result<String, Box<dyn std::error::Error>> {
        std::fs::create_dir_all("./tmp")?;
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let path = PathBuf::from(format!("./tmp/recording_{}.wav", timestamp));
        let filename = path.to_string_lossy().to_string();
        debug!(path = %filename, "Saving recording");

        write_wav_to_path(&path, buffer, self.sample_rate)?;

        info!(path = %filename, "Recording saved");
        self.current_session_files.push(path);
        Ok(filename)
    }

    fn cleanup_old_recordings(&self, dir: &str, keep: usize) {
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
        let _chunks = StopResult::ChunksOnly;
    }

    fn recorder_for_buffer(samples: Vec<i16>, chunk_max_samples: usize) -> AudioRecorder {
        AudioRecorder {
            recording: Arc::new(AtomicBool::new(true)),
            buffer: Arc::new(Mutex::new(samples)),
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
        recorder.cleanup_old_recordings(dir.to_str().unwrap(), 0);

        assert!(current.exists());
        assert!(!old.exists());

        let _ = std::fs::remove_file(current);
        let _ = std::fs::remove_dir(dir);
    }
}
