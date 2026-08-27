//! Session orchestrator for end-to-end stream recognition.
//!
//! `SessionOrchestrator` owns recording-session chunk tracking, background transcription,
//! convergence wait, and result merging.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::audio::WavChunk;
use crate::session::SessionId;
use crate::text::merge_texts;
use crate::transcriber::Transcriber;
// Re-exported so callers can keep using `core::orchestrator::TranscribeError`.
pub use crate::transcriber::TranscribeError;

// The STT retry window is kept below this deadline so a tail chunk can normally converge on stop.
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(30);

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStartError {
    ActiveSession {
        requested: SessionId,
        active: SessionId,
    },
}

impl fmt::Display for SessionStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActiveSession { requested, active } => write!(
                f,
                "cannot start session {} while session {} is active",
                requested.0, active.0
            ),
        }
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
                "Convergence timeout: {} chunk(s) still pending; partial text: {:?}",
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

struct ActiveSessionInner {
    session_id: SessionId,
    chunks: Vec<ChunkEntry>,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    result_rx: mpsc::Receiver<WorkerEvent>,
    worker: thread::JoinHandle<()>,
    next_index: usize,
    cancelled: Arc<AtomicBool>,
}

// ─── SessionOrchestrator ──────────────────────────────────────────────────────

/// Coordinates recording session lifecycle, background transcription, convergence
/// wait, and result merging for both Hold and Toggle modes.
pub struct SessionOrchestrator {
    transcriber: Arc<dyn Transcriber>,
    language: Option<String>,
    convergence_timeout: Duration,
    inner: Mutex<Option<ActiveSessionInner>>,
}

impl SessionOrchestrator {
    /// Create an orchestrator.
    ///
    /// - `transcriber`: injected for testability (use `MockTranscriber` in tests).
    /// - `config`: already validated at the application boundary.
    pub fn new(transcriber: Arc<dyn Transcriber>, config: OrchestratorConfig) -> Self {
        info!(
            language = config.language.as_deref().unwrap_or("auto"),
            convergence_timeout_secs = config.convergence_timeout.as_secs(),
            "Session orchestrator configured"
        );
        Self {
            transcriber,
            language: config.language,
            convergence_timeout: config.convergence_timeout,
            inner: Mutex::new(None),
        }
    }

    /// Start a new recording session.
    ///
    /// Returns an error without replacing an existing active session.
    pub fn start_session(&self, session_id: SessionId) -> Result<(), SessionStartError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = inner.as_ref() {
            return Err(SessionStartError::ActiveSession {
                requested: session_id,
                active: active.session_id,
            });
        }

        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<WorkerMsg>(2);
        let (result_tx, result_rx) = mpsc::channel::<WorkerEvent>();
        let cancelled = Arc::new(AtomicBool::new(false));

        let transcriber = Arc::clone(&self.transcriber);
        let worker_cancelled = Arc::clone(&cancelled);

        let worker = thread::spawn(move || {
            worker_loop(chunk_rx, result_tx, transcriber, worker_cancelled);
        });

        *inner = Some(ActiveSessionInner {
            session_id,
            chunks: Vec::new(),
            chunk_tx,
            result_rx,
            worker,
            next_index: 0,
            cancelled,
        });

