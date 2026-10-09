use anyhow::{Result, ensure};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc as async_mpsc;
use tokio_util::sync::CancellationToken;

use super::{config::DictationConfig, memory::Memory, transport::Connection};
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
    result: mpsc::Receiver<Result<String, SessionError>>,
    cancelled: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
}

struct Job {
    frames: async_mpsc::Receiver<WavChunk>,
    result: mpsc::SyncSender<Result<String, SessionError>>,
    cancelled: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
}

/// Keeps the Realtime connection and memory alive across recording sessions.
pub(crate) struct DictationWorker {
    jobs: async_mpsc::Sender<Job>,
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
        std::thread::Builder::new()
            .name("realtime-dictation".into())
            .spawn(move || runtime.block_on(worker(settings, key, memory, receiver, language)))?;
        Ok(Self { jobs })
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

    pub fn finish(self) -> Result<String, SessionError> {
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
) {
    let mut connection: Option<Connection> = None;
    loop {
        let next = if let Some(socket) = &mut connection {
            tokio::select! {
                job = jobs.recv() => job,
                _ = socket.idle() => { connection = None; continue; }
            }
        } else {
            jobs.recv().await
        };
        let Some(mut job) = next else {
            break;
        };
        let mut working_memory = memory.clone();
        let mut recording = Recording::new();
        let result = tokio::select! {
            biased;
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
                memory = working_memory;
                if let Err(e) = memory.save() {
                    tracing::warn!(error=%e, "Could not save dictation memory");
                }
                Ok(recording.text(language.clone()))
            }
            other => {
                if job.cancelled.is_cancelled()
                    && let Some(socket) = &mut connection
                {
                    // Cancellation is sent using the standard event before retiring an interrupted connection.
                    if let Err(error) = socket.cancel().await {
                        tracing::debug!(%error, "Retiring Realtime connection after cancellation cleanup failed");
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
    }
}

pub(super) struct Recording {
    id: String,
    resampler: Option<PcmResampler>,
    segment: Vec<i16>,
    results: Vec<super::result::DictationResult>,
    index: usize,
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
        }
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
        let snapshot = memory.snapshot(now());
        let result = connection
            .finish(&snapshot, &format!("{}-{}", self.id, self.index))
            .await?;
        // Log lengths without exposing dictation contents.
        tracing::debug!(
            final_chars = result.text.chars().count(),
            "Dictation completed"
        );
        self.results.push(result);
        // Cleanup is fallible even after the model has completed and passed validation.
        // Keep the text for partial delivery, but learn only once cleanup succeeds.
        connection.delete_audio().await?;
        memory.learn(&self.id, self.index, self.results.last().unwrap(), now());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
