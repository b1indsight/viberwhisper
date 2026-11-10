//! Session orchestrator for end-to-end stream recognition.
//!
//! `SessionOrchestrator` owns recording-session chunk tracking, background transcription,
//! convergence wait, and result merging.

use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::config::SecretValue;
use crate::audio::{
    WavChunk,
    segment::{Segmentation, Segmenter},
};
use crate::dictation::{
    DictationWorker, Progress, RealtimeSession, Transcript, config::DictationConfig,
};
use crate::session::SessionId;
use crate::text::merge_texts;
use crate::transcriber::Transcriber;
// Re-exported so callers can keep using `core::orchestrator::TranscribeError`.
pub use crate::transcriber::TranscribeError;

// The STT retry window is kept below this deadline so a tail chunk can normally converge on stop.
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(30);

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorConfig {
    language: Option<String>,
    convergence_timeout: Duration,
}

impl OrchestratorConfig {
    pub(crate) fn new(language: Option<String>) -> Self {
        Self {
            language,
            convergence_timeout: CONVERGENCE_TIMEOUT,
        }
    }
}

/// A start request rejected because another session is still active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStartError {
    pub requested: SessionId,
    pub active: SessionId,
}

impl fmt::Display for SessionStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot start session {} while session {} is active",
            self.requested.0, self.active.0
        )
    }
}

impl std::error::Error for SessionStartError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRoutingError {
    NoActiveSession {
        requested: SessionId,
    },
    SessionMismatch {
        requested: SessionId,
        active: SessionId,
    },
}

impl fmt::Display for SessionRoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoActiveSession { requested } => {
                write!(f, "no active session for request {}", requested.0)
            }
            Self::SessionMismatch { requested, active } => write!(
                f,
                "request for session {} does not match active session {}",
                requested.0, active.0
            ),
        }
    }
}

impl std::error::Error for SessionRoutingError {}

/// Error returned by `SessionOrchestrator::finish_session`.
#[derive(Debug)]
pub enum SessionError {
    Routing(SessionRoutingError),
    /// No chunks were recorded (recording was too short to produce any chunk).
    NoChunks,
    /// Some chunks failed but others succeeded; `partial_text` contains
    /// the merged result of the successful chunks.
    PartialFailure {
        errors: Vec<(usize, TranscribeError)>,
        partial_text: String,
    },
    /// Input integrity failed; only the accepted continuous prefix can be delivered.
    InputFailure {
        error: String,
        partial_text: String,
    },
    /// `wait_for_convergence` timed out; `partial_text` contains the merged
    /// result of chunks that completed before the deadline.
    ConvergenceTimeout {
        pending_count: usize,
        partial_text: String,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Routing(error) => write!(f, "{error}"),
            SessionError::NoChunks => write!(f, "No audio chunks recorded"),
            SessionError::InputFailure {
                error,
                partial_text,
            } => write!(
                f,
                "Audio input failed: {error}; partial text: {partial_text:?}"
            ),
            SessionError::PartialFailure {
                errors,
                partial_text,
            } => write!(
                f,
                "{} chunk(s) failed; partial text: {:?}",
                errors.len(),
                partial_text
            ),
            SessionError::ConvergenceTimeout {
                pending_count,
                partial_text,
            } => write!(
                f,
                "Convergence timeout: {} audio work item(s) still pending; partial text: {:?}",
                pending_count, partial_text
            ),
        }
    }
}

// ─── Internal types ───────────────────────────────────────────────────────────

#[derive(Debug)]
enum ChunkState {
    /// Produced and queued; not yet picked up by the worker.
    Flushed,
    /// Worker is transcribing this chunk.
    Uploading,
    /// Successfully transcribed.
    Transcribed(String),
    /// Transcription failed (all retries exhausted, or timeout).
    Failed(TranscribeError),
}

impl ChunkState {
    fn is_terminal(&self) -> bool {
        matches!(self, ChunkState::Transcribed(_) | ChunkState::Failed(_))
    }
}

struct ChunkEntry {
    index: usize,
    state: ChunkState,
}

enum WorkerMsg {
    Chunk { index: usize, chunk: WavChunk },
    Frame { chunk: WavChunk },
}

enum WorkerEvent {
    UploadStarted {
        index: usize,
    },
    Completed {
        index: usize,
        result: Result<String, TranscribeError>,
    },
}

enum SessionBackend {
    Http {
        transcriber: Arc<dyn Transcriber>,
        config: OrchestratorConfig,
        segmentation: Segmentation,
        notify: Option<Arc<dyn Fn(SessionId) + Send + Sync>>,
    },
    Realtime(DictationWorker),
}

struct ActiveSessionInner {
    session_id: SessionId,
    worker: SessionWorker,
}

enum SessionWorker {
    Http(HttpSession),
    Realtime(RealtimeSession),
}

impl SessionWorker {
    fn abort(self) {
        match self {
            Self::Http(session) => session.cancelled.cancel(),
            Self::Realtime(session) => session.abort(),
        }
    }
}

/// Owns session routing and lifecycle for both HTTP chunks and Realtime audio.
pub struct SessionOrchestrator {
    backend: SessionBackend,
    inner: Mutex<Option<ActiveSessionInner>>,
}

impl SessionOrchestrator {
    pub fn new(transcriber: Arc<dyn Transcriber>, config: OrchestratorConfig) -> Self {
        Self {
            backend: SessionBackend::Http {
                transcriber,
                config,
                segmentation: Segmentation::Fixed,
                notify: None,
            },
            inner: Mutex::new(None),
        }
    }

    /// Live microphone input consists of bounded 200 ms WAV frames, not complete STT segments.
    pub(crate) fn new_live_http(
        transcriber: Arc<dyn Transcriber>,
        config: OrchestratorConfig,
        notify: impl Fn(SessionId) + Send + Sync + 'static,
    ) -> Self {
        Self {
            backend: SessionBackend::Http {
                transcriber,
                config,
                segmentation: Segmentation::PauseAware,
                notify: Some(Arc::new(notify)),
            },
            inner: Mutex::new(None),
        }
    }

