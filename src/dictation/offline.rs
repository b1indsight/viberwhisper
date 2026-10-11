//! Fixed-context evaluation and WAV conversion; never reads or writes live memory.
use super::{config::DictationConfig, engine::Recording, memory::Memory, transport::Connection};
use crate::{
    audio::WavChunk,
    core::config::SecretValue,
    transcriber::{TranscribeError, Transcriber, TranscriberMetadata},
};
use anyhow::Result;
use std::{path::Path, sync::Mutex};

pub(crate) struct OfflineDictation {
    settings: DictationConfig,
    key: Option<SecretValue>,
    runtime: tokio::runtime::Runtime,
    language: Option<String>,
    connection: Mutex<Option<Connection>>,
}

impl OfflineDictation {
    pub fn new(
        settings: DictationConfig,
        key: Option<SecretValue>,
        language: Option<String>,
    ) -> Result<Self> {
        settings.validate()?;
        Ok(Self {
            settings,
            key,
            language,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            connection: Mutex::new(None),
        })
    }

    pub fn metadata(&self) -> TranscriberMetadata {
        TranscriberMetadata {
            endpoint: self.settings.url.clone(),
            model: self.settings.model.clone(),
            language: self.language.clone(),
            prompt: Some(self.settings.prompt()),
            temperature: 0.0,
        }
    }

    /// Conversion keeps resampling continuous across file chunks and reports any incomplete segment.
    pub fn convert(&self, path: &Path) -> Result<String> {
        self.transcribe_chunks(
            crate::audio::WavChunkReader::open(path)?.map(|chunk| chunk.map_err(Into::into)),
        )
    }

    fn transcribe_chunks(
        &self,
        chunks: impl IntoIterator<Item = Result<WavChunk>>,
    ) -> Result<String> {
        let mut connection = self.connection.lock().unwrap();
        let result = self.runtime.block_on(async {
            if connection.is_none() {
                *connection = Some(
                    Connection::connect(&self.settings, self.key.as_ref().map(SecretValue::expose))
                        .await?,
                );
            }
            let connection = connection.as_mut().unwrap();
            let mut memory = Memory::frozen(self.settings.memory.clone());
            let mut recording = Recording::new();
            for chunk in chunks {
                recording.push(connection, &mut memory, &chunk?).await?;
            }
            recording.finish(connection, &mut memory).await?;
            Ok::<_, anyhow::Error>(recording.text(self.language.clone()))
        });
        if result.is_err() {
            *connection = None;
        }
        result
    }
}

impl Transcriber for OfflineDictation {
    fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError> {
        self.transcribe_chunks(std::iter::once(Ok(chunk.clone())))
            .map_err(|error| TranscribeError::Network(error.to_string()))
    }
}
