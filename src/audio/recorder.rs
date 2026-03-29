use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use tracing::{debug, error, info, instrument, warn};

pub struct AudioRecorder {
    recording: Arc<Mutex<bool>>,
    buffer: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    sample_count: Arc<AtomicUsize>,
    gain: f32,
    sample_rate: u32,
    /// Number of samples already flushed to chunk files during the current recording.
    flushed_samples: usize,
    /// Set by the audio callback when a new chunk threshold is crossed.
    flush_needed: Arc<AtomicBool>,
    /// Maximum samples per chunk (0 = unlimited). Computed from config at start_recording.
    chunk_max_samples: usize,
    /// Config: max chunk duration in seconds.
    max_chunk_duration_secs: u32,
    /// Config: max chunk size in bytes (including 44-byte WAV header).
    max_chunk_size_bytes: u64,
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
            recording: Arc::new(Mutex::new(false)),
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain,
            sample_rate: 44100,
            flushed_samples: 0,
            flush_needed: Arc::new(AtomicBool::new(false)),
            chunk_max_samples: 0,
            max_chunk_duration_secs,
            max_chunk_size_bytes,
        })
    }

    #[instrument(skip(self))]
    pub fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if *self.recording.lock().unwrap() {
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
        self.flush_needed.store(false, Ordering::Relaxed);

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
        let flush_needed = Arc::clone(&self.flush_needed);
        let chunk_max_samples = self.chunk_max_samples;
        let gain = self.gain;

        buffer.lock().unwrap().clear();
        sample_count.store(0, Ordering::Relaxed);

        // Set to true before starting stream to avoid dropping initial frames
        *self.recording.lock().unwrap() = true;

        let stream = match sample_format {
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg =
                                    ch.iter().map(|&s| s as f32).sum::<f32>() / channels as f32;
                                (avg * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16
                            })
                            .collect();
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
                        // Signal flush when chunk threshold is crossed.
                        if chunk_max_samples > 0
                            && !flush_needed.load(Ordering::Relaxed)
                            && total % chunk_max_samples < len
                        {
                            flush_needed.store(true, Ordering::Relaxed);
                        }
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            )?,
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg = ch.iter().sum::<f32>() / channels as f32;
                                (avg * gain).clamp(-1.0, 1.0) * i16::MAX as f32
                            })
                            .map(|s| s as i16)
                            .collect();
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
                        if chunk_max_samples > 0
                            && !flush_needed.load(Ordering::Relaxed)
                            && total % chunk_max_samples < len
                        {
                            flush_needed.store(true, Ordering::Relaxed);
                        }
                    }
                },
                move |err| error!(error = %err, "Stream error"),
                None,
            )?,
            _ => {
                *self.recording.lock().unwrap() = false;
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
        if !self.flush_needed.swap(false, Ordering::Relaxed) {
            return None;
        }
        if self.chunk_max_samples == 0 {
            return None;
        }

        let buffer = self.buffer.lock().unwrap();
        let total_samples = buffer.len();

        // Determine how many samples to flush: take the first complete chunk.
        let unflushed = total_samples.saturating_sub(self.flushed_samples);
        if unflushed < self.chunk_max_samples {
            // Not enough samples yet (race with callback) — re-arm the flag and skip.
            self.flush_needed.store(true, Ordering::Relaxed);
            return None;
        }

        let chunk_end = self.flushed_samples + self.chunk_max_samples;
        let chunk_samples = &buffer[self.flushed_samples..chunk_end];
        let chunk_index = self.flushed_samples / self.chunk_max_samples;

        match self.write_chunk(chunk_samples, chunk_index) {
            Ok(path) => {
                self.flushed_samples = chunk_end;
                Some(path)
            }
            Err(e) => {
                warn!(error = %e, "Failed to write in-recording chunk; will retry next cycle");
                // Re-arm so we try again next poll.
                self.flush_needed.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    /// Write PCM samples to a WAV file under ./tmp/ and return the path.
    fn write_chunk(
        &self,
        samples: &[i16],
        chunk_index: usize,
    ) -> Result<String, Box<dyn std::error::Error>> {
        std::fs::create_dir_all("./tmp")?;
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let path = PathBuf::from(format!(
            "./tmp/chunk_live_{:04}_{}.wav",
            chunk_index, timestamp
        ));

        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(&path, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;

        let path_str = path.to_string_lossy().to_string();
        info!(path = %path_str, index = chunk_index, samples = samples.len(), "Live chunk written");
        Ok(path_str)
    }

    #[instrument(skip(self))]
    pub fn stop_recording(&mut self) -> Result<StopResult, Box<dyn std::error::Error>> {
        if !*self.recording.lock().unwrap() {
            debug!("Not recording, ignoring stop request");
            return Err("Not currently recording".into());
        }

        debug!("Stopping recording");
        *self.recording.lock().unwrap() = false;

        // Wait for pending callbacks to complete
        thread::sleep(Duration::from_millis(200));

        drop(self.stream.take());
        debug!("Stream stopped");

        let buffer = self.buffer.lock().unwrap();
        debug!(samples = buffer.len(), "Buffer size");

        if buffer.is_empty() {
            return Err("No audio data recorded".into());
        }

        // If we have already flushed some chunks, write the tail (remaining samples).
        // If no chunks were flushed, write the whole buffer as a single WAV.
        let tail_samples = &buffer[self.flushed_samples..];
        if tail_samples.is_empty() {
            // All audio was already flushed to chunks; nothing left.
            return Ok(StopResult::ChunksOnly);
        }

        let chunk_index = if self.flushed_samples > 0 && self.chunk_max_samples > 0 {
            self.flushed_samples / self.chunk_max_samples
        } else {
            0
        };

        // Write tail (or full recording if no prior chunks).
        let path = if self.flushed_samples == 0 {
            // No chunking happened — write the original-style single file.
            self.write_full_recording(&buffer)?
        } else {
            self.write_chunk(tail_samples, chunk_index)?
        };

        self.cleanup_old_recordings("./tmp", 10);

        if self.flushed_samples == 0 {
            Ok(StopResult::SingleFile(path))
        } else {
            Ok(StopResult::TailChunk(path))
        }
    }

    /// Write the entire buffer as a single WAV file (legacy path, no chunking).
    fn write_full_recording(&self, buffer: &[i16]) -> Result<String, Box<dyn std::error::Error>> {
        let mut path = PathBuf::from("./tmp");
        std::fs::create_dir_all(&path)?;

        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        path.push(format!("recording_{}.wav", timestamp));
        let filename = path.to_string_lossy().to_string();
        debug!(path = %filename, "Saving recording");

        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(&path, spec)?;
        for (i, &sample) in buffer.iter().enumerate() {
            if let Err(e) = writer.write_sample(sample) {
                error!(sample_index = i, error = %e, "Failed to write sample");
                return Err(e.into());
            }
        }
        writer.finalize()?;

        info!(path = %filename, "Recording saved");
        Ok(filename)
    }

    fn cleanup_old_recordings(&self, dir: &str, keep: usize) {
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
        *self.recording.lock().unwrap()
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

    #[test]
    fn test_audio_recorder_creation() {
        let recorder = AudioRecorder::new(1.0);
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_recorder_not_recording_initially() {
        let recorder = AudioRecorder::new(1.0).unwrap();
        assert!(!recorder.is_recording());
    }

    #[test]
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
}