    pub(crate) fn new_realtime(
        settings: DictationConfig,
        key: Option<SecretValue>,
        memory_path: Option<std::path::PathBuf>,
        language: Option<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            backend: SessionBackend::Realtime(DictationWorker::new(
                settings,
                key,
                memory_path,
                language,
            )?),
            inner: Mutex::new(None),
        })
    }

    pub fn start_session(&self, session_id: SessionId) -> Result<(), SessionStartError> {
        self.start_session_with_progress(session_id, None)
    }

    /// Desktop callers may observe provisional text without changing final-result ownership.
    pub(crate) fn start_session_with_progress(
        &self,
        session_id: SessionId,
        progress: Option<Progress>,
    ) -> Result<(), SessionStartError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = inner.as_ref() {
            return Err(SessionStartError {
                requested: session_id,
                active: active.session_id,
            });
        }
        let worker = match &self.backend {
            SessionBackend::Http {
                transcriber,
                config,
                segmentation,
                notify,
            } => {
                let notify = notify.clone().map(|notify| {
                    Arc::new(move || notify(session_id)) as Arc<dyn Fn() + Send + Sync>
                });
                SessionWorker::Http(HttpSession::new(
                    Arc::clone(transcriber),
                    config.clone(),
                    *segmentation,
                    notify,
                    progress.map_or_else(CancellationToken::new, |progress| {
                        progress.cancelled.clone()
                    }),
                ))
            }
            SessionBackend::Realtime(worker) => {
                SessionWorker::Realtime(worker.start_session(progress))
            }
        };
        *inner = Some(ActiveSessionInner { session_id, worker });
        info!(session_id = session_id.0, "Session started");
        Ok(())
    }

    pub fn on_chunk_ready(&self, session_id: SessionId, chunk: WavChunk) {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.as_mut().filter(|s| s.session_id == session_id) else {
            warn!(
                session_id = session_id.0,
                "Rejecting chunk without matching active session"
            );
            return;
        };
        match &mut session.worker {
            SessionWorker::Http(session) => session.on_chunk_ready(session_id, chunk),
            SessionWorker::Realtime(session) => session.on_chunk_ready(chunk),
        }
    }

    /// Detach only the requested session; stale stop/cancel events leave the active one intact.
    fn take_session(&self, session_id: SessionId) -> Result<SessionWorker, SessionRoutingError> {
        let mut inner = self.inner.lock().unwrap();
        let active = inner.as_ref().ok_or(SessionRoutingError::NoActiveSession {
            requested: session_id,
        })?;
        if active.session_id != session_id {
            return Err(SessionRoutingError::SessionMismatch {
                requested: session_id,
                active: active.session_id,
            });
        }
        Ok(inner.take().unwrap().worker)
    }

    /// Close input and collect the selected worker's completed or partial result.
    pub fn finish_session(&self, session_id: SessionId) -> Result<String, SessionError> {
        self.finish_for_delivery(session_id)
            .map(|transcript| transcript.text)
    }

    /// Desktop finalization retains the optional, single-use confirmation capability.
    pub(crate) fn finish_for_delivery(
        &self,
        session_id: SessionId,
    ) -> Result<Transcript, SessionError> {
        match self
            .take_session(session_id)
            .map_err(SessionError::Routing)?
        {
            SessionWorker::Http(session) => session.finish().map(Transcript::from),
            SessionWorker::Realtime(session) => session.finish(),
        }
    }

    pub fn abort_session(&self, session_id: SessionId) -> Result<(), SessionRoutingError> {
        self.take_session(session_id)?.abort();
        info!(session_id = session_id.0, "Session aborted");
        Ok(())
    }

    pub(crate) fn recording_error(&self, session_id: SessionId) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        match &inner
            .as_ref()
            .filter(|s| s.session_id == session_id)?
            .worker
        {
            SessionWorker::Realtime(session) => session.recording_error(),
            SessionWorker::Http(session) => session.input.failure.lock().unwrap().clone(),
        }
    }

    pub(crate) fn fail_session(&self, session_id: SessionId, error: &str) {
        let inner = self.inner.lock().unwrap();
        if let Some(active) = inner.as_ref().filter(|s| s.session_id == session_id) {
            match &active.worker {
                SessionWorker::Realtime(session) => session.fail(error),
                SessionWorker::Http(session)
                    if session.segmentation == Segmentation::PauseAware =>
                {
                    session.input.fail(error)
                }
                SessionWorker::Http(_) => {}
            }
        }
    }

    /// Stop recording and persist accepted Realtime feedback during normal exit.
    pub(crate) fn shutdown(&self) {
        if let Some(session) = self.inner.lock().unwrap().take() {
            session.worker.abort();
        }
        if let SessionBackend::Realtime(worker) = &self.backend {
            worker.shutdown();
        }
    }
}

impl Drop for SessionOrchestrator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Shared sticky input fault: reject subsequent frames and wake the UI even without more audio.
#[derive(Default)]
struct HttpInputStatus {
    failure: Mutex<Option<String>>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl HttpInputStatus {
    fn fail(&self, error: &str) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_some() {
            return;
        }
        *failure = Some(error.to_owned());
        drop(failure);
        if let Some(notify) = &self.notify {
            notify();
        }
    }
}

struct HttpSession {
    chunk_entries: Vec<ChunkEntry>,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    result_rx: mpsc::Receiver<WorkerEvent>,
    worker: thread::JoinHandle<()>,
    next_index: usize,
    cancelled: CancellationToken,
    config: OrchestratorConfig,
    segmentation: Segmentation,
    input: Arc<HttpInputStatus>,
}

