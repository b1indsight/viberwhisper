use std::error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::WavSpec;
use tracing::{debug, error, info, instrument, warn};

use super::chunk::{ChunkError, WavChunk, encode_i16_wav};
use super::{AudioConfig, max_frames_per_chunk};
use crate::session::SessionId;

#[derive(Debug)]
enum RecorderError {
    Chunk(ChunkError),
    DefaultInputConfig(cpal::DefaultStreamConfigError),
    BuildStream(cpal::BuildStreamError),
    PlayStream(cpal::PlayStreamError),
    NoInputDevice,
    UnsupportedSampleFormat,
    NotRecording,
    NoAudioData,
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chunk(error) => write!(f, "chunk error: {error}"),
            Self::DefaultInputConfig(error) => write!(f, "input configuration error: {error}"),
            Self::BuildStream(error) => write!(f, "failed to build input stream: {error}"),
            Self::PlayStream(error) => write!(f, "failed to start input stream: {error}"),
            Self::NoInputDevice => write!(f, "no input device available"),
            Self::UnsupportedSampleFormat => write!(f, "unsupported sample format"),
            Self::NotRecording => write!(f, "not currently recording"),
            Self::NoAudioData => write!(f, "no audio data recorded"),
        }
    }
}

impl error::Error for RecorderError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Chunk(error) => Some(error),
            Self::DefaultInputConfig(error) => Some(error),
            Self::BuildStream(error) => Some(error),
            Self::PlayStream(error) => Some(error),
            Self::NoInputDevice
            | Self::UnsupportedSampleFormat
            | Self::NotRecording
            | Self::NoAudioData => None,
        }
    }
}

impl From<ChunkError> for RecorderError {
    fn from(error: ChunkError) -> Self {
        Self::Chunk(error)
    }
}

impl From<cpal::DefaultStreamConfigError> for RecorderError {
    fn from(error: cpal::DefaultStreamConfigError) -> Self {
        Self::DefaultInputConfig(error)
    }
}

impl From<cpal::BuildStreamError> for RecorderError {
    fn from(error: cpal::BuildStreamError) -> Self {
        Self::BuildStream(error)
    }
}

impl From<cpal::PlayStreamError> for RecorderError {
    fn from(error: cpal::PlayStreamError) -> Self {
        Self::PlayStream(error)
    }
}

pub struct AudioRecorder {
    recording: Arc<AtomicBool>,
    active_session_id: Option<SessionId>,
    buffer: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    sample_count: Arc<AtomicUsize>,
    gain: f32,
    sample_rate: u32,
    /// Number of samples already emitted as chunks during the current recording.
    flushed_samples: usize,
    /// Number of complete chunks observed by the audio callback.
    ready_chunk_count: Arc<AtomicUsize>,
    /// Maximum mono frames per chunk. `None` means unlimited and `Some(0)` suppresses output.
    chunk_max_samples: Option<usize>,
    /// Production policy: max chunk duration in seconds.
    max_chunk_duration_secs: u32,
    /// Production policy: max chunk size in bytes, including the encoded WAV header.
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

impl AudioRecorder {
    /// Create a recorder with the module-owned production chunk policy.
    pub fn with_config(config: &AudioConfig) -> Self {
        let gain = config.mic_gain;
        let max_chunk_duration_secs = config.max_chunk_duration_secs;
        let max_chunk_size_bytes = config.max_chunk_size_bytes;
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

        info!(
            gain,
            max_chunk_duration_secs, max_chunk_size_bytes, "Audio recorder configured"
        );

        AudioRecorder {
            recording: Arc::new(AtomicBool::new(false)),
            active_session_id: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain,
            sample_rate: 44100,
            flushed_samples: 0,
            ready_chunk_count: Arc::new(AtomicUsize::new(0)),
            chunk_max_samples: None,
            max_chunk_duration_secs,
            max_chunk_size_bytes,
        }
    }

