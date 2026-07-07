//! Session orchestrator for end-to-end stream recognition.
//!
//! `SessionOrchestrator` unifies the Hold and Toggle recording session lifecycle:
//! chunk tracking, background transcription, convergence wait, and result merging.

use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, error, info, warn};

use crate::transcriber::{Transcriber, merge_texts};
// Re-exported so callers can keep using `core::orchestrator::TranscribeError`.
pub use crate::transcriber::TranscribeError;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Which hotkey mode initiated the recording session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Hold,
    Toggle,
}

/// Successful result of `SessionOrchestrator::stop_session`.
#[derive(Debug)]
pub struct SessionOutput {
    /// Language-aware merge of every transcribed chunk in the session.
    pub full_text: String,
    /// Merge of only the chunks that were never handed out through
    /// `take_stable_texts`. Equals `full_text` when streaming consumption was
    /// not used. An incremental post-process session that already received
    /// the consumed prefix only needs this remainder before `finish()`.
    pub unconsumed_text: String,
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
    /// A worker is transcribing this chunk. Upload retries live inside the
    /// transcriber (`ApiTranscriber`), so the orchestrator only tracks the
    /// coarse state.
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
    Chunk { index: usize, path: String },
}

/// Chunk store shared between the session and its workers. The condvar is
/// signalled whenever a chunk reaches a terminal state so `stop_session` can
/// wait for convergence without polling.
type ChunkStore = Arc<(Mutex<Vec<ChunkEntry>>, Condvar)>;

struct ActiveSessionInner {
    chunks: ChunkStore,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    workers: Vec<thread::JoinHandle<()>>,
    next_index: usize,
    /// Index of the first chunk not yet handed out via `take_stable_texts`.
    stable_consumed: usize,
}