impl HttpSession {
    fn new(
        transcriber: Arc<dyn Transcriber>,
        config: OrchestratorConfig,
        segmentation: Segmentation,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
        cancelled: CancellationToken,
    ) -> Self {
        let capacity = if segmentation == Segmentation::PauseAware {
            300
        } else {
            2
        };
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<WorkerMsg>(capacity);
        let (result_tx, result_rx) = mpsc::channel::<WorkerEvent>();
        let worker_cancelled = cancelled.clone();
        let input = Arc::new(HttpInputStatus {
            failure: Mutex::new(None),
            notify,
        });
        let worker_input = Arc::clone(&input);
        let worker = thread::spawn(move || {
            if segmentation == Segmentation::Fixed {
                worker_loop(chunk_rx, result_tx, transcriber, worker_cancelled);
            } else if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                live_worker_loop(
                    chunk_rx,
                    result_tx,
                    transcriber,
                    worker_cancelled,
                    &worker_input,
                );
            }))
            .is_err()
            {
                worker_input.fail("HTTP audio worker panicked");
            }
        });
        Self {
            chunk_entries: Vec::new(),
            chunk_tx,
            result_rx,
            worker,
            next_index: 0,
            cancelled,
            config,
            segmentation,
            input,
        }
    }

    fn on_chunk_ready(&mut self, session_id: SessionId, chunk: WavChunk) {
        if self.segmentation == Segmentation::PauseAware {
            if self.input.failure.lock().unwrap().is_some() {
                return;
            }
            if let Err(error) = self.chunk_tx.try_send(WorkerMsg::Frame { chunk }) {
                self.input.fail(match error {
                    mpsc::TrySendError::Full(_) => "HTTP audio input queue full",
                    mpsc::TrySendError::Disconnected(_) => "HTTP audio worker unavailable",
                });
            }
            while let Ok(event) = self.result_rx.try_recv() {
                apply_worker_event(&mut self.chunk_entries, event);
            }
            return;
        }
        let session = self;
        let index = session.next_index;
        session.next_index += 1;

        let state = if let Err(e) = session.chunk_tx.try_send(WorkerMsg::Chunk { index, chunk }) {
            let message = match e {
                mpsc::TrySendError::Full(_) => "worker queue full",
                mpsc::TrySendError::Disconnected(_) => "worker channel closed",
            };
            error!(
                session_id = session_id.0,
                index,
                error = message,
                "Failed to enqueue chunk; marking as failed"
            );
            ChunkState::Failed(TranscribeError::Network(message.to_string()))
        } else {
            info!(index = index, "Chunk enqueued for background transcription");
            ChunkState::Flushed
        };
        session.chunk_entries.push(ChunkEntry { index, state });

        while let Ok(event) = session.result_rx.try_recv() {
            apply_worker_event(&mut session.chunk_entries, event);
        }
    }

    fn finish(self) -> Result<String, SessionError> {
        let session = self;
        if session.segmentation == Segmentation::Fixed && session.next_index == 0 {
            // Closing the channel lets the idle worker exit immediately.
            drop(session.chunk_tx);
            let _ = session.worker.join();
            return Err(SessionError::NoChunks);
        }

        // Closing the sender is non-blocking. The receiver still drains every
        // queued chunk before its iterator ends, so the convergence deadline
        // covers the entire shutdown rather than starting after a blocking send.
        drop(session.chunk_tx);

        let mut chunk_entries = session.chunk_entries;
        let result_rx = session.result_rx;
        let worker = session.worker;
        let deadline = Instant::now() + session.config.convergence_timeout;
        let mut timed_out = false;
        let mut pending_count = 0;

        loop {
            // Fixed input registers every chunk at intake. Live input may still
            // have an unregistered tail, so only worker-channel EOF proves completion.
            if session.segmentation == Segmentation::Fixed
                && chunk_entries.iter().all(|entry| entry.state.is_terminal())
            {
                break;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            match result_rx.recv_timeout(remaining) {
                Ok(event) => apply_worker_event(&mut chunk_entries, event),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    session.cancelled.cancel();
                    pending_count = chunk_entries
                        .iter()
                        .filter(|entry| !entry.state.is_terminal())
                        .count()
                        + usize::from(session.segmentation == Segmentation::PauseAware);
                    warn!(
                        pending_count = pending_count,
                        "Convergence timeout; marking pending chunks as Failed(Timeout)"
                    );
                    for entry in &mut chunk_entries {
                        if !entry.state.is_terminal() {
                            entry.state = ChunkState::Failed(TranscribeError::Timeout);
                        }
                    }
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let error =
                        TranscribeError::Network("worker result channel closed".to_string());
                    for entry in &mut chunk_entries {
                        if !entry.state.is_terminal() {
                            entry.state = ChunkState::Failed(error.clone());
                        }
                    }
                    break;
                }
            }
        }

        if !timed_out {
            // The worker owns the only result sender. Join also waits for its
            // panic guard to record any input fault after that sender is dropped.
            let _ = worker.join();
        } else {
            // Drop the handle without joining — the worker may still be mid-request.
            // It only owns the result sender, so it cannot retain or mutate `chunk_entries`.
            drop(worker);
        }

        if timed_out {
            let texts = collect_transcribed_texts(&chunk_entries);
            return Err(SessionError::ConvergenceTimeout {
                pending_count,
                partial_text: merge_texts(&texts, session.config.language.clone()),
            });
        }

        if let Some(error) = session.input.failure.lock().unwrap().clone() {
            return Err(SessionError::InputFailure {
                error,
                partial_text: merge_texts(
                    &collect_transcribed_texts(&chunk_entries),
                    session.config.language,
                ),
            });
        }
        if chunk_entries.is_empty() {
            return Err(SessionError::NoChunks);
        }
        collect_results(&chunk_entries, session.config.language.clone())
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

fn worker_loop(
    rx: mpsc::Receiver<WorkerMsg>,
    result_tx: mpsc::Sender<WorkerEvent>,
    transcriber: Arc<dyn Transcriber>,
    cancelled: CancellationToken,
) {
    for msg in rx {
        match msg {
            WorkerMsg::Chunk { index, chunk } => {
                if cancelled.is_cancelled() {
                    continue;
                }
                transcribe_chunk(index, &chunk, &result_tx, transcriber.as_ref(), &cancelled);
            }
            WorkerMsg::Frame { .. } => unreachable!("fixed worker accepts whole chunks"),
        }
    }
    debug!("Worker channel closed, exiting");
}

fn transcribe_chunk(
    index: usize,
    chunk: &WavChunk,
    tx: &mpsc::Sender<WorkerEvent>,
    transcriber: &dyn Transcriber,
    cancelled: &CancellationToken,
) {
    if cancelled.is_cancelled() {
        return;
    }
    let _ = tx.send(WorkerEvent::UploadStarted { index });
    debug!(index, bytes = chunk.len(), "Worker transcribing chunk");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transcriber.transcribe(chunk)
    }))
    .unwrap_or_else(|_| Err(TranscribeError::Network("transcriber panicked".into())));
    let _ = tx.send(WorkerEvent::Completed { index, result });
}