    pub fn start_recording(&mut self, session_id: SessionId) -> RecorderStartOutcome {
        if self.recording.load(Ordering::Relaxed) {
            return RecorderStartOutcome::AlreadyRecording {
                requested_session_id: session_id,
                active_session_id: self.active_session_id.unwrap_or(session_id),
            };
        }

        match self.try_start_recording(session_id) {
            Ok(()) => RecorderStartOutcome::Started { session_id },
            Err(error) => RecorderStartOutcome::Failed {
                session_id,
                error: error.to_string(),
            },
        }
    }

    #[instrument(skip(self))]
    fn try_start_recording(&mut self, session_id: SessionId) -> Result<(), RecorderError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(RecorderError::NoInputDevice)?;
        let config = device.default_input_config()?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        info!(sample_rate = sample_rate, channels = channels, format = ?sample_format, "Starting recording");

        self.sample_rate = sample_rate;
        self.flushed_samples = 0;
        self.ready_chunk_count.store(0, Ordering::Release);

        self.chunk_max_samples = max_frames_per_chunk(
            self.max_chunk_duration_secs,
            self.max_chunk_size_bytes,
            WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?
        .map(|frames| usize::try_from(frames).map_err(|_| ChunkError::ArithmeticOverflow))
        .transpose()?;

        let recording = Arc::clone(&self.recording);
        let buffer = Arc::clone(&self.buffer);
        let sample_count = Arc::clone(&self.sample_count);
        let ready_chunk_count = Arc::clone(&self.ready_chunk_count);
        let chunk_max_samples = self.chunk_max_samples.unwrap_or(0);
        let gain = self.gain;

        buffer.lock().unwrap().clear();
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
            ),
            _ => {
                self.recording.store(false, Ordering::Relaxed);
                return Err(RecorderError::UnsupportedSampleFormat);
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
        self.active_session_id = Some(session_id);

        info!("Recording started");
        Ok(())
    }

    /// Encode and return the next complete in-memory chunk, if one is ready.
    pub fn take_ready_chunk(&mut self) -> Option<ReadyChunk> {
        if !self.recording.load(Ordering::Relaxed) || self.active_session_id.is_none() {
            return None;
        }
        let chunk_max_samples = self.chunk_max_samples.filter(|&samples| samples > 0)?;

        let flushed_chunk_count = self.flushed_samples / chunk_max_samples;
        let ready_chunk_count = self.ready_chunk_count.load(Ordering::Acquire);
        if ready_chunk_count <= flushed_chunk_count {
            return None;
        }

        let chunk_end = self.flushed_samples + chunk_max_samples;
        let chunk_samples = {
            let buffer = self.buffer.lock().unwrap();
            let total_samples = buffer.len();
            if total_samples < chunk_max_samples {
                debug!(
                    total_samples = total_samples,
                    chunk_size = chunk_max_samples,
                    "Chunk count is ahead of buffered samples; retrying later"
                );
                return None;
            }
            buffer[..chunk_max_samples].to_vec()
        };

        match encode_i16_wav(&chunk_samples, self.sample_rate) {
            Ok(chunk) => {
                self.buffer.lock().unwrap().drain(..chunk_max_samples);
                self.flushed_samples = chunk_end;
                Some(ReadyChunk {
                    session_id: self.active_session_id?,
                    chunk,
                })
            }
            Err(e) => {
                warn!(error = %e, "Failed to encode in-recording chunk; will retry next cycle");
                None
            }
        }
    }

    #[instrument(skip(self))]
    pub fn stop_recording(&mut self, session_id: SessionId) -> RecorderStopOutcome {
        if !self.recording.load(Ordering::Relaxed) {
            return RecorderStopOutcome::NotRecording {
                requested_session_id: session_id,
            };
        }
        let active_session_id = self.active_session_id.unwrap_or(session_id);
        if active_session_id != session_id {
            return RecorderStopOutcome::StillRecording {
                session_id: active_session_id,
                error: format!(
                    "stop requested for session {}, active session is {}",
                    session_id.0, active_session_id.0
                ),
            };
        }

        match self.try_stop_recording() {
            Ok(chunks) => {
                self.reset_chunk_bookkeeping();
                self.active_session_id = None;
                RecorderStopOutcome::Stopped {
                    session_id,
                    chunks,
                    warning: None,
                }
            }
            Err(error) if self.recording.load(Ordering::Relaxed) => {
                RecorderStopOutcome::StillRecording {
                    session_id,
                    error: error.to_string(),
                }
            }
            Err(error) => {
                self.reset_chunk_bookkeeping();
                self.active_session_id = None;
                RecorderStopOutcome::Stopped {
                    session_id,
                    chunks: Vec::new(),
                    warning: Some(error.to_string()),
                }
            }
        }
    }

