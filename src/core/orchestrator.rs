//! Session orchestrator for end-to-end stream recognition.
//!
//! `SessionOrchestrator` unifies the Hold and Toggle recording session lifecycle:
//! chunk tracking, background transcription, convergence wait, and result merging.

use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::transcriber::Transcriber;
// Re-exported so callers can keep using `core::orchestrator::TranscribeError`.
pub use crate::transcriber::TranscribeError;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Which hotkey mode initiated the recording session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Hold,
    Toggle,
}

/// Error returned by `SessionOrchestrator::stop_session`.
#[derive(Debug)]
pub enum SessionError {
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
    /// Written to disk; not yet picked up by the worker.
    Flushed,
    /// Worker is transcribing this chunk.
    /// `attempt` is reserved for future retry logic (see `max_retries` in AppConfig);
    /// retry is not yet implemented — on failure the chunk transitions directly to `Failed`.
    Uploading {
        #[allow(dead_code)]
        attempt: u32,
    },
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
    Chunk { index: usize, path: String },
}

struct ActiveSessionInner {
    #[allow(dead_code)]
    mode: SessionMode,
    chunks: Arc<Mutex<Vec<ChunkEntry>>>,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    worker: thread::JoinHandle<()>,
    next_index: usize,
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
    /// - `language`: passed to `merge_texts` for separator selection.
    /// - `convergence_timeout`: how long `stop_session` waits for background chunks.
    pub fn new(
        transcriber: Arc<dyn Transcriber>,
        language: Option<String>,
        convergence_timeout: Duration,
    ) -> Self {
        Self {
            transcriber,
            language,
            convergence_timeout,
            inner: Mutex::new(None),
        }
    }

    /// Start a new recording session.
    ///
    /// If a previous session is still active it is discarded (its background
    /// worker will finish its current transcription and then exit cleanly when
    /// the channel is dropped).
    pub fn start_session(&self, mode: SessionMode) {
        let chunks: Arc<Mutex<Vec<ChunkEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<WorkerMsg>(64);

        let worker_chunks = Arc::clone(&chunks);
        let transcriber = Arc::clone(&self.transcriber);

        let worker = thread::spawn(move || {
            worker_loop(chunk_rx, worker_chunks, transcriber);
        });

        let mut inner = self.inner.lock().unwrap();
        if inner.is_some() {
            warn!(
                "start_session called while a session is already active; discarding previous session"
            );
        }
        *inner = Some(ActiveSessionInner {
            mode,
            chunks,
            chunk_tx,
            worker,
            next_index: 0,
        });

        info!(mode = ?mode, "Session started");
    }

    /// Submit a chunk file path for background transcription.
    ///
    /// The worker thread will call `transcriber.transcribe(&path)` and delete the
    /// file after processing (success or failure). Returns the assigned chunk index,
    /// or `None` if no session is active.
    pub fn on_chunk_ready(&self, path: String) -> Option<usize> {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.as_mut() else {
            warn!(path = %path, "Chunk arrived without an active session; deleting it");
            remove_chunk_file(&path, "orphan");
            return None;
        };

        let index = session.next_index;
        session.next_index += 1;

        session.chunks.lock().unwrap().push(ChunkEntry {
            index,
            state: ChunkState::Flushed,
        });

        if let Err(e) = session.chunk_tx.try_send(WorkerMsg::Chunk {
            index,
            path: path.clone(),
        }) {
            let message = match e {
                mpsc::TrySendError::Full(_) => "worker queue full",
                mpsc::TrySendError::Disconnected(_) => "worker channel closed",
            };
            error!(path = %path, error = message, "Failed to enqueue chunk; marking as failed");
            let mut chunks = session.chunks.lock().unwrap();
            if let Some(entry) = chunks.iter_mut().find(|e| e.index == index) {
                entry.state = ChunkState::Failed(TranscribeError::Network(message.to_string()));
            }
            remove_chunk_file(&path, "rejected");
        } else {
            info!(index = index, path = %path, "Chunk enqueued for background transcription");
        }

        Some(index)
    }