        info!(session_id = session_id.0, "Session started");
        Ok(())
    }

    /// Submit one in-memory WAV chunk for background transcription.
    pub fn on_chunk_ready(
        &self,
        session_id: SessionId,
        chunk: WavChunk,
    ) -> Result<usize, SessionRoutingError> {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.as_mut() else {
            warn!("Chunk arrived without an active session");
            return Err(SessionRoutingError::NoActiveSession {
                requested: session_id,
            });
        };
        if session.session_id != session_id {
            let active = session.session_id;
            warn!(
                requested = session_id.0,
                active = active.0,
                "Rejecting stale chunk"
            );
            return Err(SessionRoutingError::SessionMismatch {
                requested: session_id,
                active,
            });
        }

        let index = session.next_index;
        session.next_index += 1;

        session.chunks.push(ChunkEntry {
            index,
            state: ChunkState::Flushed,
        });

        if let Err(e) = session.chunk_tx.try_send(WorkerMsg::Chunk { index, chunk }) {
            let message = match e {
                mpsc::TrySendError::Full(_) => "worker queue full",
                mpsc::TrySendError::Disconnected(_) => "worker channel closed",
            };
            error!(
                error = message,
                "Failed to enqueue chunk; marking as failed"
            );
            if let Some(entry) = session.chunks.iter_mut().find(|e| e.index == index) {
                entry.state = ChunkState::Failed(TranscribeError::Network(message.to_string()));
            }
        } else {
            info!(index = index, "Chunk enqueued for background transcription");
        }

        drain_worker_events(session);
        Ok(index)
    }

    /// Stop the current session and block until all chunks reach a terminal state
    /// (or `convergence_timeout` elapses).
    ///
    /// Returns:
    /// - `Ok(text)` — all chunks succeeded; `text` is the language-aware merge.
    /// - `Err(SessionError::NoChunks)` — recording produced no chunks.
    /// - `Err(SessionError::PartialFailure { … })` — some chunks failed; partial text included.
    /// - `Err(SessionError::ConvergenceTimeout { … })` — timeout hit; partial text included.
    pub fn finish_session(&self, session_id: SessionId) -> Result<String, SessionError> {
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let Some(active) = inner.take() else {
                return Err(SessionError::Routing(
                    SessionRoutingError::NoActiveSession {
                        requested: session_id,
                    },
                ));
            };
            if active.session_id != session_id {
                let active_session_id = active.session_id;
                *inner = Some(active);
                return Err(SessionError::Routing(
                    SessionRoutingError::SessionMismatch {
                        requested: session_id,
                        active: active_session_id,
                    },
                ));
            }
            active
        };

        if session.next_index == 0 {
            // Closing the channel lets the idle worker exit immediately.
            drop(session.chunk_tx);
            let _ = session.worker.join();
            return Err(SessionError::NoChunks);
        }

        // Closing the sender is non-blocking. The receiver still drains every
        // queued chunk before its iterator ends, so the convergence deadline
        // covers the entire shutdown rather than starting after a blocking send.
        drop(session.chunk_tx);

        let mut chunks = session.chunks;
        let result_rx = session.result_rx;
        let worker = session.worker;
        let deadline = Instant::now() + self.convergence_timeout;
        let mut timed_out = false;

        loop {
            if chunks.iter().all(|entry| entry.state.is_terminal()) {
                break;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            match result_rx.recv_timeout(remaining) {
                Ok(event) => apply_worker_event(&mut chunks, event),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    let pending_count = chunks
                        .iter()
                        .filter(|entry| !entry.state.is_terminal())
                        .count();
                    warn!(
                        pending_count = pending_count,
                        "Convergence timeout; marking pending chunks as Failed(Timeout)"
                    );
                    for entry in &mut chunks {
                        if !entry.state.is_terminal() {
                            entry.state = ChunkState::Failed(TranscribeError::Timeout);
                        }
                    }
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let error =
                        TranscribeError::Network("worker result channel closed".to_string());
                    for entry in &mut chunks {
                        if !entry.state.is_terminal() {
                            entry.state = ChunkState::Failed(error.clone());
                        }
                    }
                    break;
                }
            }
        }

        if !timed_out {
            // Every result was received or the result channel disconnected.
            let _ = worker.join();
        } else {
            // Drop the handle without joining — the worker may still be mid-request.
            // It only owns the result sender, so it cannot retain or mutate `chunks`.
            drop(worker);
        }

        if timed_out {
            let texts = collect_transcribed_texts(&chunks);
            let pending_count = chunks
                .iter()
                .filter(|entry| matches!(entry.state, ChunkState::Failed(TranscribeError::Timeout)))
                .count();
            return Err(SessionError::ConvergenceTimeout {
                pending_count,
                partial_text: merge_texts(&texts, self.language.as_deref()),
            });
        }

        collect_results(&chunks, self.language.as_deref())
    }

    pub fn abort_session(&self, session_id: SessionId) -> Result<(), SessionRoutingError> {
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let Some(active) = inner.take() else {
                return Err(SessionRoutingError::NoActiveSession {
                    requested: session_id,
                });
            };
            if active.session_id != session_id {
                let active_session_id = active.session_id;
                *inner = Some(active);
                return Err(SessionRoutingError::SessionMismatch {
                    requested: session_id,
                    active: active_session_id,
                });
            }
            active
        };

        session.cancelled.store(true, Ordering::Release);
        drop(session);
        info!(session_id = session_id.0, "Session aborted");
        Ok(())
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