    #[instrument(skip(self))]
    fn try_stop_recording(&mut self) -> Result<Vec<WavChunk>, RecorderError> {
        if !self.recording.load(Ordering::Relaxed) {
            debug!("Not recording, ignoring stop request");
            return Err(RecorderError::NotRecording);
        }

        debug!("Stopping recording");
        self.recording.store(false, Ordering::Relaxed);

        // Wait for pending callbacks to complete
        thread::sleep(Duration::from_millis(200));

        drop(self.stream.take());
        debug!("Stream stopped");

        let samples = {
            let buffer = self.buffer.lock().unwrap();
            debug!(samples = buffer.len(), "Buffer size");

            if buffer.is_empty() {
                return if self.flushed_samples > 0 {
                    Ok(Vec::new())
                } else {
                    Err(RecorderError::NoAudioData)
                };
            }
            buffer.to_vec()
        };

        match self.chunk_max_samples {
            Some(0) => Ok(Vec::new()),
            Some(max_samples) => samples
                .chunks(max_samples)
                .map(|samples| encode_i16_wav(samples, self.sample_rate).map_err(Into::into))
                .collect(),
            None => Ok(vec![encode_i16_wav(&samples, self.sample_rate)?]),
        }
    }

    #[cfg(test)]
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    pub fn cancel_recording(&mut self, session_id: SessionId) -> RecorderCancelOutcome {
        if !self.recording.load(Ordering::Relaxed) {
            return RecorderCancelOutcome::NotRecording;
        }
        let active_session_id = self.active_session_id.unwrap_or(session_id);
        if active_session_id != session_id {
            return RecorderCancelOutcome::SessionMismatch { active_session_id };
        }

        self.recording.store(false, Ordering::Relaxed);
        drop(self.stream.take());
        self.reset_chunk_bookkeeping();
        self.active_session_id = None;
        RecorderCancelOutcome::Cancelled
    }

