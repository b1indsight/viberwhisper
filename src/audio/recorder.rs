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
use super::segment::{Segmentation, Segmenter};
use super::{AudioConfig, max_frames_per_chunk};
use crate::session::SessionId;

mod replay;
pub(crate) use replay::replay_wav;

#[derive(Debug)]
enum RecorderError {
    Chunk(ChunkError),
    DefaultInputConfig(cpal::DefaultStreamConfigError),
    BuildStream(cpal::BuildStreamError),
    PlayStream(cpal::PlayStreamError),
    EnumerateInputDevices(cpal::DevicesError),
    NoInputDevice,
    InputDeviceNotFound(String),
    UnsupportedSampleFormat,
    NoAudioData,
    StreamingOverflow,
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chunk(error) => write!(f, "chunk error: {error}"),
            Self::DefaultInputConfig(error) => write!(f, "input configuration error: {error}"),
            Self::BuildStream(error) => write!(f, "failed to build input stream: {error}"),
            Self::PlayStream(error) => write!(f, "failed to start input stream: {error}"),
            Self::EnumerateInputDevices(error) => {
                write!(f, "failed to enumerate input devices: {error}")
            }
            Self::NoInputDevice => write!(f, "no input device available"),
            Self::InputDeviceNotFound(name) => {
                write!(f, "configured input device `{name}` is not available")
            }
            Self::UnsupportedSampleFormat => write!(f, "unsupported sample format"),
            Self::NoAudioData => write!(f, "no audio data recorded"),
            Self::StreamingOverflow => write!(f, "Recorder backlog exceeded its capacity"),
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
            Self::EnumerateInputDevices(error) => Some(error),
            Self::NoInputDevice
            | Self::InputDeviceNotFound(_)
            | Self::UnsupportedSampleFormat
            | Self::NoAudioData
            | Self::StreamingOverflow => None,
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

/// Records one session at a time and produces independently decodable WAV chunks.
pub struct AudioRecorder {
    active: Option<ActiveRecording>,
    input_device: Option<String>,
    /// Captured by value when building a stream; changes affect only the next recording.
    pub(crate) gain: f32,
    chunk_mode: ChunkMode,
    chunk_notifier: Arc<dyn Fn(SessionId) + Send + Sync>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkMode {
    Fixed,
    Frames,
    Pauses,
}

struct ActiveRecording {
    session_id: SessionId,
    shared: Arc<RecordingBuffer>,
    stream: Option<cpal::Stream>,
    /// Number of samples already emitted as chunks during the current recording.
    flushed_samples: usize,
    segmenter: Option<Segmenter>,
    /// Absolute input cursor, including analyzed PCM not yet emitted.
    analyzed_samples: usize,
    /// Absolute end of a sealed segment, retained if WAV encoding fails.
    pending_cut: Option<(usize, Option<bool>)>,
}

struct RecordingBuffer {
    sample_rate: u32,
    /// Maximum mono frames per chunk; zero suppresses output.
    chunk_max_samples: usize,
    /// Callback wake cadence; pause-aware output can end between these wakeups.
    wake_samples: usize,
    recording: AtomicBool,
    samples: Mutex<Vec<i16>>,
    sample_count: AtomicUsize,
    /// Number of wake boundaries observed by the audio callback.
    ready_chunk_count: AtomicUsize,
    capacity: Option<usize>,
    overflowed: AtomicBool,
}

fn select_named_device<D>(
    devices: impl IntoIterator<Item = (D, String)>,
    requested: &str,
) -> Result<D, RecorderError> {
    devices
        .into_iter()
        .find(|(_, name)| name == requested)
        .map(|(device, _)| device)
        .ok_or_else(|| RecorderError::InputDeviceNotFound(requested.to_string()))
}

fn readable_named_devices(
    devices: impl IntoIterator<Item = cpal::Device>,
) -> impl Iterator<Item = (cpal::Device, String)> {
    devices
        .into_iter()
        .filter_map(|device| match device.name() {
            Ok(name) => Some((device, name)),
            Err(error) => {
                warn!(error = %error, "Skipping input device with unreadable name");
                None
            }
        })
}

/// Enumerates the display names accepted by the recorder's persisted device selection.
pub(crate) fn input_device_names() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let devices = host.input_devices().map_err(|error| error.to_string())?;
    Ok(readable_named_devices(devices)
        .map(|(_, name)| name)
        .collect())
}

fn resolve_input_device(
    host: &cpal::Host,
    requested: Option<&str>,
) -> Result<cpal::Device, RecorderError> {
    let Some(requested) = requested else {
        return host
            .default_input_device()
            .ok_or(RecorderError::NoInputDevice);
    };
    let devices = host
        .input_devices()
        .map_err(RecorderError::EnumerateInputDevices)?;
    select_named_device(readable_named_devices(devices), requested)
}

impl RecordingBuffer {
    /// Fixes the mono PCM format and chunk policy for the lifetime of one recording.
    fn new(sample_rate: u32) -> Result<Self, ChunkError> {
        let chunk_max_samples = usize::try_from(max_frames_per_chunk(WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        })?)
        .map_err(|_| ChunkError::ArithmeticOverflow)?;
        Ok(Self {
            sample_rate,
            chunk_max_samples,
            wake_samples: chunk_max_samples,
            recording: AtomicBool::new(false),
            samples: Mutex::new(Vec::new()),
            sample_count: AtomicUsize::new(0),
            ready_chunk_count: AtomicUsize::new(0),
            capacity: None,
            overflowed: AtomicBool::new(false),
        })
    }

