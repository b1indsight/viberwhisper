mod listener;
mod prompt_lab;
mod setup;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::core::cli::{Cli, Commands, ConfigAction};
use crate::core::config::{self, ConfigDocument, ConfigStore, EnvironmentSecretSource};
use crate::{audio, postprocess, text, transcriber};

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
    let secrets = EnvironmentSecretSource;
    match action {
        ConfigAction::Path => unreachable!(),
        ConfigAction::Check => {
            config::check(&document, &secrets)?;
            println!("Configuration is valid.");
        }
        ConfigAction::List => {
            println!("{:<48} Value", "Key");
            println!("{}", "-".repeat(80));
            for key in ConfigDocument::field_keys() {
                let value = document.get_field(key.as_str(), &secrets)?;
                println!("{:<48} {}", key.as_str(), value);
            }
        }
        ConfigAction::Get { key } => {
            println!("{}", document.get_field(&key, &secrets)?);
        }
        ConfigAction::Set { key, value } => {
            let mut candidate = document.clone();
            candidate.set_field(&key, &value)?;
            store.save(&candidate)?;
            document = candidate;
            let displayed = document.get_field(&key, &secrets)?;
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
    let config = config::resolve_convert(&document, &EnvironmentSecretSource)?;
    let transcriber = ApiTranscriber::new(config.backend.transcriber)?;
    let post_processor = PostProcessor::new(config.backend.post_process);

    let mut chunk_reader = audio::WavChunkReader::open(
        Path::new(input),
        config.max_chunk_duration_secs,
        config.max_chunk_size_bytes,
    )?;
    let mut chunk_texts = Vec::new();
    for chunk in chunk_reader.chunks() {
        chunk_texts.push(transcriber.transcribe(&chunk?)?);
    }
    let stt_text = text::merge_texts(&chunk_texts, config.language.as_deref());
    let text = if stt_text.is_empty() {
        stt_text
    } else {
        match post_processor.process(&stt_text) {
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