/// PCM and endpoint state stay on the existing serial HTTP worker, never on the callback/UI.
struct HttpAudio {
    rate: u32,
    segmenter: Segmenter,
    samples: Vec<i16>,
    index: usize,
}

impl HttpAudio {
    fn new(rate: u32) -> anyhow::Result<Self> {
        let limit = crate::audio::max_frames_per_chunk(hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        })? as usize;
        anyhow::ensure!(limit > 0, "no capacity for HTTP audio");
        Ok(Self {
            rate,
            segmenter: Segmenter::new(rate, limit, Segmentation::PauseAware),
            samples: Vec::new(),
            index: 0,
        })
    }

    fn push(
        &mut self,
        pcm: &[i16],
        tx: &mpsc::Sender<WorkerEvent>,
        transcriber: &dyn Transcriber,
        cancelled: &CancellationToken,
    ) -> anyhow::Result<()> {
        let mut start = 0;
        for cut in self.segmenter.push(pcm) {
            self.samples.extend_from_slice(&pcm[start..cut.offset]);
            debug!(reason = ?cut.reason, index = self.index, samples = self.samples.len(), "Closing HTTP recognition segment");
            self.submit(tx, transcriber, cancelled)?;
            if cancelled.is_cancelled() {
                return Ok(());
            }
            start = cut.offset;
        }
        self.samples.extend_from_slice(&pcm[start..]);
        Ok(())
    }

    fn submit(
        &mut self,
        tx: &mpsc::Sender<WorkerEvent>,
        transcriber: &dyn Transcriber,
        cancelled: &CancellationToken,
    ) -> anyhow::Result<()> {
        if cancelled.is_cancelled() {
            return Ok(());
        }
        let chunk = crate::audio::chunk::encode_i16_wav(&self.samples, self.rate)?;
        transcribe_chunk(self.index, &chunk, tx, transcriber, cancelled);
        self.samples.clear();
        self.index += 1;
        Ok(())
    }
}

