use anyhow::{Result, ensure};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc as async_mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    config::DictationConfig,
    feedback::{Confirmation, Transcript},
    memory::Memory,
    transport::Connection,
};
use crate::{
    audio::{PcmResampler, WavChunk},
    core::{config::SecretValue, orchestrator::SessionError},
    transcriber::TranscribeError,
};

// A bounded backlog of 200 ms recorder frames. This is local backpressure, not a server limit.
const FRAME_QUEUE: usize = 300;
const SEGMENT_SAMPLES: usize = 30 * 24_000;

/// Per-recording I/O owned by the shared session orchestrator.
pub(crate) struct RealtimeSession {
    sender: async_mpsc::Sender<WavChunk>,
    result: mpsc::Receiver<Result<Transcript, SessionError>>,
    cancelled: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
}

struct Job {
    frames: async_mpsc::Receiver<WavChunk>,
    result: mpsc::SyncSender<Result<Transcript, SessionError>>,
    cancelled: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
}

/// Keeps the Realtime connection and memory alive across recording sessions.
pub(crate) struct DictationWorker {
    jobs: async_mpsc::Sender<Job>,
    shutdown: CancellationToken,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DictationWorker {
    pub fn new(
        settings: DictationConfig,
        key: Option<SecretValue>,
        memory_path: Option<PathBuf>,
        language: Option<String>,
    ) -> Result<Self> {
        settings.validate()?;
        let memory = Memory::load(memory_path, settings.memory.clone(), now()).unwrap_or_else(|e| {
            tracing::warn!(error=%e, "Dictation memory unavailable; using ephemeral memory without overwriting the file");
            Memory::load(None, settings.memory.clone(), now()).expect("empty memory")
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (jobs, receiver) = async_mpsc::channel(1);
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let thread = std::thread::Builder::new()
            .name("realtime-dictation".into())
            .spawn(move || {
                runtime.block_on(worker(
                    settings,
                    key,
                    memory,
                    receiver,
                    language,
                    worker_shutdown,
                ));
                // The worker has finished; stalled blocking DNS must not delay thread exit.
                runtime.shutdown_background();
            })?;
        Ok(Self {
            jobs,
            shutdown,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Close pending previews and finish accepted feedback before the process exits.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
        let mut thread = self.thread.lock().unwrap();
        if let Some(thread) = thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("Realtime dictation worker panicked during shutdown");
        }
    }

    pub fn start_session(&self) -> RealtimeSession {
        let (sender, frames) = async_mpsc::channel(FRAME_QUEUE);
        let (result_tx, result) = mpsc::sync_channel(1);
        let cancelled = CancellationToken::new();
        let failure = Arc::new(Mutex::new(None));
        if self
            .jobs
            .try_send(Job {
                frames,
                result: result_tx,
                cancelled: cancelled.clone(),
                failure: Arc::clone(&failure),
            })
            .is_err()
        {
            *failure.lock().unwrap() = Some("Realtime worker unavailable".into());
        }
        RealtimeSession {
            sender,
            result,
            cancelled,
            failure,
        }
    }
}

impl Drop for DictationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl RealtimeSession {
    pub fn on_chunk_ready(&self, frame: WavChunk) {
        if self.sender.try_send(frame).is_err() {
            self.fail("Realtime audio backlog exceeded its bounded capacity or worker failed");
        }
    }

    pub fn recording_error(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }

    pub fn fail(&self, error: &str) {
        *self.failure.lock().unwrap() = Some(error.into());
        self.cancelled.cancel();
    }

    pub fn abort(self) {
        self.cancelled.cancel();
    }

    pub fn finish(self) -> Result<Transcript, SessionError> {
        drop(self.sender); // EOF follows all accepted frames, including the recorder tail.
        let outcome = self
            .result
            .recv()
            .unwrap_or_else(|_| Err(failed("Realtime worker stopped".into(), String::new())));
        if let Some(error) = self.failure.lock().unwrap().take()
            && outcome.is_ok()
        {
            return Err(failed(error, String::new()));
        }
        outcome
    }
}

fn failed(error: String, partial_text: String) -> SessionError {
    SessionError::PartialFailure {
        errors: vec![(0, TranscribeError::Network(error))],
        partial_text,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn worker(
    settings: DictationConfig,
    key: Option<SecretValue>,
    mut memory: Memory,
    mut jobs: async_mpsc::Receiver<Job>,
    language: Option<String>,
    shutdown: CancellationToken,
) {
    let mut connection: Option<Connection> = None;
    loop {
        let next = if let Some(socket) = &mut connection {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                job = jobs.recv() => job,
                _ = socket.idle() => { connection = None; continue; }
            }
        } else {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                job = jobs.recv() => job,
            }
        };
        let Some(mut job) = next else {
            break;
        };
        let mut working_memory = memory.clone();
        let mut recording = Recording::new();
        let mut pending = None;
        let result = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = job.cancelled.cancelled() => Err(anyhow::anyhow!("dictation cancelled")),
            result = async {
                if connection.is_none() { connection = Some(Connection::connect(&settings, key.as_ref().map(SecretValue::expose)).await?); }
                let socket = connection.as_mut().unwrap();
                loop {
                    let frame = tokio::select! {
                        frame = job.frames.recv() => frame,
                        event = socket.receive_while_recording() => { event?; continue; }
                    };
                    let Some(frame) = frame else { break; };
                    recording.push(socket, &mut working_memory, &frame).await?;
                }
                recording.finish(socket, &mut working_memory).await
            } => result,
        };
        let result = match result {
            Ok(()) if !job.cancelled.is_cancelled() => {
                let prepared =
                    memory.prepare(std::mem::take(&mut recording.results), language.clone());
                let (confirmation, receiver) = Confirmation::channel();
                let text = prepared.text.clone();
                pending = Some((prepared, receiver));
                Ok(Transcript {
                    text,
                    confirmation: Some(confirmation),
                })
            }
            other => {
                if job.cancelled.is_cancelled()
                    && let Some(socket) = &mut connection
                {
                    // Cancellation is sent using the standard event before retiring an interrupted connection.
                    tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => break,
                        result = socket.cancel() => {
                            if let Err(error) = result {
                                tracing::debug!(%error, "Retiring Realtime connection after cancellation cleanup failed");
                            }
                        }
                    }
                }
                connection = None; // Never replay an ambiguous audio commit after a connection failure.
                let failure = job.failure.lock().unwrap().clone();
                // Overflow/recorder failures also cancel the worker to stop intake.
                // Only a cancellation without a fault suppresses completed text.
                let user_cancelled = job.cancelled.is_cancelled() && failure.is_none();
                let error = failure.unwrap_or_else(|| {
                    other
                        .err()
                        .map_or_else(|| "dictation cancelled".into(), |e| e.to_string())
                });
                tracing::warn!(error=%error, "Realtime dictation failed");
                *job.failure.lock().unwrap() = Some(error.clone());
                Err(failed(
                    error,
                    if user_cancelled {
                        String::new()
                    } else {
                        recording.text(language.clone())
                    },
                ))
            }
        };
        let _ = job.result.send(result);
        if let Some((prepared, mut receiver)) = pending {
            // Keep answering control traffic while the user edits, without accepting
            // another recording until this result has been confirmed or discarded.
            let feedback = tokio::select! {
                biased;
                feedback = async {
                    loop {
                        if let Some(socket) = &mut connection {
                            tokio::select! {
                                feedback = &mut receiver => break feedback,
                                _ = socket.idle() => { connection = None; }
                            }
                        } else {
                            break (&mut receiver).await;
                        }
                    }
                } => feedback.ok(),
                _ = shutdown.cancelled() => {
                    // Close first: a sender racing shutdown either fails or leaves
                    // a value that must be persisted before this thread is joined.
                    receiver.close();
                    receiver.try_recv().ok()
                }
            };
            if let Some(feedback) = feedback {
                memory.confirm(&recording.id, &prepared, &feedback, now());
                if let Err(error) = memory.save() {
                    tracing::warn!(%error, "Could not save confirmed dictation memory");
                }
            }
        }
    }
}

pub(super) struct Recording {
    id: String,
    resampler: Option<PcmResampler>,
    segment: Vec<i16>,
    results: Vec<super::result::DictationResult>,
    index: usize,
    evaluation: Option<(u64, Vec<super::evaluation::SegmentTrace>)>,
}

impl Recording {
    pub fn new() -> Self {
        Self {
            id: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            resampler: None,
            segment: Vec::new(),
            results: Vec::new(),
            index: 0,
            evaluation: None,
        }
    }

    pub(super) fn for_evaluation(id: String, at: u64) -> Self {
        let mut recording = Self::new();
        recording.id = id;
        recording.evaluation = Some((at, Vec::new()));
        recording
    }

    pub(super) fn traces(&self) -> Vec<super::evaluation::SegmentTrace> {
        self.evaluation
            .as_ref()
            .map(|(_, traces)| traces.clone())
            .unwrap_or_default()
    }

    pub fn text(&self, language: Option<String>) -> String {
        crate::text::merge_texts(
            &self
                .results
                .iter()
                .map(|r| r.text.clone())
                .collect::<Vec<_>>(),
            language,
        )
    }
    pub async fn push(
        &mut self,
        connection: &mut Connection,
        memory: &mut Memory,
        frame: &WavChunk,
    ) -> Result<()> {
        let (rate, samples) = crate::audio::decode_mono(frame)?;
        if self.resampler.is_none() {
            self.resampler = Some(PcmResampler::new(rate)?);
        }
        let filter = self.resampler.as_mut().unwrap();
        ensure!(filter.rate() == rate, "audio rate changed during recording");
        let pcm = filter.push(&samples, false)?;
        self.push_pcm(connection, memory, &pcm).await
    }

    pub async fn finish(&mut self, connection: &mut Connection, memory: &mut Memory) -> Result<()> {
        if let Some(mut filter) = self.resampler.take() {
            let tail = filter.push(&[], true)?;
            self.push_pcm(connection, memory, &tail).await?;
        }
        if !self.segment.is_empty() {
            self.finish_segment(connection, memory).await?;
        }
        Ok(())
    }

    async fn push_pcm(
        &mut self,
        connection: &mut Connection,
        memory: &mut Memory,
        mut pcm: &[i16],
    ) -> Result<()> {
        while !pcm.is_empty() {
            let count = (SEGMENT_SAMPLES - self.segment.len()).min(pcm.len());
            connection.append(&pcm[..count]).await?;
            self.segment.extend_from_slice(&pcm[..count]);
            pcm = &pcm[count..];
            if self.segment.len() == SEGMENT_SAMPLES {
                self.finish_segment(connection, memory).await?;
                self.segment.clear();
                self.index += 1;
            }
        }
        Ok(())
    }

    async fn finish_segment(
        &mut self,
        connection: &mut Connection,
        memory: &mut Memory,
    ) -> Result<()> {
        // Silence is uploaded to preserve the stream timeline, but cannot invent a final transcript.
        let wav = crate::audio::chunk::encode_i16_wav(&self.segment, 24_000)?;
        let speech = crate::audio::contains_speech_for_dictation(&wav).unwrap_or_else(|error| {
            tracing::warn!(%error, "Speech classification failed; preserving dictation audio");
            true
        });
        if !speech {
            return connection.clear().await;
        }
        // Preserve a short voiced tail rather than discarding it; padding is silence, not source audio.
        if self.segment.len() < 2400 {
            connection
                .append(&vec![0; 2400 - self.segment.len()])
                .await?;
        }
        let at = self
            .evaluation
            .as_ref()
            .map(|(at, _)| *at)
            .unwrap_or_else(now);
        let snapshot = memory.snapshot(at);
        let result = connection
            .finish(&snapshot, &format!("{}-{}", self.id, self.index))
            .await?;
        // Log lengths without exposing dictation contents.
        tracing::debug!(
            final_chars = result.text.chars().count(),
            "Dictation completed"
        );
        if let Some((_, traces)) = &mut self.evaluation {
            traces.push(super::evaluation::SegmentTrace {
                index: self.index,
                input: snapshot,
                output: result.raw.clone(),
            });
        }
        self.results.push(result);
        // Cleanup is fallible even after the model has completed and passed validation.
        // Keep the text for partial delivery, but learn only once cleanup succeeds.
        connection.delete_audio().await?;
        memory.learn(&self.id, self.index, self.results.last().unwrap(), at);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
