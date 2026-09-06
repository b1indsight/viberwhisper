mod listener;
mod prompt_lab;
mod setup;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::core::cli::{Cli, Commands, ConfigAction, LocalCommand};
use crate::core::config::{ConfigDocument, ConfigStore, EnvironmentSecretSource};
use crate::local::{
    LocalPaths, LocalServiceManager, PythonRuntime, dependencies_installed, detect_python_runtime,
    download_model, install_requirements, model_weights_present, setup_venv, verify_install,
};
use crate::runtime_config::{self, BackendConfig, ProfileSelection};
use crate::{audio, postprocess, text, transcriber};

struct LocalServiceGuard(Option<LocalServiceManager>);

impl LocalServiceGuard {
    fn new(manager: Option<LocalServiceManager>) -> Self {
        Self(manager)
    }
}

impl Drop for LocalServiceGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.0.as_mut() {
            manager.release();
        }
    }
}

/// Initializes process-wide services, parses the CLI, and runs the selected workflow.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
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
        Some(Commands::Local { action }) => {
            handle_local(action)?;
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
        crate::platform::report_desktop_startup_error(&error);
        return ExitCode::FAILURE;
    }

    init_tracing();

    match run_listener_with_setup() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            crate::platform::report_desktop_startup_error(error.as_ref());
            ExitCode::FAILURE
        }
    }
}

fn run_listener_with_setup() -> Result<(), Box<dyn std::error::Error>> {
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

fn handle_local(action: LocalCommand) -> Result<(), Box<dyn std::error::Error>> {
    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    match action {
        LocalCommand::Install => {
            let paths = runtime_config::resolve_local_paths(&document, &config_dir, &home_dir)?;
            ensure_local_install(&paths, true)?;
            println!("Local Gemma runtime is installed.");
            Ok(())
        }
        LocalCommand::Start => {
            let config = runtime_config::resolve_listener(
                &document,
                &EnvironmentSecretSource,
                ProfileSelection::Local,
                &config_dir,
                &home_dir,
            )?;
            listener::run_with_config(config)
        }
        LocalCommand::Stop => {
            let paths = runtime_config::resolve_local_paths(&document, &config_dir, &home_dir)?;
            let mut manager = LocalServiceManager::for_paths(paths);
            manager.stop();
            println!("Local Gemma service stopped.");
            Ok(())
        }
        LocalCommand::Status => {
            let config = runtime_config::resolve_local_service(&document, &config_dir, &home_dir)?;
            let manager = LocalServiceManager::from_config(config);
            let status = manager.status()?;
            println!("running: {}", status.running);
            println!("port: {}", status.port);
            println!(
                "pid: {}",
                status
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "n/a".to_string())
            );
            println!(
                "memory: {}",
                status
                    .memory_usage
                    .unwrap_or_else(|| "unavailable".to_string())
            );
            println!("health: {}", status.health);
            Ok(())
        }
    }
}

fn start_local_backend(
    backend: &mut BackendConfig,
) -> Result<Option<LocalServiceManager>, Box<dyn std::error::Error>> {
    let Some(config) = backend.local_service.take() else {
        return Ok(None);
    };
    ensure_local_install(&config.paths, false)?;
    let mut manager = LocalServiceManager::from_config(config);
    manager.start()?;
    Ok(Some(manager))
}

fn ensure_local_install(
    paths: &LocalPaths,
    install_deps: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let hf_endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let runtime = detect_python_runtime()?;

    log_python_runtime(&runtime);

    info!(
        step = 1,
        total_steps = 4,
        "Creating local Python virtual environment"
    );
    setup_venv(&paths.venv_dir)?;

    if install_deps {
        let requirements = local_requirements_path();
        info!(
            step = 2,
            total_steps = 4,
            "Installing local Python dependencies"
        );
        install_requirements(&paths.venv_dir, &requirements)?;
    } else if !dependencies_installed(&paths.venv_dir) {
        return Err(
            "Python dependencies are not installed. Run `viberwhisper local install` first.".into(),
        );
    } else {
        info!(
            step = 2,
            total_steps = 4,
            "Local Python dependencies already installed"
        );
    }

    if !model_weights_present(&paths.model_dir) {
        info!(
            step = 3,
            total_steps = 4,
            model = "google/gemma-4-E2B-it",
            "Downloading local model; set HF_ENDPOINT to use a mirror"
        );
    } else {
        info!(
            step = 3,
            total_steps = 4,
            model = "google/gemma-4-E2B-it",
            "Local model already present"
        );
    }
    download_model(&paths.model_dir, &hf_endpoint)?;

    info!(
        step = 4,
        total_steps = 4,
        "Verifying local runtime installation"
    );
    verify_install(&paths.venv_dir, &paths.model_dir)?;

    Ok(())
}

fn log_python_runtime(runtime: &PythonRuntime) {
    let (major, minor) = runtime.version;
    info!(
        python = %runtime.python.display(),
        version_major = major,
        version_minor = minor,
        minimum_version = "3.10",
        "Detected local Python runtime"
    );
    match &runtime.uv {
        Some(uv) => info!(runner = "uv", path = %uv.display(), "Selected local package runner"),
        None => info!(runner = "system-python", "Selected local package runner"),
    }
}

fn load_config() -> Result<(ConfigStore, ConfigDocument), Box<dyn std::error::Error>> {
    let store = ConfigStore::discover()?;
    let document = store.load()?.unwrap_or_default();
    Ok((store, document))
}

fn config_context(store: &ConfigStore) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let config_dir = store
        .path()
        .parent()
        .ok_or("configuration path has no parent directory")?
        .to_path_buf();
    let home_dir = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok((config_dir, home_dir))
}

fn local_requirements_path() -> PathBuf {
    find_server_file("requirements.txt")
}

/// Locates a file inside the `server/` directory, trying the packaged location
/// (next to the executable) first, then falling back to the development source tree.
fn find_server_file(filename: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let candidate = exe_dir.join("server").join(filename);
        if candidate.exists() {
            return candidate;
        }
    }

    // Fallback: compile-time source tree (works with `cargo run`).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("server")
        .join(filename)
}

fn handle_config(action: ConfigAction) -> Result<(), Box<dyn std::error::Error>> {
    let store = ConfigStore::discover()?;
    if matches!(action, ConfigAction::Path) {
        println!("{}", store.path().display());
        return Ok(());
    }

    let mut document = store.load()?.unwrap_or_default();
    let secrets = EnvironmentSecretSource;
    let (config_dir, home_dir) = config_context(&store)?;
    match action {
        ConfigAction::Path => unreachable!(),
        ConfigAction::Check => {
            runtime_config::check(&document, &secrets, &config_dir, &home_dir)?;
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

fn handle_convert(input: &str, output: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use postprocess::PostProcessor;
    use std::path::Path;
    use transcriber::{ApiTranscriber, Transcriber};

    info!(input, "Transcribing audio file");

    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    let mut config = runtime_config::resolve_convert(
        &document,
        &EnvironmentSecretSource,
        &config_dir,
        &home_dir,
    )?;
    let local_manager = start_local_backend(&mut config.backend)?;
    let _local_manager = LocalServiceGuard::new(local_manager);
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