    /// Stop the current session and block until all chunks reach a terminal state
    /// (or `convergence_timeout` elapses).
    ///
    /// Returns:
    /// - `Ok(text)` — all chunks succeeded; `text` is the language-aware merge.
    /// - `Err(SessionError::NoChunks)` — recording produced no chunks.
    /// - `Err(SessionError::PartialFailure { … })` — some chunks failed; partial text included.
    /// - `Err(SessionError::ConvergenceTimeout { … })` — timeout hit; partial text included.
    pub fn stop_session(&self) -> Result<String, SessionError> {
        let active = {
            let mut inner = self.inner.lock().unwrap();
            inner.take()
        };

        let Some(session) = active else {
            return Err(SessionError::NoChunks);
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

        let chunks = Arc::clone(&session.chunks);
        let deadline = Instant::now() + self.convergence_timeout;
        let mut timed_out = false;

        loop {
            let all_terminal = chunks.lock().unwrap().iter().all(|e| e.state.is_terminal());
            if all_terminal {
                break;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                let mut locked = chunks.lock().unwrap();
                let pending_count = locked.iter().filter(|e| !e.state.is_terminal()).count();
                warn!(
                    pending_count = pending_count,
                    "Convergence timeout; marking pending chunks as Failed(Timeout)"
                );
                for entry in locked.iter_mut() {
                    if !entry.state.is_terminal() {
                        entry.state = ChunkState::Failed(TranscribeError::Timeout);
                    }
                }
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        if !timed_out {
            // Worker should have processed Done and exited; join to clean up.
            let _ = session.worker.join();
        } else {
            // Drop the handle without joining — the worker may still be mid-request.
            // It will finish naturally; its Arc<Mutex<Vec<ChunkEntry>>> clone keeps
            // the data valid until the thread exits.
            drop(session.worker);
        }

        let locked = chunks.lock().unwrap();

        if timed_out {
            let texts = collect_transcribed_texts(&locked);
            let pending_count = locked
                .iter()
                .filter(|e| matches!(e.state, ChunkState::Failed(TranscribeError::Timeout)))
                .count();
            return Err(SessionError::ConvergenceTimeout {
                pending_count,
                partial_text: merge_texts(&texts, self.language.as_deref()),
            });
        }

        collect_results(&locked, self.language.as_deref())
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

fn worker_loop(
    rx: mpsc::Receiver<WorkerMsg>,
    chunks: Arc<Mutex<Vec<ChunkEntry>>>,
    transcriber: Arc<dyn Transcriber>,
) {
    for msg in rx {
        match msg {
            WorkerMsg::Chunk { index, path } => {
                // Transition to Uploading.
                {
                    let mut locked = chunks.lock().unwrap();
                    if let Some(entry) = locked.iter_mut().find(|e| e.index == index)
                        && !begin_upload(entry)
                    {
                        drop(locked);
                        remove_chunk_file(&path, "skipped");
                        continue;
                    }
                }

                debug!(index = index, path = %path, "Worker transcribing chunk");
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    transcriber.transcribe(&path)
                }))
                .unwrap_or_else(|_| {
                    Err(TranscribeError::Network("transcriber panicked".to_string()))
                });

                // Clean up the chunk file (ignore errors — file may already be gone).
                remove_chunk_file(&path, "processed");

                // Record outcome.
                let mut locked = chunks.lock().unwrap();
                if let Some(entry) = locked.iter_mut().find(|e| e.index == index) {
                    record_worker_result(entry, result);
                }
            }
        }
    }
    debug!("Worker channel closed, exiting");
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn record_worker_result(entry: &mut ChunkEntry, result: Result<String, TranscribeError>) {
    // A convergence timeout is terminal from the caller's perspective. A late
    // HTTP response must not rewrite the snapshot used for the timeout result.
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
            "Skipping upload for chunk that is no longer flushed"
        );
        return false;
    }
    entry.state = ChunkState::Uploading { attempt: 1 };
    true
}

fn remove_chunk_file(path: &str, reason: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        debug!(path, reason, error = %e, "Could not delete chunk file");
    }
}

