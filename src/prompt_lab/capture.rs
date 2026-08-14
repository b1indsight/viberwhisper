use std::fmt;
use std::io::{BufWriter, Cursor};
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

use super::dataset::{CaptureTranscription, DatasetError, DatasetStore, SampleReservation};
use crate::audio::WavChunk;
use crate::session::SessionId;

#[derive(Debug)]
pub(crate) enum CaptureError {
    Dataset(DatasetError),
    Io(std::io::Error),
    Wav(hound::Error),
    Clock(SystemTimeError),
    TimestampOverflow,
    ActiveSession(SessionId),
    NoActiveSession(SessionId),
    SessionMismatch {
        requested: SessionId,
        active: SessionId,
    },
    WorkerClosed(SessionId),
    WorkerPanicked,
    NoAudioChunks,
    UnsupportedFormat(WavSpec),
    FormatChanged {
        expected: WavSpec,
        actual: WavSpec,
    },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dataset(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "capture I/O failed: {error}"),
            Self::Wav(error) => write!(formatter, "capture WAV failed: {error}"),
            Self::Clock(error) => write!(formatter, "capture clock failed: {error}"),
            Self::TimestampOverflow => formatter.write_str("capture timestamp is too large"),
            Self::ActiveSession(session_id) => {
                write!(
                    formatter,
                    "capture session {} is already active",
                    session_id.0
                )
            }
            Self::NoActiveSession(session_id) => {
                write!(formatter, "no capture session for session {}", session_id.0)
            }
            Self::SessionMismatch { requested, active } => write!(
                formatter,
                "capture request for session {} does not match active session {}",
                requested.0, active.0
            ),
            Self::WorkerClosed(session_id) => write!(
                formatter,
                "capture worker closed for session {}",
                session_id.0
            ),
            Self::WorkerPanicked => formatter.write_str("capture worker panicked"),
            Self::NoAudioChunks => formatter.write_str("capture received no audio chunks"),
            Self::UnsupportedFormat(spec) => {
                write!(formatter, "capture does not support WAV format {spec:?}")
            }
            Self::FormatChanged { expected, actual } => write!(
                formatter,
                "capture WAV format changed from {expected:?} to {actual:?}"
            ),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dataset(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Wav(error) => Some(error),
            Self::Clock(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DatasetError> for CaptureError {
    fn from(error: DatasetError) -> Self {
        Self::Dataset(error)
    }
}

impl From<std::io::Error> for CaptureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<hound::Error> for CaptureError {
    fn from(error: hound::Error) -> Self {
        Self::Wav(error)
    }
}

/// Owns at most one dataset archive because the recording state machine owns at most one session.
pub(crate) struct PromptLabCapture {
    store: Arc<DatasetStore>,
    active: Option<CaptureSession>,
}

impl PromptLabCapture {
    pub(crate) fn new(store: Arc<DatasetStore>) -> Self {
        Self {
            store,
            active: None,
        }
    }

    pub(crate) fn start(
        &mut self,
        session_id: SessionId,
        created_at_unix_ms: u64,
    ) -> Result<(), CaptureError> {
        if let Some(active) = &self.active {
            return Err(CaptureError::ActiveSession(active.session_id));
        }
        let reservation = self.store.reserve_sample(created_at_unix_ms)?;
        let audio_path = reservation.audio_path.clone();
        let (sender, receiver) = mpsc::sync_channel(2);
        let worker = thread::Builder::new()
            .name(format!("prompt-lab-archive-{}", session_id.0))
            .spawn(move || archive_chunks(audio_path, receiver))?;
        self.active = Some(CaptureSession {
            session_id,
            store: Arc::clone(&self.store),
            reservation,
            sender: Some(sender),
            worker,
        });
        Ok(())
    }

    pub(crate) fn start_now(&mut self, session_id: SessionId) -> Result<(), CaptureError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(CaptureError::Clock)?;
        let created_at_unix_ms =
            u64::try_from(elapsed.as_millis()).map_err(|_| CaptureError::TimestampOverflow)?;
        self.start(session_id, created_at_unix_ms)
    }

    pub(crate) fn push(
        &mut self,
        session_id: SessionId,
        chunk: WavChunk,
    ) -> Result<(), CaptureError> {
        let active = self
            .active
            .as_mut()
            .ok_or(CaptureError::NoActiveSession(session_id))?;
        if active.session_id != session_id {
            return Err(CaptureError::SessionMismatch {
                requested: session_id,
                active: active.session_id,
            });
        }
        let sender = active
            .sender
            .as_ref()
            .ok_or(CaptureError::WorkerClosed(session_id))?;
        sender
            .send(chunk)
            .map_err(|_| CaptureError::WorkerClosed(session_id))
    }

    pub(crate) fn take(&mut self, session_id: SessionId) -> Result<CaptureSession, CaptureError> {
        let active = self
            .active
            .as_ref()
            .ok_or(CaptureError::NoActiveSession(session_id))?;
        if active.session_id != session_id {
            return Err(CaptureError::SessionMismatch {
                requested: session_id,
                active: active.session_id,
            });
        }
        Ok(self.active.take().expect("active capture was checked"))
    }

    pub(crate) fn cancel(&mut self, session_id: SessionId) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.session_id == session_id)
        {
            self.active.take();
        }
    }
}

