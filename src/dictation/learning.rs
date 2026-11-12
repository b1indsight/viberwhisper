//! Serial memory ownership for HTTP cleanup, independent of Realtime network I/O.

use std::{
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tokio::sync::mpsc as async_mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    Clock,
    config::MemoryConfig,
    feedback::{Confirmation, Transcript},
    memory::{Memory, Snapshot},
    result::DictationResult,
};

enum Command {
    Snapshot(mpsc::SyncSender<(Snapshot, u64)>),
    Prepare(DictationResult, mpsc::SyncSender<Transcript>),
}

/// One writer orders accepted confirmation feedback before the next HTTP request snapshot.
pub(crate) struct LearningWorker {
    commands: async_mpsc::Sender<Command>,
    shutdown: CancellationToken,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl LearningWorker {
    pub fn new(
        settings: MemoryConfig,
        path: Option<PathBuf>,
        language: Option<String>,
        clock: Clock,
    ) -> Result<Self> {
        settings.validate()?;
        let (commands, receiver) = async_mpsc::channel(1);
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let thread = std::thread::Builder::new()
            .name("http-cleanup-memory".into())
            .spawn(move || {
                runtime.block_on(worker(settings, path, language, clock, receiver, stop))
            })?;
        Ok(Self {
            commands,
            shutdown,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Called on the finalization thread, never on the UI thread.
    #[cfg(test)]
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.snapshot_with_time().map(|(snapshot, _)| snapshot)
    }

    /// Capture the snapshot and its Unix-seconds retention time together on the owner.
    /// Like `snapshot`, this waits for any pending confirmation on a finalization thread.
    pub fn snapshot_with_time(&self) -> Result<(Snapshot, u64)> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.commands
            .blocking_send(Command::Snapshot(sender))
            .context("cleanup memory worker stopped")?;
        receiver.recv().context("cleanup memory worker stopped")
    }

    /// Keep source metadata in the worker until the single-use preview feedback resolves.
    pub fn prepare(&self, result: DictationResult) -> Result<Transcript> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.commands
            .blocking_send(Command::Prepare(result, sender))
            .context("cleanup memory worker stopped")?;
        receiver.recv().context("cleanup memory worker stopped")
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        if let Some(thread) = self.thread.lock().unwrap().take()
            && thread.join().is_err()
        {
            tracing::warn!("Cleanup memory worker panicked during shutdown");
        }
    }
}

impl Drop for LearningWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn worker(
    settings: MemoryConfig,
    path: Option<PathBuf>,
    language: Option<String>,
    clock: Clock,
    mut receiver: async_mpsc::Receiver<Command>,
    stop: CancellationToken,
) {
    let mut memory = Memory::load_or_ephemeral(path, settings, clock());
    loop {
        let command = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            command = receiver.recv() => command,
        };
        match command {
            Some(Command::Snapshot(reply)) => {
                let at = clock();
                let _ = reply.send((memory.snapshot(at), at));
            }
            Some(Command::Prepare(result, reply)) => {
                let prepared = memory.prepare(vec![result], language.clone());
                let (confirmation, mut feedback) = Confirmation::channel();
                let _ = reply.send(Transcript {
                    text: prepared.text.clone(),
                    confirmation: Some(confirmation),
                });
                let feedback = tokio::select! {
                    biased;
                    feedback = &mut feedback => feedback.ok(),
                    _ = stop.cancelled() => {
                        // As in engine::worker, close before draining so a racing
                        // successful send is persisted, while an open preview cannot block exit.
                        feedback.close();
                        feedback.try_recv().ok()
                    }
                };
                if let Some(feedback) = feedback {
                    let recording = format!(
                        "http-{}-{}",
                        std::process::id(),
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    let at = clock();
                    memory.confirm(&recording, &prepared, &feedback.text, at);
                    if let Err(error) = memory.save() {
                        tracing::warn!(%error, "Could not save confirmed cleanup memory");
                    }
                    feedback.acknowledge(at);
                }
            }
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> u64 {
        super::super::system_clock()()
    }

    #[test]
    fn injected_clock_orders_confirmed_cleanup_and_retention() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        // A later lab snapshot must not supply the feedback time. Advance on
        // every read as well, so a second clock read cannot fabricate an ACK.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let seed: super::super::MemorySeed = serde_json::from_value(json!({
            "observations":[{"recording":"seed", "segment":0,"at":99,"context":"project","text":"seed history","terms":[]}]
        })).unwrap();
        seed.write(&path, 100).unwrap();
        let time = Arc::new(AtomicU64::new(100));
        let clock_time = Arc::clone(&time);
        let worker = LearningWorker::new(
            MemoryConfig::default(),
            Some(path.clone()),
            None,
            Arc::new(move || clock_time.fetch_add(1, Ordering::SeqCst)),
        )
        .unwrap();
        let (snapshot, snapshot_at) = worker.snapshot_with_time().unwrap();
        assert_eq!(snapshot.contexts[0].history, vec!["seed history"]);
        assert_eq!(snapshot_at, 101);
        assert_eq!(time.load(Ordering::SeqCst), 102);
        let result = worker
            .prepare(
                DictationResult::parse(
                    &json!({"text":"confirmed text", "context":"project", "memory_candidates":[]})
                        .to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        time.store(200, Ordering::SeqCst);
        assert_eq!(
            result
                .confirmation
                .unwrap()
                .confirm_with_ack(result.text)
                .unwrap(),
            200
        );
        assert_eq!(time.load(Ordering::SeqCst), 201);
        time.store(300, Ordering::SeqCst);
        let (snapshot, snapshot_at) = worker.snapshot_with_time().unwrap();
        assert_eq!(snapshot.contexts[0].history[0], "confirmed text");
        assert_eq!(snapshot_at, 300);
        assert_eq!(time.load(Ordering::SeqCst), 301);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["observations"][1]["at"], 200);
        time.store(86_600, Ordering::SeqCst);
        assert!(worker.snapshot().unwrap().history_is_empty());
    }