    fn enable_streaming(&mut self) {
        self.chunk_max_samples = (self.sample_rate as usize / 5).max(1);
        self.wake_samples = self.chunk_max_samples;
        self.capacity = Some(self.sample_rate as usize * 60);
    }

    /// Publishes PCM before advertising newly complete chunks to the consumer.
    fn push_mono(&self, mono: &[i16]) -> bool {
        let len = mono.len();
        let mut samples = self.samples.lock().unwrap();
        if self.overflowed.load(Ordering::Acquire) {
            return false;
        }
        if self
            .capacity
            .is_some_and(|cap| samples.len().saturating_add(len) > cap)
        {
            self.overflowed.store(true, Ordering::Release);
            return true;
        }
        samples.extend_from_slice(mono);
        drop(samples);
        let total = self.sample_count.fetch_add(len, Ordering::Relaxed) + len;
        if total % (self.sample_rate as usize / 2) < len {
            debug!(
                frames = total,
                seconds = total / self.sample_rate as usize,
                "Recording progress"
            );
        }
        if let Some(ready_chunks) = total.checked_div(self.wake_samples) {
            let previous = self.ready_chunk_count.swap(ready_chunks, Ordering::AcqRel);
            return ready_chunks > previous;
        }
        false
    }
}

impl AudioRecorder {
    /// Create a recorder with the module-owned production chunk policy.
    pub fn with_config(
        config: &AudioConfig,
        chunk_notifier: impl Fn(SessionId) + Send + Sync + 'static,
    ) -> Self {
        let gain = config.mic_gain;
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
            max_chunk_duration_secs = super::MAX_CHUNK_DURATION_SECS,
            max_chunk_size_bytes = super::MAX_CHUNK_SIZE_BYTES,
            "Audio recorder configured"
        );