#[derive(Debug)]
pub(crate) struct CaptureSession {
    session_id: SessionId,
    store: Arc<DatasetStore>,
    reservation: SampleReservation,
    sender: Option<SyncSender<WavChunk>>,
    worker: JoinHandle<Result<(), CaptureError>>,
}

#[derive(Debug)]
pub(crate) struct CapturedSample {
    pub(crate) id: String,
    pub(crate) audio_path: std::path::PathBuf,
    pub(crate) sidecar_path: std::path::PathBuf,
}

impl CaptureSession {
    pub(crate) fn finish(
        mut self,
        transcription: CaptureTranscription,
    ) -> Result<CapturedSample, CaptureError> {
        self.sender.take();
        self.worker
            .join()
            .map_err(|_| CaptureError::WorkerPanicked)??;
        let sample = self
            .store
            .complete_capture(self.reservation, transcription)?;
        Ok(CapturedSample {
            audio_path: self.store.root().join(&sample.audio.path),
            sidecar_path: self.store.samples_dir().join(format!("{}.json", sample.id)),
            id: sample.id,
        })
    }
}

fn archive_chunks(
    path: std::path::PathBuf,
    receiver: mpsc::Receiver<WavChunk>,
) -> Result<(), CaptureError> {
    let mut writer: Option<WavWriter<BufWriter<std::fs::File>>> = None;
    let mut expected_spec = None;
    for chunk in receiver {
        let mut reader = WavReader::new(Cursor::new(chunk.shared_bytes()))?;
        let spec = reader.spec();
        if spec.sample_format != SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err(CaptureError::UnsupportedFormat(spec));
        }
        if let Some(expected) = expected_spec {
            if expected != spec {
                return Err(CaptureError::FormatChanged {
                    expected,
                    actual: spec,
                });
            }
        } else {
            expected_spec = Some(spec);
            writer = Some(WavWriter::create(&path, spec)?);
        }
        let output = writer
            .as_mut()
            .expect("writer is created for the first chunk");
        for sample in reader.samples::<i16>() {
            output.write_sample(sample?)?;
        }
    }
    writer.ok_or(CaptureError::NoAudioChunks)?.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use hound::WavReader;
    use tempfile::tempdir;

    use super::*;
    use crate::audio::chunk::encode_i16_wav;
    use crate::prompt_lab::{CaptureTranscription, DatasetStore, SttSnapshot};

    fn snapshot() -> SttSnapshot {
        SttSnapshot {
            endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
            model: "whisper-test".to_string(),
            language: Some("zh".to_string()),
            temperature: 0.0,
            prompt: Some("词汇提示".to_string()),
        }
    }

    #[test]
    fn archives_all_session_chunks_into_one_sample_wav() {
        let directory = tempdir().unwrap();
        let store = Arc::new(DatasetStore::open_or_create(directory.path().join("lab")).unwrap());
        let mut capture = PromptLabCapture::new(store.clone());
        capture.start(SessionId(7), 42).unwrap();
        capture
            .push(SessionId(7), encode_i16_wav(&[1, 2], 16_000).unwrap())
            .unwrap();
        capture
            .push(SessionId(7), encode_i16_wav(&[3, 4], 16_000).unwrap())
            .unwrap();

        let session = capture.take(SessionId(7)).unwrap();
        let sample = session
            .finish(CaptureTranscription::success("结果", snapshot()))
            .unwrap();

        let bytes = std::fs::read(&sample.audio_path).unwrap();
        let samples = WavReader::new(Cursor::new(bytes))
            .unwrap()
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(samples, [1, 2, 3, 4]);
    }

    #[test]
    fn rejects_chunks_and_take_for_another_session() {
        let directory = tempdir().unwrap();
        let store = Arc::new(DatasetStore::open_or_create(directory.path().join("lab")).unwrap());
        let mut capture = PromptLabCapture::new(store);
        capture.start(SessionId(7), 42).unwrap();

        assert!(
            capture
                .push(SessionId(8), encode_i16_wav(&[1], 16_000).unwrap())
                .unwrap_err()
                .to_string()
                .contains("session 8")
        );
        assert!(
            capture
                .take(SessionId(8))
                .unwrap_err()
                .to_string()
                .contains("session 8")
        );
    }

    #[test]
    fn empty_archive_cannot_publish_a_sample() {
        let directory = tempdir().unwrap();
        let store = Arc::new(DatasetStore::open_or_create(directory.path().join("lab")).unwrap());
        let mut capture = PromptLabCapture::new(store);
        capture.start(SessionId(7), 42).unwrap();

        let error = capture
            .take(SessionId(7))
            .unwrap()
            .finish(CaptureTranscription::failed("no chunks", snapshot()))
            .unwrap_err();

        assert!(error.to_string().contains("no audio chunks"));
    }
}