    #[test]
    fn http_and_realtime_memory_share_confirmed_rules_without_relearning() {
        // Seed through the exact Memory API used by Realtime, then confirm an
        // unchanged HTTP correction. Automatic output must not become manual evidence.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation-memory.json");
        let settings = MemoryConfig::default();
        let source = DictationResult::parse(
            &json!({
                "text":"使用维伯耳语。", "context":"项目",
                "memory_candidates":["维伯耳语"]
            })
            .to_string(),
        )
        .unwrap();
        let mut realtime = Memory::load(Some(path.clone()), settings.clone(), now()).unwrap();
        let prepared = realtime.prepare(vec![source.clone()], Some("zh".into()));
        realtime.confirm("realtime-source", &prepared, "使用ViberWhisper。", now());
        realtime.save().unwrap();
        let worker = LearningWorker::new(
            settings.clone(),
            Some(path.clone()),
            Some("zh".into()),
            super::super::system_clock(),
        )
        .unwrap();
        assert_eq!(
            worker.snapshot().unwrap().contexts[0].corrections[0].to,
            "ViberWhisper"
        );
        let shown = worker.prepare(source).unwrap();
        assert_eq!(shown.text, "使用ViberWhisper。");
        shown.confirmation.unwrap().confirm(shown.text).unwrap();
        worker.shutdown();
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["corrections"][0]["recording"], "realtime-source");
        let reloaded = Memory::load(Some(path.clone()), settings, now()).unwrap();
        assert_eq!(
            reloaded.snapshot(now()).contexts[0].corrections[0].to,
            "ViberWhisper"
        );

        let worker = LearningWorker::new(
            MemoryConfig::default(),
            Some(path.clone()),
            None,
            super::super::system_clock(),
        )
        .unwrap();
        let unconfirmed = worker
            .prepare(
                DictationResult::parse(
                    &json!({"text":"未确认", "context":"其他", "memory_candidates":[]}).to_string(),
                )
                .unwrap(),
            )
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        worker.shutdown();
        assert!(
            unconfirmed
                .confirmation
                .unwrap()
                .confirm_with_ack("不能提交".into())
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
