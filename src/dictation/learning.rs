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
    config::MemoryConfig,
    feedback::{Confirmation, Transcript},
    memory::{Memory, Snapshot},
    result::DictationResult,
};

enum Command {
    Snapshot(mpsc::SyncSender<Snapshot>),
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
    ) -> Result<Self> {
        settings.validate()?;
        let (commands, receiver) = async_mpsc::channel(1);
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let thread = std::thread::Builder::new()
            .name("http-cleanup-memory".into())
            .spawn(move || runtime.block_on(worker(settings, path, language, receiver, stop)))?;
        Ok(Self {
            commands,
            shutdown,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Called on the finalization thread, never on the UI thread.
    pub fn snapshot(&self) -> Result<Snapshot> {
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
    mut receiver: async_mpsc::Receiver<Command>,
    stop: CancellationToken,
) {
    // Match engine::worker recovery: never overwrite damaged or unknown-version storage.
    let mut memory = Memory::load(path, settings.clone(), now()).unwrap_or_else(|error| {
        tracing::warn!(%error, "Cleanup memory unavailable; using ephemeral memory without overwriting the file");
        Memory::load(None, settings, now()).expect("empty memory")
    });
    loop {
        let command = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            command = receiver.recv() => command,
        };
        match command {
            Some(Command::Snapshot(reply)) => {
                let _ = reply.send(memory.snapshot(now()));
            }
            Some(Command::Prepare(result, reply)) => {
                let prepared = memory.prepare(vec![result], language.clone());
                let (confirmation, mut feedback) = Confirmation::channel();
                let _ = reply.send(Transcript {
                    text: prepared.text.clone(),
                    confirmation: Some(confirmation),
                });
                let text = tokio::select! {
                    biased;
                    text = &mut feedback => text.ok(),
                    _ = stop.cancelled() => {
                        // As in engine::worker, close before draining so a racing
                        // successful send is persisted, while an open preview cannot block exit.
                        feedback.close();
                        feedback.try_recv().ok()
                    }
                };
                if let Some(text) = text {
                    let recording = format!(
                        "http-{}-{}",
                        std::process::id(),
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    );
                    memory.confirm(&recording, &prepared, &text, now());
                    if let Err(error) = memory.save() {
                        tracing::warn!(%error, "Could not save confirmed cleanup memory");
                    }
                }
            }
            None => break,
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn http_and_realtime_memory_share_confirmed_rules_without_relearning() {
        // Seed through the exact Memory API used by Realtime, then confirm an
        // unchanged HTTP correction. Automatic output must not become manual evidence.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation-memory.json");
        let settings = MemoryConfig::default();
        let source = DictationResult::parse(&json!({
            "text":"使用维伯耳语。", "context":{"kind":"new","label":"项目"},
            "memory_candidates":[{"kind":"proper_noun","text":"维伯耳语","quote":"使用维伯耳语。"}]
        }).to_string(), &[]).unwrap();
        let mut realtime = Memory::load(Some(path.clone()), settings.clone(), now()).unwrap();
        let prepared = realtime.prepare(vec![source.clone()], Some("zh".into()));
        realtime.confirm("realtime-source", &prepared, "使用ViberWhisper。", now());
        realtime.save().unwrap();
        let worker =
            LearningWorker::new(settings.clone(), Some(path.clone()), Some("zh".into())).unwrap();
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

        let worker =
            LearningWorker::new(MemoryConfig::default(), Some(path.clone()), None).unwrap();
        let unconfirmed = worker
            .prepare(
                DictationResult::parse(
                    &json!({"text":"未确认", "context":{"kind":"new","label":"其他"}}).to_string(),
                    &[],
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
                .confirm("不能提交".into())
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