/// Language-aware text merging (duplicated from `transcriber::api` to avoid
/// cross-module coupling; kept intentionally minimal).
fn merge_texts(texts: &[String], language: Option<&str>) -> String {
    let sep = match language {
        Some(lang) if lang.starts_with("zh") => "",
        _ => " ",
    };
    texts
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(sep)
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
        SessionOrchestrator::new(transcriber, Some("en".to_string()), timeout)
    }

    fn default_orchestrator(transcriber: Arc<dyn Transcriber>) -> SessionOrchestrator {
        make_orchestrator_with_timeout(transcriber, Duration::from_secs(5))
    }

    // ── Mock transcribers ────────────────────────────────────────────────────

    /// Always returns a fixed text; never touches the file system.
    struct FixedTranscriber(String);

    impl Transcriber for FixedTranscriber {
        fn transcribe(&self, _path: &str) -> Result<String, TranscribeError> {
            Ok(self.0.clone())
        }
    }

    /// Returns pre-configured results in call order.
    struct ScriptedTranscriber {
        results: Vec<Result<String, TranscribeError>>,
        call_count: AtomicUsize,
    }

    impl ScriptedTranscriber {
        fn new(results: Vec<Result<String, TranscribeError>>) -> Self {
            Self {
                results,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl Transcriber for ScriptedTranscriber {
        fn transcribe(&self, _path: &str) -> Result<String, TranscribeError> {
            let i = self.call_count.fetch_add(1, Ordering::SeqCst);
            match self.results.get(i) {
                Some(Ok(s)) => Ok(s.clone()),
                Some(Err(e)) => Err(e.clone()),
                None => Ok("extra".to_string()),
            }
        }
    }

    /// Sleeps for `delay` before returning.
    struct SlowTranscriber {
        delay: Duration,
    }

    struct GateTranscriber {
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Transcriber for GateTranscriber {
        fn transcribe(&self, _path: &str) -> Result<String, TranscribeError> {
            let (lock, condvar) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = condvar.wait(released).unwrap();
            }
            Ok("released".to_string())
        }
    }

    impl Transcriber for SlowTranscriber {
        fn transcribe(&self, _path: &str) -> Result<String, TranscribeError> {
            thread::sleep(self.delay);
            Ok("slow".to_string())
        }
    }

    /// Always panics — used to test worker-panic handling.
    struct PanicTranscriber;

    impl Transcriber for PanicTranscriber {
        fn transcribe(&self, _path: &str) -> Result<String, TranscribeError> {
            panic!("intentional transcriber panic for testing");
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn test_single_chunk_success() {
        let t = Arc::new(FixedTranscriber("hello world".to_string()));
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("chunk0.wav".to_string());
        let result = orch.stop_session();

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_multi_chunk_ordered_merge() {
        // Three chunks whose transcriptions arrive in-order.
        let t = Arc::new(ScriptedTranscriber::new(vec![
            Ok("one".to_string()),
            Ok("two".to_string()),
            Ok("three".to_string()),
        ]));
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("c0.wav".to_string()); // index 0
        orch.on_chunk_ready("c1.wav".to_string()); // index 1
        orch.on_chunk_ready("c2.wav".to_string()); // index 2
        let result = orch.stop_session();

        assert!(result.is_ok());
        // Chunks must be joined in submission order, not completion order.
        assert_eq!(result.unwrap(), "one two three");
    }

    #[test]
    fn test_no_chunks_returns_error() {
        let t = Arc::new(MockTranscriber);
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Toggle);
        let result = orch.stop_session();

        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn test_stop_session_without_start_returns_no_chunks() {
        let t = Arc::new(MockTranscriber);
        let orch = default_orchestrator(t);

        // stop_session called without a prior start_session.
        let result = orch.stop_session();
        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn test_chunk_without_active_session_is_deleted() {
        let path = std::env::temp_dir().join(format!(
            "viberwhisper-orphan-chunk-{}-{:?}.wav",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"orphan chunk").unwrap();
        let orch = default_orchestrator(Arc::new(MockTranscriber));

        let result = orch.on_chunk_ready(path.to_string_lossy().into_owned());

        assert!(result.is_none());
        assert!(!path.exists(), "orphan chunk file was not deleted");
    }

    #[test]
    fn test_partial_failure_returns_error_with_partial_text() {
        let t = Arc::new(ScriptedTranscriber::new(vec![
            Ok("good chunk".to_string()),
            Err(TranscribeError::Api {
                status: 500,
                body: "server error".to_string(),
            }),
            Ok("another good".to_string()),
        ]));
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("c0.wav".to_string());
        orch.on_chunk_ready("c1.wav".to_string());
        orch.on_chunk_ready("c2.wav".to_string());
        let result = orch.stop_session();

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
        // Worker sleeps 500 ms per chunk; timeout is 100 ms — should time out.
        let t = Arc::new(SlowTranscriber {
            delay: Duration::from_millis(500),
        });
        let orch = make_orchestrator_with_timeout(t, Duration::from_millis(100));

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("slow.wav".to_string());
        let result = orch.stop_session();

        match result {
            Err(SessionError::ConvergenceTimeout { pending_count, .. }) => {
                assert_eq!(pending_count, 1);
            }
            other => panic!("Expected ConvergenceTimeout, got {:?}", other),
        }
    }

    #[test]
    fn test_timed_out_chunk_cannot_be_overwritten_by_late_worker_result() {
        let mut entry = ChunkEntry {
            index: 0,
            state: ChunkState::Failed(TranscribeError::Timeout),
        };

        assert!(!begin_upload(&mut entry));
        record_worker_result(&mut entry, Ok("late result".to_string()));

        assert!(matches!(
            entry.state,
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
        orch.start_session(SessionMode::Hold);

        let started = Instant::now();
        for index in 0..100 {
            orch.on_chunk_ready(format!("queue-{index}.wav"));
        }

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "enqueueing blocked on a full worker queue"
        );
        let inner = orch.inner.lock().unwrap();
        let chunks = inner.as_ref().unwrap().chunks.lock().unwrap();
        assert!(chunks.iter().any(|entry| matches!(
            entry.state,
            ChunkState::Failed(TranscribeError::Network(ref message))
                if message == "worker queue full"
        )));
        drop(chunks);
        drop(inner);

        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
        let _ = orch.stop_session();
    }

    #[test]
    fn test_hold_and_toggle_same_lifecycle() {
        // Both modes should go through the same start/stop path.
        for mode in [SessionMode::Hold, SessionMode::Toggle] {
            let t = Arc::new(FixedTranscriber("text".to_string()));
            let orch = default_orchestrator(t);

            orch.start_session(mode);
            orch.on_chunk_ready("chunk.wav".to_string());
            let result = orch.stop_session();

            assert!(result.is_ok(), "Mode {:?} failed: {:?}", mode, result);
            assert_eq!(result.unwrap(), "text");
        }
    }

    #[test]
    fn test_session_reentry_starts_fresh() {
        // start_session while a previous session exists should start fresh.
        let t = Arc::new(FixedTranscriber("new session".to_string()));
        let orch = default_orchestrator(t);

        // First session: submit a chunk but do NOT call stop_session.
        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("old_chunk.wav".to_string());

        // Second session: replaces the first.
        orch.start_session(SessionMode::Toggle);
        orch.on_chunk_ready("new_chunk.wav".to_string());
        let result = orch.stop_session();

        // Only the new session's chunk should appear.
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(result.unwrap(), "new session");
    }

    #[test]
    fn test_worker_panic_marks_chunks_failed_via_timeout() {
        // PanicTranscriber panics; the worker thread dies; convergence times out.
        let t = Arc::new(PanicTranscriber);
        let orch = make_orchestrator_with_timeout(t, Duration::from_millis(200));

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("panic.wav".to_string());
        let result = orch.stop_session();

        // Worker panicked → chunk never reaches terminal state → ConvergenceTimeout.
        assert!(
            matches!(
                result,
                Err(SessionError::ConvergenceTimeout { .. })
                    | Err(SessionError::PartialFailure { .. })
            ),
            "Expected timeout or partial failure, got {:?}",
            result
        );
    }

    #[test]
    fn test_worker_panic_removes_chunk_file_and_reports_failure() {
        let path = std::env::temp_dir().join(format!(
            "viberwhisper-panic-chunk-{}-{:?}.wav",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"test chunk").unwrap();
        let t = Arc::new(PanicTranscriber);
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready(path.to_string_lossy().into_owned());
        let result = orch.stop_session();

        assert!(matches!(result, Err(SessionError::PartialFailure { .. })));
        assert!(!path.exists(), "panicking worker leaked chunk file");
    }

    #[test]
    fn test_merge_texts_zh() {
        let texts = vec!["你好".to_string(), "世界".to_string()];
        assert_eq!(merge_texts(&texts, Some("zh")), "你好世界");
    }

    #[test]
    fn test_merge_texts_en() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        assert_eq!(merge_texts(&texts, Some("en")), "hello world");
    }

    #[test]
    fn test_merge_texts_empty_filtered() {
        let texts = vec!["a".to_string(), "".to_string(), "b".to_string()];
        assert_eq!(merge_texts(&texts, None), "a b");
    }
}
