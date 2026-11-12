//! Process entry points, CLI workflows, configuration, and session logic.

pub mod cli;
pub mod config;
pub(crate) mod listener;
pub(crate) mod logging;
pub mod orchestrator;
mod prompt_lab;
pub(crate) mod recognition;
pub mod recording_session;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};

use crate::core::cli::{Cli, Commands, ConfigAction};
use crate::core::config::{ConfigDocument, ConfigStore};
use crate::ui::setup;

/// Initializes process-wide services, parses the CLI, and runs the selected workflow.
pub fn run() -> Result<()> {
    crate::platform::initialize_dpi_awareness()?;
    if setup::run_hotkey_capture_helper_if_requested()? {
        return Ok(());
    }
    logging::init()?;

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
    if let Err(error) = crate::platform::initialize_dpi_awareness() {
        crate::platform::report_desktop_startup_error(&error);
        return ExitCode::FAILURE;
    }
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

    if let Err(error) = logging::init() {
        crate::platform::report_desktop_startup_error(&error);
        return ExitCode::FAILURE;
    }

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
        crate::ui::listener::run_with_config(config)?;
    }
    Ok(())
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
    use std::path::Path;
    use tokio_util::sync::CancellationToken;

    info!(input, "Transcribing audio file");

    let (_, mut document) = load_config()?;
    // Conversion has empty initial state and never confirms learning. It still
    // uses production memory switches and correction/finalization algorithms.
    document.dictation.memory.long_term.clear();
    let pipeline = recognition::RecognitionPipeline::from_config(
        &document,
        None,
        crate::dictation::system_clock(),
        None,
    )?;
    let result = pipeline.recognize_wav(
        Path::new(input),
        crate::session::SessionId(1),
        &CancellationToken::new(),
    )?;
    let text = result
        .transcript
        .map(|transcript| transcript.text)
        .unwrap_or_default();
    pipeline.shutdown();
    if let Some(error) = &result.recognition_error
        && text.is_empty()
    {
        return Err(anyhow::anyhow!("{error}"));
    }
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
    if let Some(error) = result.recognition_error {
        return Err(anyhow::anyhow!("{error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::SecretSource;

    // Keep this fixture local; core/listener.rs uses the same no-secret test setup.
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
        let config = recognition::RecognitionConfig::from_config(&document, false).unwrap();
        assert!(config.metadata().unwrap().model.is_empty());
        assert!(matches!(
            crate::postprocess::PostProcessConfig::from_config(&document).unwrap(),
            crate::postprocess::PostProcessConfig::Llm(_)
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
        let config = recognition::RecognitionConfig::from_config(&document, false).unwrap();
        assert_eq!(config.metadata().unwrap().language.as_deref(), Some("zh"));
    }
}