/// Number of background transcription workers per session. Long recordings
/// produce chunks faster than one upload round-trip completes; a few parallel
/// uploads keep the convergence wait short while results are still merged in
/// submission order via chunk indices.
const WORKER_THREADS: usize = 3;

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
        let chunks: ChunkStore = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<WorkerMsg>(64);

        let shared_rx = Arc::new(Mutex::new(chunk_rx));
        let workers = (0..WORKER_THREADS)
            .map(|_| {
                let rx = Arc::clone(&shared_rx);
                let worker_chunks = Arc::clone(&chunks);
                let transcriber = Arc::clone(&self.transcriber);
                thread::spawn(move || worker_loop(rx, worker_chunks, transcriber))
            })
            .collect();

        let mut inner = self.inner.lock().unwrap();
        if inner.is_some() {
            warn!(
                "start_session called while a session is already active; discarding previous session"
            );
        }
        *inner = Some(ActiveSessionInner {
            chunks,
            chunk_tx,
            workers,
            next_index: 0,
            stable_consumed: 0,
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

        session.chunks.0.lock().unwrap().push(ChunkEntry {
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
            {
                let mut chunks = session.chunks.0.lock().unwrap();
                if let Some(entry) = chunks.iter_mut().find(|e| e.index == index) {
                    entry.state = ChunkState::Failed(TranscribeError::Network(message.to_string()));
                }
            }
            session.chunks.1.notify_all();
            remove_chunk_file(&path, "rejected");
        } else {
            info!(index = index, path = %path, "Chunk enqueued for background transcription");
        }

        Some(index)
    }

    /// Hand out the texts of chunks that became "stable" since the last call:
    /// the maximal prefix (in submission order) whose chunks have all reached
    /// a terminal state. Each text is returned exactly once. Failed chunks
    /// contribute no text but still advance the prefix, matching how
    /// `stop_session` merges results.
    ///
    /// The main loop calls this while recording to feed the LLM post-process
    /// preheat session (see `postprocess::llm` for the compatibility notes on
    /// the non-streaming chat API). Returns an empty vec when no session is
    /// active or nothing new is stable yet.
    pub fn take_stable_texts(&self) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.as_mut() else {
            return Vec::new();
        };

        let chunks = session.chunks.0.lock().unwrap();
        let mut texts = Vec::new();
        loop {
            let next_index = session.stable_consumed;
            let Some(entry) = chunks.iter().find(|e| e.index == next_index) else {
                break;
            };
            match &entry.state {
                ChunkState::Transcribed(text) => {
                    if !text.is_empty() {
                        texts.push(text.clone());
                    }
                }
                ChunkState::Failed(_) => {}
                // Prefix is not stable yet — stop here and retry next poll.
                _ => break,
            }
            session.stable_consumed += 1;
        }
        texts
    }

    /// Stop the current session and block until all chunks reach a terminal state
    /// (or `convergence_timeout` elapses).
    ///
    /// Returns:
    /// - `Ok(SessionOutput)` — all chunks succeeded; carries the full merge and
    ///   the not-yet-consumed remainder (see `SessionOutput`).
    /// - `Err(SessionError::NoChunks)` — recording produced no chunks.
    /// - `Err(SessionError::PartialFailure { … })` — some chunks failed; partial text included.
    /// - `Err(SessionError::ConvergenceTimeout { … })` — timeout hit; partial text included.
    pub fn stop_session(&self) -> Result<SessionOutput, SessionError> {
        let active = {
            let mut inner = self.inner.lock().unwrap();
            inner.take()
        };

        let Some(session) = active else {
            return Err(SessionError::NoChunks);
        };

        if session.next_index == 0 {
            // Closing the channel lets the idle workers exit immediately.
            drop(session.chunk_tx);
            for worker in session.workers {
                let _ = worker.join();
            }
            return Err(SessionError::NoChunks);
        }

        // Closing the sender is non-blocking. The receiver still drains every
        // queued chunk before its iterator ends, so the convergence deadline
        // covers the entire shutdown rather than starting after a blocking send.
        drop(session.chunk_tx);

        let chunks = Arc::clone(&session.chunks);
        let stable_consumed = session.stable_consumed;
        let deadline = Instant::now() + self.convergence_timeout;
        let mut timed_out = false;

        {
            let (lock, cvar) = &*chunks;
            let mut locked = lock.lock().unwrap();
            loop {
                if locked.iter().all(|e| e.state.is_terminal()) {
                    break;
                }
                let now = Instant::now();
                if now >= deadline {
                    timed_out = true;
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
                // Woken by workers on every terminal state transition; the
                // timeout bound also covers a worker dying without notifying.
                let (guard, _) = cvar.wait_timeout(locked, deadline - now).unwrap();
                locked = guard;
            }
        }

        if !timed_out {
            // Workers should have drained the queue and exited; join to clean up.
            for worker in session.workers {
                let _ = worker.join();
            }
        } else {
            // Drop the handles without joining — workers may still be mid-request.
            // They finish naturally; their ChunkStore clones keep the data valid
            // until each thread exits.
            drop(session.workers);
        }

        let locked = chunks.0.lock().unwrap();

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

        collect_results(&locked, stable_consumed, self.language.as_deref())
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

fn worker_loop(
    rx: Arc<Mutex<mpsc::Receiver<WorkerMsg>>>,
    chunks: ChunkStore,
    transcriber: Arc<dyn Transcriber>,
) {
    loop {
        // Take the receiver lock only for the blocking recv itself; workers
        // waiting on this mutex are exactly the idle ones.
        let msg = rx.lock().unwrap().recv();
        match msg {
            Err(_) => break,
            Ok(WorkerMsg::Chunk { index, path }) => {
                // Transition to Uploading.
                {
                    let mut locked = chunks.0.lock().unwrap();
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

                // Record outcome and wake the convergence waiter.
                {
                    let mut locked = chunks.0.lock().unwrap();
                    if let Some(entry) = locked.iter_mut().find(|e| e.index == index) {
                        record_worker_result(entry, result);
                    }
                }
                chunks.1.notify_all();
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
    entry.state = ChunkState::Uploading;
    true
}

fn remove_chunk_file(path: &str, reason: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        debug!(path, reason, error = %e, "Could not delete chunk file");
    }
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

fn collect_results(
    chunks: &[ChunkEntry],
    stable_consumed: usize,
    language: Option<&str>,
) -> Result<SessionOutput, SessionError> {
    let mut ordered: Vec<&ChunkEntry> = chunks.iter().collect();
    ordered.sort_by_key(|e| e.index);

    let mut texts: Vec<String> = Vec::new();
    let mut unconsumed: Vec<String> = Vec::new();
    let mut errors: Vec<(usize, TranscribeError)> = Vec::new();

    for entry in ordered {
        match &entry.state {
            ChunkState::Transcribed(t) => {
                texts.push(t.clone());
                if entry.index >= stable_consumed {
                    unconsumed.push(t.clone());
                }
            }
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
        Ok(SessionOutput {
            full_text: merge_texts(&texts, language),
            unconsumed_text: merge_texts(&unconsumed, language),
        })
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

    /// Returns pre-configured results keyed by chunk path. Path-keyed rather
    /// than call-ordered because parallel workers pick up chunks in a
    /// nondeterministic order.
    struct ScriptedTranscriber {
        results: std::collections::HashMap<String, Result<String, TranscribeError>>,
    }

    impl ScriptedTranscriber {
        fn new<const N: usize>(results: [(&str, Result<String, TranscribeError>); N]) -> Self {
            Self {
                results: results
                    .into_iter()
                    .map(|(path, result)| (path.to_string(), result))
                    .collect(),
            }
        }
    }

    impl Transcriber for ScriptedTranscriber {
        fn transcribe(&self, path: &str) -> Result<String, TranscribeError> {
            self.results
                .get(path)
                .cloned()
                .unwrap_or_else(|| Ok("extra".to_string()))
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
        assert_eq!(result.unwrap().full_text, "hello world");
    }

    #[test]
    fn test_multi_chunk_ordered_merge() {
        let t = Arc::new(ScriptedTranscriber::new([
            ("c0.wav", Ok("one".to_string())),
            ("c1.wav", Ok("two".to_string())),
            ("c2.wav", Ok("three".to_string())),
        ]));
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("c0.wav".to_string()); // index 0
        orch.on_chunk_ready("c1.wav".to_string()); // index 1
        orch.on_chunk_ready("c2.wav".to_string()); // index 2
        let result = orch.stop_session();

        assert!(result.is_ok());
        // Chunks must be joined in submission order, not completion order.
        let output = result.unwrap();
        assert_eq!(output.full_text, "one two three");
        // take_stable_texts was never called, so nothing was consumed.
        assert_eq!(output.unconsumed_text, "one two three");
    }

    #[test]
    fn test_take_stable_texts_streams_prefix_in_order_once() {
        let t = Arc::new(ScriptedTranscriber::new([
            ("c0.wav", Ok("one".to_string())),
            ("c1.wav", Ok("two".to_string())),
        ]));
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("c0.wav".to_string());
        orch.on_chunk_ready("c1.wav".to_string());

        // Workers run in the background; poll until both chunks are stable.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut collected: Vec<String> = Vec::new();
        while collected.len() < 2 && Instant::now() < deadline {
            collected.extend(orch.take_stable_texts());
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(collected, vec!["one".to_string(), "two".to_string()]);
        // Consumed texts are handed out exactly once.
        assert!(orch.take_stable_texts().is_empty());

        let output = orch.stop_session().unwrap();
        assert_eq!(output.full_text, "one two");
        assert_eq!(output.unconsumed_text, "");
    }

    #[test]
    fn test_take_stable_texts_waits_for_prefix_completion() {
        // c0 is gated; c1 completes immediately. Nothing may be handed out
        // until c0 lands, because the merge order must match submission order.
        struct PrefixGateTranscriber {
            release: Arc<(Mutex<bool>, Condvar)>,
        }
        impl Transcriber for PrefixGateTranscriber {
            fn transcribe(&self, path: &str) -> Result<String, TranscribeError> {
                if path == "c0.wav" {
                    let (lock, condvar) = &*self.release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = condvar.wait(released).unwrap();
                    }
                    Ok("first".to_string())
                } else {
                    Ok("second".to_string())
                }
            }
        }

        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let t = Arc::new(PrefixGateTranscriber {
            release: Arc::clone(&release),
        });
        let orch = default_orchestrator(t);

        orch.start_session(SessionMode::Hold);
        orch.on_chunk_ready("c0.wav".to_string());
        orch.on_chunk_ready("c1.wav".to_string());

        // Give c1 time to finish; the prefix is still blocked on c0.
        thread::sleep(Duration::from_millis(200));
        assert!(orch.take_stable_texts().is_empty());

        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut collected: Vec<String> = Vec::new();
        while collected.len() < 2 && Instant::now() < deadline {
            collected.extend(orch.take_stable_texts());
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(collected, vec!["first".to_string(), "second".to_string()]);
        let output = orch.stop_session().unwrap();
        assert_eq!(output.full_text, "first second");
        assert_eq!(output.unconsumed_text, "");
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
        let t = Arc::new(ScriptedTranscriber::new([
            ("c0.wav", Ok("good chunk".to_string())),
            (
                "c1.wav",
                Err(TranscribeError::Api {
                    status: 500,
                    body: "server error".to_string(),
                }),
            ),
            ("c2.wav", Ok("another good".to_string())),
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
        let chunks = inner.as_ref().unwrap().chunks.0.lock().unwrap();
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
            assert_eq!(result.unwrap().full_text, "text");
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
        assert_eq!(result.unwrap().full_text, "new session");
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
}