fn worker_loop(
    rx: mpsc::Receiver<WorkerMsg>,
    result_tx: mpsc::Sender<WorkerEvent>,
    transcriber: Arc<dyn Transcriber>,
    cancelled: Arc<AtomicBool>,
) {
    for msg in rx {
        match msg {
            WorkerMsg::Chunk { index, chunk } => {
                if cancelled.load(Ordering::Acquire) {
                    continue;
                }

                let _ = result_tx.send(WorkerEvent::UploadStarted { index });

                debug!(
                    index = index,
                    bytes = chunk.len(),
                    "Worker transcribing chunk"
                );
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    transcriber.transcribe(&chunk)
                }))
                .unwrap_or_else(|_| {
                    Err(TranscribeError::Network("transcriber panicked".to_string()))
                });

                let _ = result_tx.send(WorkerEvent::Completed { index, result });
            }
        }
    }
    debug!("Worker channel closed, exiting");
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn drain_worker_events(session: &mut ActiveSessionInner) {
    while let Ok(event) = session.result_rx.try_recv() {
        apply_worker_event(&mut session.chunks, event);
    }
}

fn apply_worker_event(chunks: &mut [ChunkEntry], event: WorkerEvent) {
    match event {
        WorkerEvent::UploadStarted { index } => {
            if let Some(entry) = chunks.iter_mut().find(|entry| entry.index == index) {
                begin_upload(entry);
            }
        }
        WorkerEvent::Completed { index, result } => {
            if let Some(entry) = chunks.iter_mut().find(|entry| entry.index == index) {
                record_worker_result(entry, result);
            }
        }
    }
}

fn record_worker_result(entry: &mut ChunkEntry, result: Result<String, TranscribeError>) {
    // A convergence timeout is terminal from the caller's perspective. A late
    // worker event must not rewrite the snapshot used for the timeout result.
    if entry.state.is_terminal() {
        debug!(
            index = entry.index,
            "Ignoring late result for terminal chunk"
        );
        return;
    }

    entry.state = match result {
        Ok(text) => {
            info!(index = entry.index, "Chunk transcribed successfully");
            ChunkState::Transcribed(text)
        }
        Err(e) => {
            error!(index = entry.index, error = %e, "Chunk transcription failed");
            ChunkState::Failed(e)
        }
    };
}

fn begin_upload(entry: &mut ChunkEntry) -> bool {
    if !matches!(entry.state, ChunkState::Flushed) {
        debug!(
            index = entry.index,
            "Ignoring upload event for chunk that is no longer flushed"
        );
        return false;
    }
    entry.state = ChunkState::Uploading;
    true
}