fn live_worker_loop(
    rx: mpsc::Receiver<WorkerMsg>,
    tx: mpsc::Sender<WorkerEvent>,
    transcriber: Arc<dyn Transcriber>,
    cancelled: CancellationToken,
    input: &HttpInputStatus,
) {
    let mut audio: Option<HttpAudio> = None;
    for message in rx {
        if cancelled.is_cancelled() {
            break;
        }
        let WorkerMsg::Frame { chunk } = message else {
            unreachable!("live worker accepts frames");
        };
        let result = (|| -> anyhow::Result<()> {
            let (rate, pcm) = crate::audio::decode_mono(&chunk)?;
            // Live is a closed microphone-frame path, not a second offline WAV entry point.
            anyhow::ensure!(
                pcm.len() <= rate as usize / 5,
                "HTTP audio frame exceeds 200 ms"
            );
            if audio.is_none() {
                audio = Some(HttpAudio::new(rate)?);
            }
            let audio = audio.as_mut().unwrap();
            anyhow::ensure!(audio.rate == rate, "audio rate changed during recording");
            audio.push(&pcm, &tx, transcriber.as_ref(), &cancelled)
        })();
        if let Err(error) = result {
            input.fail(&format!("HTTP audio input: {error:#}"));
            break; // Never join later frames across an undecodable input hole.
        }
    }
    if !cancelled.is_cancelled()
        && let Some(audio) = &mut audio
        && audio.segmenter.finish().is_some()
        && let Err(error) = audio.submit(&tx, transcriber.as_ref(), &cancelled)
    {
        input.fail(&format!("HTTP audio tail: {error:#}"));
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn apply_worker_event(chunk_entries: &mut Vec<ChunkEntry>, event: WorkerEvent) {
    match event {
        WorkerEvent::UploadStarted { index } => {
            if let Some(entry) = chunk_entries.iter_mut().find(|entry| entry.index == index) {
                if matches!(entry.state, ChunkState::Flushed) {
                    entry.state = ChunkState::Uploading;
                } else {
                    debug!(
                        index,
                        "Ignoring upload event for chunk that is no longer flushed"
                    );
                }
            } else {
                // Live segments become known only when the worker seals and starts them.
                chunk_entries.push(ChunkEntry {
                    index,
                    state: ChunkState::Uploading,
                });
            }
        }
        WorkerEvent::Completed { index, result } => {
            if let Some(entry) = chunk_entries.iter_mut().find(|entry| entry.index == index) {
                // A timeout is terminal: late completion must not rewrite the
                // snapshot that finalization reports to the caller.
                if entry.state.is_terminal() {
                    debug!(index, "Ignoring late result for terminal chunk");
                    return;
                }
                entry.state = match result {
                    Ok(text) => {
                        info!(index, "Chunk transcribed successfully");
                        ChunkState::Transcribed(text)
                    }
                    Err(e) => {
                        error!(index, error = %e, "Chunk transcription failed");
                        ChunkState::Failed(e)
                    }
                };
            }
        }
    }
}

fn collect_transcribed_texts(chunk_entries: &[ChunkEntry]) -> Vec<String> {
    let mut ordered: Vec<&ChunkEntry> = chunk_entries.iter().collect();
    ordered.sort_by_key(|e| e.index);
    ordered
        .iter()
        .filter_map(|e| {
            if let ChunkState::Transcribed(t) = &e.state {
                Some(t.clone())
            } else {
                None
            }
        })
        .collect()
}

fn collect_results(
    chunk_entries: &[ChunkEntry],
    language: Option<String>,
) -> Result<String, SessionError> {
    let mut ordered: Vec<&ChunkEntry> = chunk_entries.iter().collect();
    ordered.sort_by_key(|e| e.index);

    let mut texts: Vec<String> = Vec::new();
    let mut errors: Vec<(usize, TranscribeError)> = Vec::new();

    for entry in ordered {
        match &entry.state {
            ChunkState::Transcribed(t) => texts.push(t.clone()),
            ChunkState::Failed(e) => errors.push((entry.index, e.clone())),
            _ => {
                // Should not happen after convergence completes.
                errors.push((
                    entry.index,
                    TranscribeError::Network("chunk did not reach terminal state".to_string()),
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(merge_texts(&texts, language))
    } else {
        Err(SessionError::PartialFailure {
            errors,
            partial_text: merge_texts(&texts, language),
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcriber::MockTranscriber;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_orchestrator_with_timeout(
        transcriber: Arc<dyn Transcriber>,
        timeout: Duration,
    ) -> SessionOrchestrator {
        SessionOrchestrator::new(
            transcriber,
            OrchestratorConfig {
                language: Some("en".to_string()),
                convergence_timeout: timeout,
            },
        )
    }

    fn default_orchestrator(transcriber: Arc<dyn Transcriber>) -> SessionOrchestrator {
        make_orchestrator_with_timeout(transcriber, Duration::from_secs(5))
    }

    #[test]
    fn production_config_uses_fixed_convergence_timeout() {
        let config = OrchestratorConfig::new(Some("zh".to_string()));

        assert_eq!(config.convergence_timeout, Duration::from_secs(30));
    }

    fn test_chunk() -> WavChunk {
        WavChunk::from_encoded_bytes(b"test wav chunk".to_vec())
    }

    struct AudioTranscriber {
        uploads: mpsc::Sender<Vec<i16>>,
        replies: Mutex<mpsc::Receiver<Result<String, TranscribeError>>>,
    }

    impl Transcriber for AudioTranscriber {
        fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError> {
            self.uploads
                .send(crate::audio::decode_mono(chunk).unwrap().1)
                .unwrap();
            self.replies
                .lock()
                .unwrap()
                .recv()
                .unwrap_or_else(|_| Ok("accepted".into()))
        }
    }

    type HttpHarness = (
        SessionOrchestrator,
        mpsc::Receiver<Vec<i16>>,
        mpsc::Sender<Result<String, TranscribeError>>,
        mpsc::Receiver<SessionId>,
    );

    fn live_http(timeout: Duration) -> HttpHarness {
        let (uploads, received) = mpsc::channel();
        let (replies, response) = mpsc::channel();
        let (notify, faults) = mpsc::channel();
        let orchestrator = SessionOrchestrator::new_live_http(
            Arc::new(AudioTranscriber {
                uploads,
                replies: Mutex::new(response),
            }),
            OrchestratorConfig {
                language: Some("en".into()),
                convergence_timeout: timeout,
            },
            move |id| {
                let _ = notify.send(id);
            },
        );
        orchestrator.start_session(SessionId(1)).unwrap();
        (orchestrator, received, replies, faults)
    }

    fn speech_then_pause() -> Vec<i16> {
        let speech = WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let mut pcm = crate::audio::decode_mono(&speech).unwrap().1;
        pcm.extend(vec![0; 32_000]);
        pcm
    }

    fn feed_live(orchestrator: &SessionOrchestrator, pcm: &[i16]) {
        for frame in pcm.chunks(3200) {
            orchestrator.on_chunk_ready(
                SessionId(1),
                crate::audio::chunk::encode_i16_wav(frame, 16_000).unwrap(),
            );
        }
    }

    #[test]
    fn silent_live_http_uses_no_service_and_one_sample_tail_is_retained() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut document = crate::core::config::ConfigDocument::default();
        document.inference.api.transcription.api_url =
            format!("http://{}/transcriptions", listener.local_addr().unwrap());
        let config =
            crate::core::recognition::RecognitionConfig::from_config(&document, false).unwrap();
        assert!(config.uses_streaming_frames());
        let orchestrator = config.build(|_| {}).unwrap();
        orchestrator.start_session(SessionId(1)).unwrap();
        feed_live(&orchestrator, &vec![0; 31 * 16_000]);
        assert_eq!(orchestrator.finish_session(SessionId(1)).unwrap(), "");
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );

        let (orchestrator, uploads, replies, _) = live_http(Duration::from_secs(5));
        feed_live(&orchestrator, &[1234]);
        drop(replies);
        assert_eq!(
            orchestrator.finish_session(SessionId(1)).unwrap(),
            "accepted"
        );
        assert_eq!(uploads.try_recv().unwrap(), [1234]);
    }

    #[test]
    fn cancel_after_stop_reaches_the_detached_http_worker() {
        let (orchestrator, uploads, replies, _) = live_http(Duration::from_millis(100));
        orchestrator.abort_session(SessionId(1)).unwrap();
        let progress = Progress::new(|| {});
        orchestrator
            .start_session_with_progress(SessionId(1), Some(progress.clone()))
            .unwrap();
        feed_live(&orchestrator, &speech_then_pause());
        uploads.recv_timeout(Duration::from_secs(2)).unwrap();
        feed_live(&orchestrator, &speech_then_pause());
        let orchestrator = Arc::new(orchestrator);
        let finishing = Arc::clone(&orchestrator);
        let finished = thread::spawn(move || finishing.finish_session(SessionId(1)));
        // A new session can start only after finalization detached the old one.
        let deadline = Instant::now() + Duration::from_secs(2);
        while orchestrator.start_session(SessionId(2)).is_err() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        progress.cancel();
        replies.send(Ok("late".into())).unwrap();
        let _ = finished.join().unwrap();
        orchestrator.abort_session(SessionId(2)).unwrap();
        drop(orchestrator);
        drop(replies);
        assert!(
            matches!(
                uploads.recv_timeout(Duration::from_secs(2)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ),
            "Esc after stop must prevent the next request without cancelling a new session"
        );
    }

    #[test]
    fn live_http_submits_before_stop_and_waits_for_unregistered_tail() {
        // At EOF the first segment may already be terminal while the tail has
        // not even acquired an index. Finalization must wait for both.
        let (orchestrator, uploads, replies, _) = live_http(Duration::from_secs(5));
        let mut pcm = speech_then_pause();
        feed_live(&orchestrator, &pcm);
        let first = uploads.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!((48_000..80_000).contains(&first.len()));
        let tail = vec![1234; 317];
        feed_live(&orchestrator, &tail);
        pcm.extend(tail);
        replies.send(Ok("repeat".into())).unwrap();
        let finished = thread::spawn(move || orchestrator.finish_session(SessionId(1)));
        let second = uploads.recv_timeout(Duration::from_secs(2)).unwrap();
        replies.send(Ok("repeat".into())).unwrap();
        assert_eq!(finished.join().unwrap().unwrap(), "repeat repeat");
        assert_eq!([first, second].concat(), pcm);
    }

    #[test]
    fn live_http_stops_new_calls_after_cancel_or_convergence_timeout() {
        for cancel in [true, false] {
            let (orchestrator, uploads, replies, _) = live_http(Duration::ZERO);
            feed_live(&orchestrator, &speech_then_pause());
            uploads.recv_timeout(Duration::from_secs(2)).unwrap();
            feed_live(&orchestrator, &speech_then_pause());
            if cancel {
                orchestrator.abort_session(SessionId(1)).unwrap();
            } else {
                assert!(matches!(
                    orchestrator.finish_session(SessionId(1)),
                    Err(SessionError::ConvergenceTimeout {
                        pending_count: 1..,
                        ..
                    })
                ));
            }
            drop(orchestrator);
            replies.send(Ok("late".into())).unwrap();
            assert!(
                matches!(
                    uploads.recv_timeout(Duration::from_secs(2)),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ),
                "queued audio must not start another request"
            );
        }
    }

    #[test]
    fn live_http_full_queue_keeps_only_the_contiguous_prefix() {
        let (orchestrator, uploads, replies, faults) = live_http(Duration::from_secs(5));
        let mut accepted = speech_then_pause();
        feed_live(&orchestrator, &accepted);
        let first = uploads.recv_timeout(Duration::from_secs(2)).unwrap();
        for marker in 1..=301 {
            let frame = vec![marker; 3200];
            feed_live(&orchestrator, &frame);
            if orchestrator.recording_error(SessionId(1)).is_some() {
                break;
            }
            accepted.extend(frame);
        }
        assert_eq!(
            faults.recv_timeout(Duration::from_secs(1)).unwrap(),
            SessionId(1)
        );
        feed_live(&orchestrator, &[999; 3200]);
        drop(replies); // Release the blocked request and allow the accepted prefix to drain.
        assert!(matches!(
            orchestrator.finish_session(SessionId(1)),
            Err(SessionError::InputFailure { .. })
        ));
        let mut actual = first;
        for segment in uploads.try_iter() {
            assert!(segment.len() <= 480_000);
            actual.extend(segment);
        }
        assert_eq!(actual, accepted, "reject every frame after the first hole");
    }

    #[test]
    fn live_http_decode_fault_wakes_without_more_audio_and_preserves_prefix() {
        let (orchestrator, uploads, replies, faults) = live_http(Duration::from_secs(5));
        feed_live(&orchestrator, &[77; 3200]);
        orchestrator.on_chunk_ready(SessionId(1), test_chunk());
        feed_live(&orchestrator, &[99; 3200]);
        assert_eq!(
            faults.recv_timeout(Duration::from_secs(2)).unwrap(),
            SessionId(1)
        );
        assert_eq!(
            uploads.recv_timeout(Duration::from_secs(2)).unwrap(),
            vec![77; 3200]
        );
        replies.send(Ok("prefix".into())).unwrap();
        assert!(matches!(orchestrator.finish_session(SessionId(1)),
            Err(SessionError::InputFailure { partial_text, .. }) if partial_text == "prefix"));
        assert!(uploads.try_recv().is_err());
    }

    #[test]
    fn live_http_segment_failure_does_not_block_later_segments() {
        let (orchestrator, uploads, replies, _) = live_http(Duration::from_secs(5));
        let mut pcm = speech_then_pause();
        pcm.extend(speech_then_pause());
        pcm.extend([777]);
        feed_live(&orchestrator, &pcm);
        let finished = thread::spawn(move || orchestrator.finish_session(SessionId(1)));
        for response in [
            Err(TranscribeError::Timeout),
            Ok("middle".into()),
            Ok("tail".into()),
        ] {
            uploads.recv_timeout(Duration::from_secs(2)).unwrap();
            replies.send(response).unwrap();
        }
        assert!(
            matches!(finished.join().unwrap(), Err(SessionError::PartialFailure { errors, partial_text })
            if errors.len() == 1 && errors[0].0 == 0 && partial_text == "middle tail")
        );
    }

    // ── Mock transcribers ────────────────────────────────────────────────────

    /// Always returns a fixed text; never touches the file system.
    struct FixedTranscriber(String);

    impl Transcriber for FixedTranscriber {
        fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
            Ok(self.0.clone())
        }
    }

    /// Returns pre-configured results in call order.
    struct ScriptedTranscriber {
        results: Vec<Result<String, TranscribeError>>,
        call_count: AtomicUsize,
        call_tx: mpsc::Sender<usize>,
    }

    impl ScriptedTranscriber {
        fn new(results: Vec<Result<String, TranscribeError>>) -> (Self, mpsc::Receiver<usize>) {
            let (call_tx, call_rx) = mpsc::channel();
            (
                Self {
                    results,
                    call_count: AtomicUsize::new(0),
                    call_tx,
                },
                call_rx,
            )
        }
    }

    impl Transcriber for ScriptedTranscriber {
        fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
            let i = self.call_count.fetch_add(1, Ordering::SeqCst);
            let _ = self.call_tx.send(i);
            match self.results.get(i) {
                Some(Ok(s)) => Ok(s.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Ok("extra".to_string()),
            }
        }
    }

    struct GateTranscriber {
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Transcriber for GateTranscriber {
        fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
            let (lock, condvar) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = condvar.wait(released).unwrap();
            }
            Ok("released".to_string())
        }
    }

    /// Always panics — used to test worker-panic handling.
    struct PanicTranscriber;

    impl Transcriber for PanicTranscriber {
        fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
            panic!("intentional transcriber panic for testing");
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn worker_reports_result_without_shared_chunks() {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::channel();
        let cancelled = CancellationToken::new();
        let worker = thread::spawn({
            let cancelled = cancelled.clone();
            move || {
                worker_loop(
                    chunk_rx,
                    result_tx,
                    Arc::new(FixedTranscriber("event result".to_string())),
                    cancelled,
                );
            }
        });

        chunk_tx
            .send(WorkerMsg::Chunk {
                index: 7,
                chunk: test_chunk(),
            })
            .unwrap();
        drop(chunk_tx);

        assert!(matches!(
            result_rx.recv().unwrap(),
            WorkerEvent::UploadStarted { index: 7 }
        ));
        assert!(matches!(
            result_rx.recv().unwrap(),
            WorkerEvent::Completed {
                index: 7,
                result: Ok(ref text),
            } if text == "event result"
        ));
        worker.join().unwrap();
    }

    #[test]
    fn session_applies_worker_state_transitions() {
        let mut chunk_entries = vec![ChunkEntry {
            index: 3,
            state: ChunkState::Flushed,
        }];

        apply_worker_event(&mut chunk_entries, WorkerEvent::UploadStarted { index: 3 });
        assert!(matches!(chunk_entries[0].state, ChunkState::Uploading));

        apply_worker_event(
            &mut chunk_entries,
            WorkerEvent::Completed {
                index: 3,
                result: Ok("done".to_string()),
            },
        );
        assert!(matches!(
            chunk_entries[0].state,
            ChunkState::Transcribed(ref text) if text == "done"
        ));
    }

    #[test]
    fn multi_chunk_results_remain_index_ordered() {
        let mut chunk_entries = vec![
            ChunkEntry {
                index: 0,
                state: ChunkState::Flushed,
            },
            ChunkEntry {
                index: 1,
                state: ChunkState::Flushed,
            },
        ];

        apply_worker_event(
            &mut chunk_entries,
            WorkerEvent::Completed {
                index: 1,
                result: Ok("second".to_string()),
            },
        );
        apply_worker_event(
            &mut chunk_entries,
            WorkerEvent::Completed {
                index: 0,
                result: Ok("first".to_string()),
            },
        );

        assert_eq!(
            collect_results(&chunk_entries, Some("en".into())).unwrap(),
            "first second"
        );
    }

    #[test]
    fn disconnected_result_channel_still_drains_queued_chunks() {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(2);
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            worker_loop(
                chunk_rx,
                result_tx,
                Arc::new(FixedTranscriber("unused".to_string())),
                CancellationToken::new(),
            );
        });
        drop(result_rx);

        for index in 0..2 {
            chunk_tx
                .send(WorkerMsg::Chunk {
                    index,
                    chunk: test_chunk(),
                })
                .unwrap();
        }
        drop(chunk_tx);
        worker.join().unwrap();
    }

    #[test]
    fn cancelled_worker_drains_queued_chunks_after_result_disconnect() {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(2);
        for index in 0..2 {
            chunk_tx
                .send(WorkerMsg::Chunk {
                    index,
                    chunk: test_chunk(),
                })
                .unwrap();
        }
        drop(chunk_tx);

        let (result_tx, result_rx) = mpsc::channel();
        drop(result_rx);
        let worker = thread::spawn(move || {
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            worker_loop(
                chunk_rx,
                result_tx,
                Arc::new(FixedTranscriber("unused".to_string())),
                cancelled,
            );
        });
        worker.join().unwrap();
    }

    #[test]
    fn test_single_chunk_success() {
        let t = Arc::new(FixedTranscriber("hello world".to_string()));
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk());
        let result = orch.finish_session(SessionId(1));

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_no_chunks_returns_error() {
        let t = Arc::new(MockTranscriber);
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        let result = orch.finish_session(SessionId(1));

        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn test_stop_session_without_start_returns_no_chunks() {
        let t = Arc::new(MockTranscriber);
        let orch = default_orchestrator(t);

        // stop_session called without a prior start_session.
        let result = orch.finish_session(SessionId(1));
        assert!(matches!(result, Err(SessionError::Routing(_))));
    }

    #[test]
    fn test_chunk_without_active_session_is_rejected() {
        let orch = default_orchestrator(Arc::new(MockTranscriber));

        orch.on_chunk_ready(SessionId(1), test_chunk());
        orch.start_session(SessionId(2)).unwrap();
        assert!(matches!(
            orch.finish_session(SessionId(2)),
            Err(SessionError::NoChunks)
        ));
    }

    #[test]
    fn test_partial_failure_returns_error_with_partial_text() {
        let (t, calls) = ScriptedTranscriber::new(vec![
            Ok("good chunk".to_string()),
            Err(TranscribeError::Api {
                status: 500,
                body: "server error".to_string(),
            }),
            Ok("another good".to_string()),
        ]);
        let orch = default_orchestrator(Arc::new(t));

        orch.start_session(SessionId(1)).unwrap();
        for expected_call in 0..3 {
            orch.on_chunk_ready(SessionId(1), test_chunk());
            assert_eq!(
                calls.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected_call
            );
        }
        let result = orch.finish_session(SessionId(1));

        match result {
            Err(SessionError::PartialFailure {
                errors,
                partial_text,
            }) => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].0, 1); // chunk index 1 failed
                // The structured error variant survives end-to-end (no string parsing).
                assert!(matches!(
                    errors[0].1,
                    TranscribeError::Api { status: 500, .. }
                ));
                // partial_text contains only the successful chunks in order.
                assert_eq!(partial_text, "good chunk another good");
            }
            other => panic!("Expected PartialFailure, got {:?}", other),
        }
    }

    #[test]
    fn test_convergence_timeout() {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let t = Arc::new(GateTranscriber {
            release: Arc::clone(&release),
        });
        let orch = make_orchestrator_with_timeout(t, Duration::from_millis(100));

        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk());
        let result = orch.finish_session(SessionId(1));

        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();

        match result {
            Err(SessionError::ConvergenceTimeout { pending_count, .. }) => {
                assert_eq!(pending_count, 1);
            }
            other => panic!("Expected ConvergenceTimeout, got {:?}", other),
        }
    }

    #[test]
    fn test_timed_out_chunk_cannot_be_overwritten_by_late_worker_result() {
        let mut chunk_entries = vec![ChunkEntry {
            index: 0,
            state: ChunkState::Failed(TranscribeError::Timeout),
        }];

        apply_worker_event(&mut chunk_entries, WorkerEvent::UploadStarted { index: 0 });
        apply_worker_event(
            &mut chunk_entries,
            WorkerEvent::Completed {
                index: 0,
                result: Ok("late result".to_string()),
            },
        );

        assert!(matches!(
            chunk_entries[0].state,
            ChunkState::Failed(TranscribeError::Timeout)
        ));
    }

    #[test]
    fn test_full_worker_queue_marks_chunk_failed_without_blocking() {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let t = Arc::new(GateTranscriber {
            release: Arc::clone(&release),
        });
        let orch = default_orchestrator(t);
        orch.start_session(SessionId(1)).unwrap();

        let started = Instant::now();
        for _ in 0..100 {
            orch.on_chunk_ready(SessionId(1), test_chunk());
        }

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "enqueueing blocked on a full worker queue"
        );
        let inner = orch.inner.lock().unwrap();
        let SessionWorker::Http(session) = &inner.as_ref().unwrap().worker else {
            panic!("expected HTTP worker");
        };
        let chunk_entries = &session.chunk_entries;
        // When STT falls behind recording, rejected submissions must remain in
        // the session so finalization cannot silently report complete success.
        assert_eq!(chunk_entries.len(), 100);
        let failed_indices: Vec<_> = chunk_entries
            .iter()
            .filter_map(|entry| match &entry.state {
                ChunkState::Failed(TranscribeError::Network(message))
                    if message == "worker queue full" =>
                {
                    Some(entry.index)
                }
                _ => None,
            })
            .collect();
        assert!(!failed_indices.is_empty());
        drop(inner);

        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
        match orch.finish_session(SessionId(1)) {
            Err(SessionError::PartialFailure {
                errors,
                partial_text,
            }) => {
                assert_eq!(
                    errors
                        .into_iter()
                        .map(|(index, _)| index)
                        .collect::<Vec<_>>(),
                    failed_indices
                );
                assert!(!partial_text.is_empty());
            }
            other => panic!("Expected queue failures with partial text, got {other:?}"),
        }
    }

    #[test]
    fn test_session_reentry_is_rejected_without_replacing_active_session() {
        let t = Arc::new(FixedTranscriber("new session".to_string()));
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk());

        let error = orch.start_session(SessionId(2)).unwrap_err();
        assert_eq!(
            error,
            SessionStartError {
                requested: SessionId(2),
                active: SessionId(1),
            }
        );
        let result = orch.finish_session(SessionId(1));

        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(result.unwrap(), "new session");
    }

    #[test]
    fn mismatched_chunk_and_finish_do_not_mutate_active_session() {
        let orch = default_orchestrator(Arc::new(FixedTranscriber("active".into())));
        orch.start_session(SessionId(1)).unwrap();

        orch.on_chunk_ready(SessionId(2), test_chunk());
        assert!(matches!(
            orch.finish_session(SessionId(2)),
            Err(SessionError::Routing(
                SessionRoutingError::SessionMismatch { .. }
            ))
        ));
        assert!(matches!(
            orch.finish_session(SessionId(1)),
            Err(SessionError::NoChunks)
        ));
    }

    #[test]
    fn abort_rejects_wrong_id_and_preserves_active_session() {
        let orch = default_orchestrator(Arc::new(MockTranscriber));
        orch.start_session(SessionId(1)).unwrap();

        assert!(matches!(
            orch.abort_session(SessionId(2)),
            Err(SessionRoutingError::SessionMismatch { .. })
        ));
        assert!(orch.abort_session(SessionId(1)).is_ok());
    }

    #[test]
    fn test_worker_panic_reports_failure() {
        let t = Arc::new(PanicTranscriber);
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk());
        let result = orch.finish_session(SessionId(1));

        assert!(matches!(result, Err(SessionError::PartialFailure { .. })));
    }
}