        AudioRecorder {
            active: None,
            input_device: config.input_device.clone(),
            gain,
            chunk_mode: ChunkMode::Fixed,
            chunk_notifier: Arc::new(chunk_notifier),
        }
    }

    pub(crate) fn input_device(&self) -> Option<&str> {
        self.input_device.as_deref()
    }

    /// Selects the next recording's input without changing the active stream or its buffers.
    pub(crate) fn set_input_device(&mut self, device: Option<String>) {
        self.input_device = device;
    }

    /// Keeps the WAV envelope, but emits bounded 200 ms frames for Realtime.
    pub(crate) fn enable_streaming_frames(&mut self) {
        self.chunk_mode = ChunkMode::Frames;
    }

    /// Detects acoustic endpoints when taking chunks, outside the capture callback and PCM lock.
    pub(crate) fn enable_pause_aware_segments(&mut self) {
        self.chunk_mode = ChunkMode::Pauses;
    }

    pub(crate) fn streaming_overflowed(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.shared.overflowed.load(Ordering::Acquire))
    }

    /// Starts a session, preserving any recording that is already active.
    pub fn start_recording(&mut self, session_id: SessionId) -> RecorderStartOutcome {
        if let Some(active) = &self.active {
            return RecorderStartOutcome::AlreadyRecording {
                requested_session_id: session_id,
                active_session_id: active.session_id,
            };
        }

        match self.try_start_recording(session_id) {
            Ok(active) => {
                self.active = Some(active);
                RecorderStartOutcome::Started { session_id }
            }
            Err(error) => RecorderStartOutcome::Failed {
                session_id,
                error: error.to_string(),
            },
        }
    }

    #[instrument(skip(self))]
    fn try_start_recording(&self, session_id: SessionId) -> Result<ActiveRecording, RecorderError> {
        let host = cpal::default_host();
        let device = resolve_input_device(&host, self.input_device.as_deref())?;
        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        info!(sample_rate, channels, format = ?sample_format, "Starting recording");

        let mut active = ActiveRecording::new(session_id, sample_rate, self.chunk_mode)?;

        let stream = match sample_format {
            cpal::SampleFormat::I16 => {
                let mut callback = active.input_callback::<i16>(
                    channels,
                    self.gain,
                    Arc::clone(&self.chunk_notifier),
                    scale_i16,
                );
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| callback(data),
                    move |err| error!(error = %err, "Stream error"),
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let mut callback = active.input_callback::<f32>(
                    channels,
                    self.gain,
                    Arc::clone(&self.chunk_notifier),
                    scale_f32,
                );
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| callback(data),
                    move |err| error!(error = %err, "Stream error"),
                    None,
                )
            }
            _ => return Err(RecorderError::UnsupportedSampleFormat),
        }?;

        // Enable callbacks immediately before playback. A rejected start owns only
        // this session's state, so a later start cannot receive its stale samples.
        active.shared.recording.store(true, Ordering::Relaxed);
        if let Err(error) = stream.play() {
            active.shared.recording.store(false, Ordering::Relaxed);
            return Err(error.into());
        }
        active.stream = Some(stream);
        info!("Recording started");
        Ok(active)
    }

    /// Returns the next complete live chunk, retaining PCM if encoding fails.
    pub fn take_ready_chunk(&mut self) -> Option<ReadyChunk> {
        self.active.as_mut()?.take_ready_chunk()
    }

    /// Stops the matching session and encodes its remaining complete chunks and tail.
    #[instrument(skip(self))]
    pub fn stop_recording(&mut self, session_id: SessionId) -> RecorderStopOutcome {
        let Some(active) = self.active.take() else {
            return RecorderStopOutcome::NotRecording {
                requested_session_id: session_id,
            };
        };
        if active.session_id != session_id {
            let active_session_id = active.session_id;
            self.active = Some(active);
            return RecorderStopOutcome::StillRecording {
                session_id: active_session_id,
                error: format!(
                    "stop requested for session {}, active session is {}",
                    session_id.0, active_session_id.0
                ),
            };
        }

        let (chunks, warning) = match active.stop() {
            Ok(chunks) => (chunks, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        RecorderStopOutcome::Stopped {
            session_id,
            chunks,
            warning,
        }
    }

    #[cfg(test)]
    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// Cancels the matching session without encoding any remaining audio.
    pub fn cancel_recording(&mut self, session_id: SessionId) -> RecorderCancelOutcome {
        let Some(mut active) = self.active.take() else {
            return RecorderCancelOutcome::NotRecording;
        };
        if active.session_id != session_id {
            let active_session_id = active.session_id;
            self.active = Some(active);
            return RecorderCancelOutcome::SessionMismatch { active_session_id };
        }
        active.shared.recording.store(false, Ordering::Relaxed);
        drop(active.stream.take());
        RecorderCancelOutcome::Cancelled
    }
}

fn scale_i16(average: f32, gain: f32) -> i16 {
    (average * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn scale_f32(average: f32, gain: f32) -> i16 {
    ((average * gain).clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

impl ActiveRecording {
    fn new(session_id: SessionId, sample_rate: u32, mode: ChunkMode) -> Result<Self, ChunkError> {
        let mut buffer = RecordingBuffer::new(sample_rate)?;
        let segmenter = match mode {
            ChunkMode::Fixed => None,
            ChunkMode::Frames => {
                buffer.enable_streaming();
                None
            }
            ChunkMode::Pauses => {
                buffer.wake_samples = (sample_rate as usize / 5).max(1);
                buffer.capacity = Some(sample_rate as usize * 60);
                Some(Segmenter::new(
                    sample_rate,
                    buffer.chunk_max_samples,
                    Segmentation::PauseAware,
                ))
            }
        };
        Ok(Self {
            session_id,
            shared: Arc::new(buffer),
            stream: None,
            flushed_samples: 0,
            segmenter,
            analyzed_samples: 0,
            pending_cut: None,
        })
    }

    /// Shares callback bookkeeping while keeping each input format's numeric conversion explicit.
    /// The scratch allocation is reused, and downmixing happens outside the shared PCM lock.
    fn input_callback<T: Copy + Into<f32>>(
        &self,
        channels: usize,
        gain: f32,
        chunk_notifier: Arc<dyn Fn(SessionId) + Send + Sync>,
        scale: impl Fn(f32, f32) -> i16 + Send + 'static,
    ) -> impl FnMut(&[T]) + Send + 'static {
        let shared = Arc::clone(&self.shared);
        let session_id = self.session_id;
        let mut mono = Vec::new();
        move |data| {
            if !shared.recording.load(Ordering::Relaxed) {
                return;
            }
            mono.clear();
            mono.extend(data.chunks(channels).map(|frame| {
                let average =
                    frame.iter().map(|&sample| sample.into()).sum::<f32>() / channels as f32;
                scale(average, gain)
            }));
            if shared.push_mono(&mono) {
                chunk_notifier(session_id);
            }
        }
    }

    fn take_ready_chunk(&mut self) -> Option<ReadyChunk> {
        let mut count = self.shared.chunk_max_samples;
        if count == 0 {
            return None;
        }
        if let Some(segmenter) = &mut self.segmenter {
            while self.pending_cut.is_none() {
                let pcm = {
                    let buffer = self.shared.samples.lock().unwrap();
                    let offset = self.analyzed_samples - self.flushed_samples;
                    let end = (offset + self.shared.wake_samples).min(buffer.len());
                    buffer[offset..end].to_vec()
                };
                if pcm.is_empty() {
                    return None;
                }
                // A step can cross at most one endpoint. Stop analyzing at that
                // cut so encoding failure needs only one retained boundary.
                let step = (self.shared.wake_samples / 10).max(1).min(count);
                for part in pcm.chunks(step) {
                    let cuts = segmenter.push(part);
                    if let Some(cut) = cuts.first() {
                        debug_assert_eq!(cuts.len(), 1);
                        self.pending_cut = Some((self.analyzed_samples + cut.offset, cut.speech));
                        debug!(reason = ?cut.reason, "Closing recorded recognition segment");
                    }
                    self.analyzed_samples += part.len();
                    if self.pending_cut.is_some() {
                        break;
                    }
                }
            }
            count = self.pending_cut.unwrap().0 - self.flushed_samples;
        } else {
            let flushed_chunk_count = self.flushed_samples / count;
            if self.shared.ready_chunk_count.load(Ordering::Acquire) <= flushed_chunk_count {
                return None;
            }
        }

        let chunk_samples = {
            let buffer = self.shared.samples.lock().unwrap();
            let total_samples = buffer.len();
            if total_samples < count {
                debug!(
                    total_samples,
                    chunk_size = count,
                    "Chunk count is ahead of buffered samples; leaving readiness pending"
                );
                return None;
            }
            buffer[..count].to_vec()
        };

        let chunk = match encode_i16_wav(&chunk_samples, self.shared.sample_rate) {
            Ok(chunk) => chunk.with_speech(self.pending_cut.and_then(|(_, speech)| speech)),
            Err(error) => {
                warn!(error = %error, "Failed to encode in-recording chunk; retaining PCM until stop");
                return None;
            }
        };
        self.shared.samples.lock().unwrap().drain(..count);
        self.flushed_samples += count;
        self.pending_cut = None;
        Some(ReadyChunk {
            session_id: self.session_id,
            chunk,
        })
    }

    fn stop(mut self) -> Result<Vec<WavChunk>, RecorderError> {
        debug!("Stopping recording");
        self.shared.recording.store(false, Ordering::Relaxed);
        if self.stream.is_some() {
            // A live stream can still have callbacks in flight after recording is disabled.
            thread::sleep(Duration::from_millis(200));
        }
        drop(self.stream.take());
        debug!("Stream stopped");

        // Check after callbacks have stopped: the queued readiness notification
        // may not have been handled before the user's stop request.
        if self.shared.overflowed.load(Ordering::Acquire) {
            return Err(RecorderError::StreamingOverflow);
        }

        let samples = std::mem::take(&mut *self.shared.samples.lock().unwrap());
        debug!(samples = samples.len(), "Buffer size");
        if samples.is_empty() {
            return if self.flushed_samples > 0 {
                Ok(Vec::new())
            } else {
                Err(RecorderError::NoAudioData)
            };
        }
        if self.shared.chunk_max_samples == 0 {
            return Ok(Vec::new());
        }
        if let Some(segmenter) = &mut self.segmenter {
            // Readiness may be unhandled at stop. Reuse already analyzed PCM,
            // then process every remaining sample before sealing the tail.
            let mut ends: Vec<_> = self.pending_cut.into_iter().collect();
            ends.extend(
                segmenter
                    .push(&samples[self.analyzed_samples - self.flushed_samples..])
                    .into_iter()
                    .map(|cut| (self.analyzed_samples + cut.offset, cut.speech)),
            );
            if let Some(cut) = segmenter.finish() {
                ends.push((self.flushed_samples + samples.len(), cut.speech));
            }
            let mut start = 0;
            return ends
                .into_iter()
                .map(|(end, speech)| {
                    let end = end - self.flushed_samples;
                    let chunk = encode_i16_wav(&samples[start..end], self.shared.sample_rate);
                    start = end;
                    chunk
                        .map(|chunk| chunk.with_speech(speech))
                        .map_err(Into::into)
                })
                .collect();
        }
        samples
            .chunks(self.shared.chunk_max_samples)
            .map(|samples| encode_i16_wav(samples, self.shared.sample_rate).map_err(Into::into))
            .collect()
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
    #[test]
    fn stop_before_overflow_notification_preserves_failure() {
        let mut recorder = recorder_for_buffer(vec![1; 3200], 3200);
        // A native stop event can arrive before the queued AudioChunkAvailable event.
        recorder
            .active
            .as_ref()
            .unwrap()
            .shared
            .overflowed
            .store(true, Ordering::Release);
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("not stopped")
        };
        assert!(chunks.is_empty());
        assert!(warning.unwrap().contains("capacity"));
    }
    #[test]
    fn streaming_frames_are_small_and_backlog_failure_is_explicit() {
        let mut buffer = super::RecordingBuffer::new(16_000).unwrap();
        buffer.enable_streaming();
        assert_eq!(buffer.chunk_max_samples, 3200);
        assert!(!buffer.push_mono(&[0; 3199]));
        assert!(buffer.push_mono(&[0]));
        assert!(buffer.push_mono(&vec![0; 16_000 * 60]));
        assert!(buffer.overflowed.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(buffer.samples.lock().unwrap().len(), 3200);
    }

    use super::*;

    #[test]
    fn named_device_selection_is_exact_and_deterministic() {
        let devices = || {
            [
                (0, "Built-in Mic".into()),
                (1, "USB Mic".into()),
                (2, "USB Mic".into()),
            ]
        };
        assert_eq!(select_named_device(devices(), "USB Mic").unwrap(), 1);
        assert!(select_named_device(devices(), "usb mic").is_err());
    }

    #[test]
    fn choosing_next_input_preserves_the_active_recording() {
        // Selecting a microphone while recording must retain the current session and its PCM.
        let samples = vec![-71, 280, 1300, -905];
        let mut recorder = recorder_for_buffer(samples.clone(), 16_000);
        recorder.set_input_device(Some("Next Mic".into()));
        assert_eq!(recorder.input_device(), Some("Next Mic"));
        assert!(matches!(
            recorder.start_recording(SessionId(2)),
            RecorderStartOutcome::AlreadyRecording {
                active_session_id: SessionId(1),
                ..
            }
        ));
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("device selection interrupted the active recording");
        };
        assert_eq!(warning, None);
        assert_eq!(decode_chunks(chunks), samples);
        recorder.set_input_device(None);
        assert_eq!(recorder.input_device(), None);
    }

    fn recorder_for_buffer(samples: Vec<i16>, chunk_max_samples: usize) -> AudioRecorder {
        let sample_count = samples.len();
        AudioRecorder {
            active: Some(ActiveRecording {
                session_id: SessionId(1),
                shared: Arc::new(RecordingBuffer {
                    sample_rate: 16000,
                    chunk_max_samples,
                    wake_samples: chunk_max_samples,
                    recording: AtomicBool::new(true),
                    samples: Mutex::new(samples),
                    sample_count: AtomicUsize::new(sample_count),
                    ready_chunk_count: AtomicUsize::new(
                        sample_count.checked_div(chunk_max_samples).unwrap_or(0),
                    ),
                    capacity: None,
                    overflowed: AtomicBool::new(false),
                }),
                stream: None,
                flushed_samples: 0,
                segmenter: None,
                analyzed_samples: 0,
                pending_cut: None,
            }),
            input_device: None,
            gain: 1.0,
            chunk_mode: ChunkMode::Fixed,
            chunk_notifier: Arc::new(|_| {}),
        }
    }

    fn decode_chunks(chunks: impl IntoIterator<Item = WavChunk>) -> Vec<i16> {
        chunks
            .into_iter()
            .flat_map(|chunk| {
                hound::WavReader::new(std::io::Cursor::new(chunk.bytes()))
                    .unwrap()
                    .samples::<i16>()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            })
            .collect()
    }

    fn pause_recorder(rate: u32) -> AudioRecorder {
        let mut recorder = recorder_for_buffer(Vec::new(), rate as usize * 30);
        recorder.active =
            Some(ActiveRecording::new(SessionId(1), rate, ChunkMode::Pauses).unwrap());
        recorder
    }

    #[test]
    fn pause_chunks_preserve_breaths_and_samples_across_drain_patterns() {
        let wav = WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (_, speech) = crate::audio::decode_mono(&wav).unwrap();
        for rate in [16_000, 44_100, 48_000] {
            let voice = crate::audio::PcmResampler::new(16_000, rate)
                .unwrap()
                .push(&speech, true)
                .unwrap();
            for pause_ms in [600, 2000] {
                let mut pcm = voice.clone();
                pcm.extend(vec![0; rate as usize * pause_ms / 1000]);
                pcm.extend_from_slice(&voice);
                let mut boundaries = Vec::new();
                // Include delayed UI consumption and stop before any readiness event is handled.
                for (packet_size, drain) in [
                    (317, true),
                    (rate as usize / 5, true),
                    (pcm.len(), true),
                    (pcm.len(), false),
                ] {
                    let mut recorder = pause_recorder(rate);
                    let mut chunks = Vec::new();
                    for packet in pcm.chunks(packet_size) {
                        recorder.active.as_ref().unwrap().shared.push_mono(packet);
                        if drain {
                            while let Some(ready) = recorder.take_ready_chunk() {
                                assert_eq!(ready.session_id, SessionId(1));
                                chunks.push(ready.chunk);
                            }
                        }
                    }
                    if drain {
                        assert_eq!(chunks.len(), usize::from(pause_ms == 2000));
                    }
                    let RecorderStopOutcome::Stopped {
                        chunks: tail,
                        warning,
                        ..
                    } = recorder.stop_recording(SessionId(1))
                    else {
                        panic!("not stopped")
                    };
                    assert!(warning.is_none());
                    chunks.extend(tail);
                    assert_eq!(chunks.len(), 1 + usize::from(pause_ms == 2000));
                    let lengths: Vec<_> = chunks
                        .iter()
                        .map(|chunk| {
                            let reader =
                                hound::WavReader::new(std::io::Cursor::new(chunk.bytes())).unwrap();
                            assert_eq!(reader.spec().sample_rate, rate);
                            reader.len() as usize
                        })
                        .collect();
                    if pause_ms == 2000 {
                        assert!((rate as usize * 3..rate as usize * 5).contains(&lengths[0]));
                    }
                    assert_eq!(decode_chunks(chunks), pcm);
                    assert!(recorder.take_ready_chunk().is_none());
                    boundaries.push(lengths);
                }
                assert!(boundaries.windows(2).all(|pair| pair[0] == pair[1]));
            }
        }
    }

    #[test]
    fn pause_chunks_keep_the_hard_limit_and_exact_stop_tail() {
        for rate in [16_000, 44_100, 48_000] {
            let limit = rate as usize * 30;
            for length in [limit - 1, limit, limit + 1, rate as usize * 65 + 317] {
                let mut recorder = pause_recorder(rate);
                let mut pcm = vec![0; length];
                *pcm.last_mut().unwrap() = 17;
                let mut chunks = Vec::new();
                for packet in pcm.chunks(rate as usize / 5) {
                    recorder.active.as_ref().unwrap().shared.push_mono(packet);
                    while let Some(ready) = recorder.take_ready_chunk() {
                        assert_eq!(decode_chunks([ready.chunk.clone()]).len(), limit);
                        chunks.push(ready.chunk);
                    }
                }
                assert_eq!(chunks.len(), length / limit);
                let RecorderStopOutcome::Stopped {
                    chunks: tail,
                    warning,
                    ..
                } = recorder.stop_recording(SessionId(1))
                else {
                    panic!("not stopped")
                };
                assert!(warning.is_none());
                assert_eq!(tail.len(), usize::from(length % limit != 0));
                chunks.extend(tail);
                assert_eq!(decode_chunks(chunks), pcm);
            }
        }
    }

    #[test]
    fn pause_encoding_failure_preserves_the_cut_for_retry_stop_or_cancel() {
        let wav = WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (_, voice) = crate::audio::decode_mono(&wav).unwrap();
        let mut pcm = voice.clone();
        pcm.extend(vec![0; 32_000]);
        pcm.extend(voice);
        pcm.extend(vec![0; 32_000]);
        pcm.extend((0..16_000).map(|i| ((i * 7919 % 101) as i16 - 50) * 30));
        for action in ["retry", "stop", "cancel"] {
            let mut recorder = pause_recorder(16_000);
            recorder.active.as_ref().unwrap().shared.push_mono(&pcm);
            Arc::get_mut(&mut recorder.active.as_mut().unwrap().shared)
                .unwrap()
                .sample_rate = 0;
            assert!(recorder.take_ready_chunk().is_none());
            Arc::get_mut(&mut recorder.active.as_mut().unwrap().shared)
                .unwrap()
                .sample_rate = 16_000;
            if action == "cancel" {
                assert_eq!(
                    recorder.cancel_recording(SessionId(1)),
                    RecorderCancelOutcome::Cancelled
                );
                assert!(recorder.take_ready_chunk().is_none());
                assert!(matches!(
                    recorder.stop_recording(SessionId(1)),
                    RecorderStopOutcome::NotRecording { .. }
                ));
                continue;
            }
            let mut chunks = Vec::new();
            if action == "retry" {
                while let Some(ready) = recorder.take_ready_chunk() {
                    chunks.push(ready.chunk);
                }
                assert_eq!(chunks.len(), 2);
            }
            let RecorderStopOutcome::Stopped {
                chunks: tail,
                warning,
                ..
            } = recorder.stop_recording(SessionId(1))
            else {
                panic!("not stopped")
            };
            assert!(warning.is_none());
            chunks.extend(tail);
            assert_eq!(chunks.len(), 3);
            assert_eq!(
                chunks.iter().map(|c| c.speech).collect::<Vec<_>>(),
                [Some(true), Some(true), Some(false)]
            );
            assert_eq!(decode_chunks(chunks), pcm);
        }
    }

    #[test]
    fn pause_recording_keeps_short_wakeups_and_bounded_pcm() {
        let mut recorder = pause_recorder(16_000);
        let shared = &recorder.active.as_ref().unwrap().shared;
        assert!(!shared.push_mono(&[0; 3199]));
        assert!(shared.push_mono(&[0]));
        assert!(shared.push_mono(&vec![0; 60 * 16_000]));
        assert!(recorder.streaming_overflowed());
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("not stopped")
        };
        assert!(chunks.is_empty());
        assert!(warning.unwrap().contains("capacity"));
    }

    #[test]
    fn callbacks_preserve_downmix_gain_clipping_and_stop_gating() {
        // Stereo input and gain clipping must survive sharing the I16/F32 callback pipeline.
        let recorder = recorder_for_buffer(Vec::new(), 4);
        let active = recorder.active.as_ref().unwrap();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let notified = Arc::clone(&notifications);
        let notifier: Arc<dyn Fn(SessionId) + Send + Sync> =
            Arc::new(move |id| notified.lock().unwrap().push(id));
        let mut i16_callback = active.input_callback(2, 2.0, Arc::clone(&notifier), scale_i16);
        let mut f32_callback = active.input_callback(2, 2.0, notifier, scale_f32);
        i16_callback(&[100i16, 300, i16::MAX, i16::MAX]);
        i16_callback(&[i16::MIN, i16::MIN, 100, -100]);
        f32_callback(&[0.25f32, 0.75, -1.0, -1.0, 0.25, -0.25, 0.0, 0.5]);
        active.shared.recording.store(false, Ordering::Relaxed);
        i16_callback(&[1, 1]);
        f32_callback(&[1.0, 1.0]);
        assert_eq!(
            *active.shared.samples.lock().unwrap(),
            [400, i16::MAX, i16::MIN, 0, i16::MAX, -32767, 0, 16383]
        );
        assert_eq!(*notifications.lock().unwrap(), [SessionId(1), SessionId(1)]);
    }

    #[test]
    fn gain_changes_leave_active_audio_unchanged_and_apply_to_new_callbacks() {
        // Editing gain mid-recording must not change the remaining samples in that recording.
        let mut recorder = recorder_for_buffer(Vec::new(), 16);
        let mut old_callback = recorder.active.as_ref().unwrap().input_callback(
            1,
            recorder.gain,
            Arc::clone(&recorder.chunk_notifier),
            scale_i16,
        );
        old_callback(&[1200i16, -3400]);
        recorder.gain = 0.5;
        old_callback(&[5600i16, -7800]);
        let RecorderStopOutcome::Stopped { chunks, .. } = recorder.stop_recording(SessionId(1))
        else {
            panic!("recording did not stop");
        };
        assert_eq!(decode_chunks(chunks), [1200, -3400, 5600, -7800]);

        let next = ActiveRecording::new(SessionId(2), 16_000, ChunkMode::Fixed).unwrap();
        next.shared.recording.store(true, Ordering::Relaxed);
        let mut new_callback = next.input_callback(
            1,
            recorder.gain,
            Arc::clone(&recorder.chunk_notifier),
            scale_i16,
        );
        new_callback(&[1200i16, -3400]);
        recorder.active = Some(next);
        let RecorderStopOutcome::Stopped { chunks, .. } = recorder.stop_recording(SessionId(2))
        else {
            panic!("next recording did not stop");
        };
        assert_eq!(decode_chunks(chunks), [600, -1700]);
    }

    #[test]
    fn test_take_ready_chunk_flushes_each_ready_chunk() {
        let samples: Vec<i16> = (0..30).collect();
        let mut recorder = recorder_for_buffer(samples, 10);

        let first = recorder.take_ready_chunk();
        let second = recorder.take_ready_chunk();
        let third = recorder.take_ready_chunk();
        let fourth = recorder.take_ready_chunk();

        assert!(first.is_some());
        assert!(second.is_some());
        assert!(third.is_some());
        assert!(fourth.is_none());
        assert_eq!(recorder.active.as_ref().unwrap().flushed_samples, 30);
        assert!(
            recorder
                .active
                .as_ref()
                .unwrap()
                .shared
                .samples
                .lock()
                .unwrap()
                .is_empty(),
            "flushed samples should be released from memory"
        );

        assert_eq!(
            decode_chunks(
                [first, second, third]
                    .into_iter()
                    .flatten()
                    .map(|ready| ready.chunk)
            ),
            (0..30).collect::<Vec<_>>()
        );
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("expected fully flushed stop");
        };
        assert!(chunks.is_empty());
        assert!(warning.is_none());
    }

    #[test]
    fn each_new_chunk_boundary_is_reported_to_the_callback() {
        let mut buffer = RecordingBuffer::new(16_000).unwrap();
        buffer.chunk_max_samples = 10;
        buffer.wake_samples = 10;
        assert!(!buffer.push_mono(&[1; 5]));
        assert!(buffer.push_mono(&[1; 5]));
        assert!(buffer.push_mono(&[2; 10]));
        assert_eq!(buffer.ready_chunk_count.load(Ordering::Acquire), 2);
    }

    #[test]
    fn stop_time_chunk_catch_up_clears_readiness_and_stops_polling() {
        let samples: Vec<i16> = (0..25).collect();
        let mut recorder = recorder_for_buffer(samples, 10);

        let result = recorder.stop_recording(SessionId(1));

        let RecorderStopOutcome::Stopped { chunks, .. } = result else {
            panic!("expected stop-time chunk catch-up");
        };
        assert_eq!(chunks.len(), 3);
        assert_eq!(decode_chunks(chunks), (0..25).collect::<Vec<_>>());
        assert!(!recorder.is_recording());
        assert!(recorder.take_ready_chunk().is_none());
    }

    #[test]
    fn test_stop_recording_catches_up_after_live_chunk() {
        let samples: Vec<i16> = (0..35).collect();
        let mut recorder = recorder_for_buffer(samples, 10);

        let first = recorder.take_ready_chunk();
        let first = first.unwrap();

        let result = recorder.stop_recording(SessionId(1));

        match result {
            RecorderStopOutcome::Stopped { chunks, .. } => {
                assert_eq!(chunks.len(), 3);
                assert_eq!(
                    decode_chunks(std::iter::once(first.chunk).chain(chunks)),
                    (0..35).collect::<Vec<_>>()
                );
            }
            _ => panic!("expected remaining audio to be chunked"),
        }
    }

    #[test]
    fn stop_result_reports_not_recording_without_guessing() {
        let mut recorder = recorder_for_buffer(Vec::new(), 10);
        // A session stopped before receiving input still becomes idle after reporting no audio.
        assert!(matches!(
            recorder.stop_recording(SessionId(1)),
            RecorderStopOutcome::Stopped {
                warning: Some(_),
                ..
            }
        ));

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
        assert_eq!(
            recorder.cancel_recording(SessionId(2)),
            RecorderCancelOutcome::SessionMismatch {
                active_session_id: SessionId(1)
            }
        );
        assert!(matches!(
            recorder.start_recording(SessionId(2)),
            RecorderStartOutcome::AlreadyRecording {
                active_session_id: SessionId(1),
                ..
            }
        ));
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("active session must remain stoppable");
        };
        assert!(warning.is_none());
        assert_eq!(decode_chunks(chunks), [1, 2, 3]);
        assert!(matches!(
            recorder.stop_recording(SessionId(1)),
            RecorderStopOutcome::NotRecording { .. }
        ));
    }

    #[test]
    fn cancel_clears_recorder_state() {
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 10);

        assert_eq!(
            recorder.cancel_recording(SessionId(1)),
            RecorderCancelOutcome::Cancelled
        );
        assert!(!recorder.is_recording());
        assert_eq!(
            recorder.cancel_recording(SessionId(1)),
            RecorderCancelOutcome::NotRecording
        );
        assert!(recorder.take_ready_chunk().is_none());
    }

    #[test]
    fn too_small_chunk_capacity_produces_no_stop_chunk() {
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 0);

        let RecorderStopOutcome::Stopped { chunks, .. } = recorder.stop_recording(SessionId(1))
        else {
            panic!("expected stopped outcome");
        };

        assert!(chunks.is_empty());
    }

    #[test]
    fn failed_live_encoding_retains_pcm_for_stop_recovery() {
        // A failed live encoder must not consume audio that stop-time encoding can recover.
        let mut recorder = recorder_for_buffer(vec![1, 2, 3], 2);
        let active = recorder.active.as_mut().unwrap();
        Arc::get_mut(&mut active.shared).unwrap().sample_rate = 0;
        assert!(active.take_ready_chunk().is_none());
        Arc::get_mut(&mut active.shared).unwrap().sample_rate = 16_000;
        let RecorderStopOutcome::Stopped {
            chunks, warning, ..
        } = recorder.stop_recording(SessionId(1))
        else {
            panic!("expected recovered stop output");
        };
        assert!(warning.is_none());
        assert_eq!(decode_chunks(chunks), [1, 2, 3]);
    }
}