fn collect_transcribed_texts(chunks: &[ChunkEntry]) -> Vec<String> {
    let mut ordered: Vec<&ChunkEntry> = chunks.iter().collect();
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

fn collect_results(chunks: &[ChunkEntry], language: Option<&str>) -> Result<String, SessionError> {
    let mut ordered: Vec<&ChunkEntry> = chunks.iter().collect();
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
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker = thread::spawn({
            let cancelled = Arc::clone(&cancelled);
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
        let mut chunks = vec![ChunkEntry {
            index: 3,
            state: ChunkState::Flushed,
        }];

        apply_worker_event(&mut chunks, WorkerEvent::UploadStarted { index: 3 });
        assert!(matches!(chunks[0].state, ChunkState::Uploading));

        apply_worker_event(
            &mut chunks,
            WorkerEvent::Completed {
                index: 3,
                result: Ok("done".to_string()),
            },
        );
        assert!(matches!(
            chunks[0].state,
            ChunkState::Transcribed(ref text) if text == "done"
        ));
    }

    #[test]
    fn multi_chunk_results_remain_index_ordered() {
        let mut chunks = vec![
            ChunkEntry {
                index: 0,
                state: ChunkState::Flushed,
            },
            ChunkEntry {
                index: 1,
                state: ChunkState::Flushed,
            },
            ChunkEntry {
                index: 2,
                state: ChunkState::Flushed,
            },
        ];

        apply_worker_event(
            &mut chunks,
            WorkerEvent::Completed {
                index: 2,
                result: Ok("second".to_string()),
            },
        );
        apply_worker_event(
            &mut chunks,
            WorkerEvent::Completed {
                index: 1,
                result: Ok(String::new()),
            },
        );
        apply_worker_event(
            &mut chunks,
            WorkerEvent::Completed {
                index: 0,
                result: Ok("first".to_string()),
            },
        );

        assert_eq!(
            collect_results(&chunks, Some("en")).unwrap(),
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
                Arc::new(AtomicBool::new(false)),
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
            worker_loop(
                chunk_rx,
                result_tx,
                Arc::new(FixedTranscriber("unused".to_string())),
                Arc::new(AtomicBool::new(true)),
            );
        });
        worker.join().unwrap();
    }

    #[test]
    fn test_single_chunk_success() {
        let t = Arc::new(FixedTranscriber("hello world".to_string()));
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        let _ = orch.on_chunk_ready(SessionId(1), test_chunk());
        let result = orch.finish_session(SessionId(1));

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn empty_chunk_results_finish_as_an_empty_success() {
        let orch = default_orchestrator(Arc::new(FixedTranscriber(String::new())));

        orch.start_session(SessionId(1)).unwrap();
        orch.on_chunk_ready(SessionId(1), test_chunk()).unwrap();

        assert_eq!(orch.finish_session(SessionId(1)).unwrap(), "");
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

        let result = orch.on_chunk_ready(SessionId(1), test_chunk());

        assert!(matches!(
            result,
            Err(SessionRoutingError::NoActiveSession { .. })
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
            assert_eq!(
                orch.on_chunk_ready(SessionId(1), test_chunk()).unwrap(),
                expected_call
            );
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
        let _ = orch.on_chunk_ready(SessionId(1), test_chunk());
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
        let mut chunks = vec![ChunkEntry {
            index: 0,
            state: ChunkState::Failed(TranscribeError::Timeout),
        }];

        apply_worker_event(&mut chunks, WorkerEvent::UploadStarted { index: 0 });
        apply_worker_event(
            &mut chunks,
            WorkerEvent::Completed {
                index: 0,
                result: Ok("late result".to_string()),
            },
        );

        assert!(matches!(
            chunks[0].state,
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
            let _ = orch.on_chunk_ready(SessionId(1), test_chunk());
        }

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "enqueueing blocked on a full worker queue"
        );
        let inner = orch.inner.lock().unwrap();
        let chunks = &inner.as_ref().unwrap().chunks;
        assert!(chunks.iter().any(|entry| matches!(
            entry.state,
            ChunkState::Failed(TranscribeError::Network(ref message))
                if message == "worker queue full"
        )));
        drop(inner);

        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
        let _ = orch.finish_session(SessionId(1));
    }

    #[test]
    fn test_session_reentry_is_rejected_without_replacing_active_session() {
        let t = Arc::new(FixedTranscriber("new session".to_string()));
        let orch = default_orchestrator(t);

        orch.start_session(SessionId(1)).unwrap();
        let _ = orch.on_chunk_ready(SessionId(1), test_chunk());

        let error = orch.start_session(SessionId(2)).unwrap_err();
        assert_eq!(
            error,
            SessionStartError::ActiveSession {
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

        assert!(matches!(
            orch.on_chunk_ready(SessionId(2), test_chunk()),
            Err(SessionRoutingError::SessionMismatch { .. })
        ));
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
        let _ = orch.on_chunk_ready(SessionId(1), test_chunk());
        let result = orch.finish_session(SessionId(1));

        assert!(matches!(result, Err(SessionError::PartialFailure { .. })));
    }
}
