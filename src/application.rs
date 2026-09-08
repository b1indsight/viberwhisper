mod listener;
mod prompt_lab;
mod setup;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::core::cli::{Cli, Commands, ConfigAction};
use crate::core::config::{ConfigDocument, ConfigStore, fields};
use crate::{audio, postprocess, text, transcriber};

/// Settings consumed by offline transcription and final text cleanup.
#[derive(Debug)]
struct ConvertConfig {
    transcriber: transcriber::TranscriberConfig,
    post_process: postprocess::PostProcessConfig,
    language: Option<String>,
}

impl ConvertConfig {
    fn from_config(document: &ConfigDocument) -> Result<Self> {
        Ok(Self {
            transcriber: transcriber::TranscriberConfig::from_config(document)?,
            post_process: postprocess::PostProcessConfig::from_config(document)?,
            language: document.select(fields::TranscriptionLanguage, std::convert::identity),
        })
    }
}

/// Initializes process-wide services, parses the CLI, and runs the selected workflow.
pub fn run() -> Result<()> {
    if setup::run_hotkey_capture_helper_if_requested()? {
        return Ok(());
    }
    init_tracing();

    let cli = Cli::parse();

    match cli.command {
        None => {
            run_listener_with_setup()?;
        }
        Some(Commands::Setup) => {
            setup::run_explicit()?;
        }
        Some(Commands::Config { action }) => {
            handle_config(action)?;
        }
        Some(Commands::Convert { input, output }) => {
            handle_convert(&input, output.as_deref())?;
        }
        Some(Commands::PromptLab { action }) => {
            prompt_lab::handle(action)?;
        }
    }

    Ok(())
}

/// Starts the desktop listener directly and presents fatal startup errors without a console.
pub fn run_desktop() -> ExitCode {
    match setup::run_hotkey_capture_helper_if_requested() {
        Ok(true) => return ExitCode::SUCCESS,
        Ok(false) => {}
        Err(_) => return ExitCode::FAILURE,
    }
    if let Err(error) = crate::platform::prepare_desktop_output() {
        let error = anyhow::Error::new(error).context("failed to prepare desktop output");
        crate::platform::report_desktop_startup_error(&error);
        return ExitCode::FAILURE;
    }

    init_tracing();

    match run_listener_with_setup() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            crate::platform::report_desktop_startup_error(&error);
            ExitCode::FAILURE
        }
    }
}

fn run_listener_with_setup() -> Result<()> {
    if let Some(config) = setup::listener_config()? {
        listener::run_with_config(config)?;
    }
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")),
        )
        .with_target(false)
        .init();
}

fn load_config() -> Result<(ConfigStore, ConfigDocument)> {
    let store = ConfigStore::discover()?;
    let document = store.load()?.unwrap_or_default();
    Ok((store, document))
}

fn handle_config(action: ConfigAction) -> Result<()> {
    let store = ConfigStore::discover()?;
    if matches!(action, ConfigAction::Path) {
        println!("{}", store.path().display());
        return Ok(());
    }

    let mut document = store.load()?.unwrap_or_default();
    match action {
        ConfigAction::Path => unreachable!(),
        ConfigAction::Check => {
            listener::ListenerConfig::from_config(&document)?;
            println!("Configuration loaded successfully.");
        }
        ConfigAction::List => {
            println!("{:<48} Value", "Key");
            println!("{}", "-".repeat(80));
            for key in ConfigDocument::field_keys() {
                let value = document.get_field(key.as_str())?;
                println!("{:<48} {}", key.as_str(), value);
            }
        }
        ConfigAction::Get { key } => {
            println!("{}", document.get_field(&key)?);
        }
        ConfigAction::Set { key, value } => {
            let mut candidate = document.clone();
            candidate.set_field(&key, &value)?;
            store.save(&candidate)?;
            document = candidate;
            let displayed = document.get_field(&key)?;
            println!("Set {key} = {displayed}");
        }
    }
    Ok(())
}

fn handle_convert(input: &str, output: Option<&str>) -> Result<()> {
    use postprocess::PostProcessor;
    use std::path::Path;
    use transcriber::{ApiTranscriber, Transcriber};

    info!(input, "Transcribing audio file");

    let (_, document) = load_config()?;
    let config = ConvertConfig::from_config(&document)?;
    let transcriber = ApiTranscriber::new(config.transcriber)?;
    let post_processor = PostProcessor::new(config.post_process);

    let chunk_reader = audio::WavChunkReader::open(
        Path::new(input),
        audio::MAX_CHUNK_DURATION_SECS,
        audio::MAX_CHUNK_SIZE_BYTES,
    )?;
    let mut chunk_texts = Vec::new();
    for chunk in chunk_reader {
        chunk_texts.push(transcriber.transcribe(&chunk?)?);
    }
    let stt_text = text::merge_texts(&chunk_texts, config.language.clone());
    let text = if stt_text.is_empty() {
        stt_text
    } else {
        match post_processor.process_text(&stt_text) {
            Ok(processed) if !processed.is_empty() => processed,
            Ok(_) => {
                // Empty post-process output is not useful; keep the STT text.
                warn!("Post-processing returned empty text, using original STT text");
                stt_text
            }
            Err(e) => {
                // Runtime LLM errors should not discard a successful STT result.
                warn!(error = %e, "Post-processing failed, using original STT text");
                stt_text
            }
        }
    };
    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &text) {
                error!(path, error = %e, "Failed to write transcription output");
                return Err(e.into());
            }
            println!("Saved to: {}", path);
        }
        None => println!("{}", text),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::SecretSource;

    // Keep this fixture local; application/listener.rs uses the same no-secret test setup.
    struct EmptySecrets;

    impl SecretSource for EmptySecrets {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn api_configuration_defers_model_and_protocol_checks_to_requests() {
        let mut document = ConfigDocument::new(EmptySecrets);
        document.inference.api.transcription.api_url = "file:///transcriptions".to_string();
        document.inference.api.transcription.model.clear();
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url = Some("file:///completions".to_string());
        document.inference.api.post_process.model = Some(String::new());
        // Configuration constructs typed settings; HTTP transport and API model errors
        // belong to the request that uses them, including when cleanup is enabled.
        let config = ConvertConfig::from_config(&document).unwrap();
        assert!(config.transcriber.metadata().model.is_empty());
        assert!(matches!(
            config.post_process,
            postprocess::PostProcessConfig::Llm(_)
        ));
    }

    #[test]
    fn conversion_settings_ignore_hotkeys_and_allow_unauthenticated_endpoints() {
        let mut document = ConfigDocument::new(EmptySecrets);
        document.input.hold_hotkey = "not a hotkey".to_string();
        document.inference.api.transcription.api_url =
            "http://127.0.0.1:8080/v1/audio/transcriptions".to_string();
        document.transcription.language = Some("zh".to_string());
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url =
            Some("http://127.0.0.1:8080/v1/chat/completions".to_string());
        document.inference.api.post_process.model = Some("local-model".to_string());
        // WAV conversion uses STT and cleanup, without requiring a valid desktop hotkey binding.
        let config = ConvertConfig::from_config(&document).unwrap();
        assert_eq!(config.language.as_deref(), Some("zh"));
        assert_eq!(
            config.transcriber.metadata().language.as_deref(),
            Some("zh")
        );
    }
}
