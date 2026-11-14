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
use crate::audio::WavChunk;
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
    /// Recorder input failed; completed text carries an incomplete-recording warning.
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

struct WorkerMsg {
    index: usize,
    chunk: WavChunk,
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
            } => SessionWorker::Http(HttpSession::new(
                Arc::clone(transcriber),
                config.clone(),
                progress.map_or_else(CancellationToken::new, |progress| {
                    progress.cancelled.clone()
                }),
            )),
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
            SessionWorker::Http(session) => session.input_failure.clone(),
        }
    }

    pub(crate) fn fail_session(&self, session_id: SessionId, error: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = inner.as_mut().filter(|s| s.session_id == session_id) {
            match &mut active.worker {
                SessionWorker::Realtime(session) => session.fail(error),
                SessionWorker::Http(session) => {
                    session
                        .input_failure
                        .get_or_insert_with(|| error.to_owned());
                }
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

struct HttpSession {
    chunk_entries: Vec<ChunkEntry>,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    result_rx: mpsc::Receiver<WorkerEvent>,
    worker: thread::JoinHandle<()>,
    next_index: usize,
    cancelled: CancellationToken,
    config: OrchestratorConfig,
    input_failure: Option<String>,
}

impl HttpSession {
    fn new(
        transcriber: Arc<dyn Transcriber>,
        config: OrchestratorConfig,
        cancelled: CancellationToken,
    ) -> Self {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<WorkerMsg>(2);
        let (result_tx, result_rx) = mpsc::channel::<WorkerEvent>();
        let worker_cancelled = cancelled.clone();
        let worker = thread::spawn(move || {
            worker_loop(chunk_rx, result_tx, transcriber, worker_cancelled);
        });
        Self {
            chunk_entries: Vec::new(),
            chunk_tx,
            result_rx,
            worker,
            next_index: 0,
            cancelled,
            config,
            input_failure: None,
        }
    }

    fn on_chunk_ready(&mut self, session_id: SessionId, chunk: WavChunk) {
        let session = self;
        let index = session.next_index;
        session.next_index += 1;

        let state = if let Err(e) = session.chunk_tx.try_send(WorkerMsg { index, chunk }) {
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
            // Every complete chunk, including the recorder tail, is registered at intake.
            if chunk_entries.iter().all(|entry| entry.state.is_terminal()) {
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
                        .count();
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

        if let Some(error) = session.input_failure {
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
    for WorkerMsg { index, chunk } in rx {
        if cancelled.is_cancelled() {
            continue;
        }
        transcribe_chunk(index, &chunk, &result_tx, transcriber.as_ref(), &cancelled);
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

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn apply_worker_event(chunk_entries: &mut [ChunkEntry], event: WorkerEvent) {
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

    fn test_chunk() -> WavChunk {
        WavChunk::from_encoded_bytes(b"test wav chunk".to_vec())
    }

    #[test]
    fn silent_chunks_use_no_service() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut document = crate::core::config::ConfigDocument::default();
        document.inference.api.transcription.api_url =
            format!("http://{}/transcriptions", listener.local_addr().unwrap());
        let config =
            crate::core::recognition::RecognitionConfig::from_config(&document, false).unwrap();
        let orchestrator = config.build().unwrap();
        orchestrator.start_session(SessionId(1)).unwrap();
        for samples in [30 * 16_000, 16_000] {
            orchestrator.on_chunk_ready(
                SessionId(1),
                crate::audio::chunk::encode_i16_wav(&vec![0; samples], 16_000).unwrap(),
            );
        }
        assert_eq!(orchestrator.finish_session(SessionId(1)).unwrap(), "");
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn cancel_after_stop_reaches_the_detached_http_worker() {
        let (transcriber, calls, release) = GateTranscriber::new();
        let orchestrator =
            make_orchestrator_with_timeout(Arc::new(transcriber), Duration::from_millis(100));
        let progress = Progress::new(|| {});
        orchestrator
            .start_session_with_progress(SessionId(1), Some(progress.clone()))
            .unwrap();
        orchestrator.on_chunk_ready(SessionId(1), test_chunk());
        calls.recv_timeout(Duration::from_secs(2)).unwrap();
        orchestrator.on_chunk_ready(SessionId(1), test_chunk());
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
        drop(release);
        let _ = finished.join().unwrap();
        orchestrator.abort_session(SessionId(2)).unwrap();
        drop(orchestrator);
        assert!(
            matches!(
                calls.recv_timeout(Duration::from_secs(2)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ),
            "Esc after stop must prevent the next request without cancelling a new session"
        );
    }

    #[test]
    fn complete_chunks_submit_before_stop_and_preserve_repeated_text() {
        let (transcriber, calls) =
            ScriptedTranscriber::new(vec![Ok("repeat".into()), Ok("repeat".into())]);
        let orchestrator = default_orchestrator(Arc::new(transcriber));
        orchestrator.start_session(SessionId(1)).unwrap();
        orchestrator.on_chunk_ready(SessionId(1), test_chunk());
        assert_eq!(calls.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        orchestrator.on_chunk_ready(SessionId(1), test_chunk());
        assert_eq!(
            orchestrator.finish_session(SessionId(1)).unwrap(),
            "repeat repeat"
        );
    }

    #[test]
    fn http_stops_new_calls_after_cancel_or_convergence_timeout() {
        for cancel in [true, false] {
            let (transcriber, calls, release) = GateTranscriber::new();
            let orchestrator =
                make_orchestrator_with_timeout(Arc::new(transcriber), Duration::ZERO);
            orchestrator.start_session(SessionId(1)).unwrap();
            orchestrator.on_chunk_ready(SessionId(1), test_chunk());
            calls.recv_timeout(Duration::from_secs(2)).unwrap();
            orchestrator.on_chunk_ready(SessionId(1), test_chunk());
            if cancel {
                orchestrator.abort_session(SessionId(1)).unwrap();
            } else {
                assert!(matches!(
                    orchestrator.finish_session(SessionId(1)),
                    Err(SessionError::ConvergenceTimeout {
                        pending_count: 2,
                        ..
                    })
                ));
            }
            drop(orchestrator);
            drop(release);
            assert!(
                matches!(
                    calls.recv_timeout(Duration::from_secs(2)),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ),
                "queued audio must not start another request"
            );
        }
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

    /// Reports each call and blocks until the release sender is dropped.
    struct GateTranscriber {
        calls: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl GateTranscriber {
        fn new() -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (calls, started) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            (
                Self {
                    calls,
                    release: Mutex::new(wait),
                },
                started,
                release,
            )
        }
    }

    impl Transcriber for GateTranscriber {
        fn transcribe(&self, _chunk: &WavChunk) -> Result<String, TranscribeError> {
            let _ = self.calls.send(());
            let _ = self.release.lock().unwrap().recv();
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
                .send(WorkerMsg {
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
                .send(WorkerMsg {
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
    fn test_no_chunks_returns_error() {
        let t = Arc::new(MockTranscriber);
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        let result = orch.finish_session(SessionId(1));

        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn recorder_failure_keeps_the_first_error_and_completed_text() {
        // Recorder faults must survive complete-chunk intake, including a fault
        // before the first segment; segmentation no longer belongs to HTTP.
        for has_chunk in [false, true] {
            let orch = default_orchestrator(Arc::new(FixedTranscriber("prefix".into())));
            orch.start_session(SessionId(1)).unwrap();
            if has_chunk {
                orch.on_chunk_ready(SessionId(1), test_chunk());
            }
            orch.fail_session(SessionId(1), "recorder overflow");
            orch.fail_session(SessionId(1), "later fault");
            assert_eq!(
                orch.recording_error(SessionId(1)).as_deref(),
                Some("recorder overflow")
            );
            assert!(matches!(orch.finish_session(SessionId(1)),
                Err(SessionError::InputFailure { error, partial_text })
                if error == "recorder overflow"
                    && partial_text == if has_chunk { "prefix" } else { "" }));
        }
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
        let (transcriber, calls, release) = GateTranscriber::new();
        let orch = Arc::new(default_orchestrator(Arc::new(transcriber)));
        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk());
        calls.recv_timeout(Duration::from_secs(5)).unwrap();

        // A slow upload leaves room for two queued chunks. The fourth chunk must
        // be rejected without blocking recording or hiding incomplete delivery.
        let (submitted, done) = mpsc::channel();
        let enqueue = Arc::clone(&orch);
        let submitter = thread::spawn(move || {
            for _ in 0..3 {
                enqueue.on_chunk_ready(SessionId(1), test_chunk());
            }
            submitted.send(()).unwrap();
        });
        let submission = done.recv_timeout(Duration::from_secs(5));

        drop(release);
        submitter.join().unwrap();
        submission.expect("enqueueing blocked on a full worker queue");
        match orch.finish_session(SessionId(1)) {
            Err(SessionError::PartialFailure {
                errors,
                partial_text,
            }) => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(&errors[0], (3, TranscribeError::Network(message))
                    if message == "worker queue full"));
                assert_eq!(partial_text, "released released released");
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