    fn reset_chunk_bookkeeping(&mut self) {
        self.buffer.lock().unwrap().clear();
        self.sample_count.store(0, Ordering::Relaxed);
        self.ready_chunk_count.store(0, Ordering::Release);
        self.flushed_samples = 0;
        self.chunk_max_samples = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyChunk {
    pub session_id: SessionId,
    pub chunk: WavChunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderStartOutcome {
    Started {
        session_id: SessionId,
    },
    AlreadyRecording {
        requested_session_id: SessionId,
        active_session_id: SessionId,
    },
    Failed {
        session_id: SessionId,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderStopOutcome {
    Stopped {
        session_id: SessionId,
        chunks: Vec<WavChunk>,
        warning: Option<String>,
    },
    StillRecording {
        session_id: SessionId,
        error: String,
    },
    NotRecording {
        requested_session_id: SessionId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderCancelOutcome {
    Cancelled,
    NotRecording,
    SessionMismatch { active_session_id: SessionId },
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests touch the real audio stack via `cpal`. On Windows CI they
    // have been observed to pass and then crash the test process during exit,
    // so keep them on platforms where teardown is stable.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_recorder_with_config() {
        let config = AudioConfig::from_section(&crate::core::config::AudioSection::default());
        let recorder = AudioRecorder::with_config(&config);
        assert!(!recorder.is_recording());
        assert_eq!(recorder.max_chunk_duration_secs, 30);
        assert_eq!(recorder.max_chunk_size_bytes, 23 * 1024 * 1024);
    }

    fn recorder_for_buffer(samples: Vec<i16>, chunk_max_samples: usize) -> AudioRecorder {
        AudioRecorder {
            recording: Arc::new(AtomicBool::new(true)),
            active_session_id: Some(SessionId(1)),
            buffer: Arc::new(Mutex::new(samples)),
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain: 1.0,
            sample_rate: 16000,
            flushed_samples: 0,
            ready_chunk_count: Arc::new(AtomicUsize::new(0)),
            chunk_max_samples: Some(chunk_max_samples),
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
        assert!(
            recorder.buffer.lock().unwrap().is_empty(),
            "flushed samples should be released from memory"
        );

        for ready in [first, second, third].into_iter().flatten() {
            assert!(hound::WavReader::new(std::io::Cursor::new(ready.chunk.bytes())).is_ok());
        }
    }

    #[test]
    fn test_stop_recording_splits_unflushed_ready_chunks() {
        let samples: Vec<i16> = (0..25).collect();
        let mut recorder = recorder_for_buffer(samples, 10);

        let result = recorder.stop_recording(SessionId(1));

        match result {
            RecorderStopOutcome::Stopped { chunks, .. } => {
                assert_eq!(chunks.len(), 3);
            }
            _ => panic!("expected stop-time chunk catch-up"),
        }
    }

    #[test]
    fn stop_time_chunk_catch_up_clears_readiness_and_stops_polling() {
        let samples: Vec<i16> = (0..25).collect();
        let mut recorder = recorder_for_buffer(samples, 10);
        recorder.ready_chunk_count.store(2, Ordering::Release);

        let result = recorder.stop_recording(SessionId(1));

        let RecorderStopOutcome::Stopped { chunks, .. } = result else {
            panic!("expected stop-time chunk catch-up");
        };
        assert_eq!(chunks.len(), 3);
        assert!(recorder.buffer.lock().unwrap().is_empty());
        assert_eq!(recorder.ready_chunk_count.load(Ordering::Acquire), 0);
        assert_eq!(recorder.flushed_samples, 0);
        assert_eq!(recorder.chunk_max_samples, None);
        assert!(recorder.take_ready_chunk().is_none());
    }

    #[test]
    fn test_stop_recording_catches_up_after_live_chunk() {
        let samples: Vec<i16> = (0..35).collect();
        let mut recorder = recorder_for_buffer(samples, 10);
        recorder.ready_chunk_count.store(1, Ordering::Release);

        let first = recorder.take_ready_chunk();
        assert!(first.is_some());

        let result = recorder.stop_recording(SessionId(1));

        match result {
            RecorderStopOutcome::Stopped { chunks, .. } => {
                assert_eq!(chunks.len(), 3);
            }
            _ => panic!("expected remaining audio to be chunked"),
        }
    }

    #[test]
    fn stop_result_reports_not_recording_without_guessing() {
        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        recorder.recording.store(false, Ordering::Relaxed);
        recorder.active_session_id = None;

        assert_eq!(
            recorder.stop_recording(SessionId(7)),
            RecorderStopOutcome::NotRecording {
                requested_session_id: SessionId(7)
            }
        );
    }

    #[test]
    fn mismatched_stop_does_not_stop_active_recording() {
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 10);

        assert!(matches!(
            recorder.stop_recording(SessionId(2)),
            RecorderStopOutcome::StillRecording {
                session_id: SessionId(1),
                ..
            }
        ));
        assert!(recorder.is_recording());
    }

    #[test]
    fn cancel_clears_recorder_state() {
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 10);

        assert_eq!(
            recorder.cancel_recording(SessionId(1)),
            RecorderCancelOutcome::Cancelled
        );
        assert!(!recorder.is_recording());
        assert!(recorder.buffer.lock().unwrap().is_empty());
    }

    #[test]
    fn too_small_chunk_capacity_produces_no_stop_chunk() {
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 10);
        recorder.chunk_max_samples = Some(0);

        let RecorderStopOutcome::Stopped { chunks, .. } = recorder.stop_recording(SessionId(1))
        else {
            panic!("expected stopped outcome");
        };

        assert!(chunks.is_empty());
    }
}
