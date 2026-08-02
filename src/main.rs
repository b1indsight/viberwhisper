mod audio;
mod core;
mod input;
mod local;
mod platform;
mod postprocess;
mod runtime_config;
mod text;
mod transcriber;

use clap::Parser;
use core::cli::{Cli, Commands, ConfigAction, LocalCommand};
use core::config::{ConfigDocument, ConfigStore, EnvironmentSecretSource};
use local::{
    LocalPaths, LocalServiceManager, PythonRuntime, dependencies_installed, detect_python_runtime,
    download_model, install_requirements, model_weights_present, setup_venv, verify_install,
};
use runtime_config::{BackendConfig, ListenerConfig, ProfileSelection};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            run_listener()?;
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
    }

    Ok(())
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
            run_listener_with_config(config)
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

    print_python_runtime(&runtime);

    println!("[local] step 1/4 – python venv");
    setup_venv(&paths.venv_dir)?;

    if install_deps {
        let requirements = local_requirements_path();
        println!("[local] step 2/4 – python dependencies");
        install_requirements(&paths.venv_dir, &requirements)?;
    } else if !dependencies_installed(&paths.venv_dir) {
        return Err(
            "Python dependencies are not installed. Run `viberwhisper local install` first.".into(),
        );
    } else {
        println!("[local] step 2/4 – python dependencies (skipped)");
    }

    if !model_weights_present(&paths.model_dir) {
        println!("[local] step 3/4 – downloading google/gemma-4-E2B-it");
        println!("[local]   set HF_ENDPOINT env var to use a mirror");
    } else {
        println!("[local] step 3/4 – model already present, skipping download");
    }
    download_model(&paths.model_dir, &hf_endpoint)?;

    println!("[local] step 4/4 – verify");
    verify_install(&paths.venv_dir, &paths.model_dir)?;

    Ok(())
}

fn print_python_runtime(runtime: &PythonRuntime) {
    let (major, minor) = runtime.version;
    println!(
        "[local] python: {} ({}.{}; require >= 3.10)",
        runtime.python.display(),
        major,
        minor
    );
    match &runtime.uv {
        Some(uv) => println!("[local] package runner: uv ({})", uv.display()),
        None => println!("[local] package runner: system python fallback"),
    }
}

fn load_config() -> Result<(ConfigStore, ConfigDocument), Box<dyn std::error::Error>> {
    let store = ConfigStore::discover()?;
    let document = store.load()?;
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

fn run_listener() -> Result<(), Box<dyn std::error::Error>> {
    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    let config = runtime_config::resolve_listener(
        &document,
        &EnvironmentSecretSource,
        ProfileSelection::Configured,
        &config_dir,
        &home_dir,
    )?;
    run_listener_with_config(config)
}

fn run_listener_with_config(mut config: ListenerConfig) -> Result<(), Box<dyn std::error::Error>> {
    use audio::AudioRecorder;
    use core::orchestrator::SessionOrchestrator;
    use core::recording_session::{
        ControlAction, ControlEvent, ControlSource, RecordingSessionMachine, SessionEvent,
        SessionMode,
    };
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::tray::{TrayAction, TrayManager};
    use postprocess::PostProcessor;
    use std::sync::Arc;
    use transcriber::{ApiTranscriber, Transcriber};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let local_manager = start_local_backend(&mut config.backend)?;
    let _local_manager = LocalServiceGuard::new(local_manager);
    let hotkey_manager = HotkeyManager::new(&config.hotkeys);

    let mut recorder = AudioRecorder::with_config(&config.audio);

    // Build transcriber and wrap in Arc<dyn Transcriber> for orchestrator injection.
    let transcriber: Arc<dyn Transcriber> =
        Arc::new(ApiTranscriber::new(config.backend.transcriber)?);

    let post_processor = PostProcessor::new(config.backend.post_process);

    let orchestrator = SessionOrchestrator::new(Arc::clone(&transcriber), config.orchestrator);

    #[cfg(target_os = "macos")]
    let typer = platform::macos::MacTyper;
    #[cfg(target_os = "windows")]
    let typer = platform::windows::WindowsTyper;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let typer = input::typer::MockTyper;

    let mut tray = TrayManager::new()?;
    info!("System tray icon started");
    let mut session_machine = RecordingSessionMachine::new();

    if let Some(hotkey) = config.hotkeys.hold_label.as_deref() {
        println!("Hold {hotkey} to record, release to transcribe.");
    }
    if let Some(hotkey) = config.hotkeys.toggle_label.as_deref() {
        println!("Press {hotkey} to start recording, press again to stop.");
    }
    println!("Press Ctrl+C to exit.");
    println!();

    let mut counter = 0;
    loop {
        tray.update();

        if let Some(action) = tray.check_action() {
            let event = match action {
                TrayAction::Exit => SessionEvent::ShutdownRequested,
                TrayAction::ToggleRecording => SessionEvent::Control(ControlEvent {
                    source: ControlSource::Tray,
                    action: ControlAction::Toggle(SessionMode::Toggle),
                }),
            };
            if drive_session(
                &mut session_machine,
                event,
                &mut recorder,
                &orchestrator,
                &mut tray,
                &post_processor,
                &typer,
            ) {
                break Ok(());
            }
        }

        if let Some(event) = hotkey_manager.check_event() {
            let control = match event {
                HotkeyEvent::Pressed(HotkeySource::Hold) => Some(ControlEvent {
                    source: ControlSource::HoldHotkey,
                    action: ControlAction::Start(SessionMode::Hold),
                }),
                HotkeyEvent::Released(HotkeySource::Hold) => Some(ControlEvent {
                    source: ControlSource::HoldHotkey,
                    action: ControlAction::Stop,
                }),
                HotkeyEvent::Pressed(HotkeySource::Toggle) => Some(ControlEvent {
                    source: ControlSource::ToggleHotkey,
                    action: ControlAction::Toggle(SessionMode::Toggle),
                }),
                HotkeyEvent::Released(HotkeySource::Toggle) => None,
            };
            if let Some(control) = control {
                let _ = drive_session(
                    &mut session_machine,
                    SessionEvent::Control(control),
                    &mut recorder,
                    &orchestrator,
                    &mut tray,
                    &post_processor,
                    &typer,
                );
            }
        }

        if let Some(chunk) = recorder.take_ready_chunk() {
            let _ = drive_session(
                &mut session_machine,
                SessionEvent::ChunkReady {
                    session_id: chunk.session_id,
                    chunk: chunk.chunk,
                },
                &mut recorder,
                &orchestrator,
                &mut tray,
                &post_processor,
                &typer,
            );
        }

        counter += 1;
        if counter % 300 == 0 {
            let status = format!("{:?}", session_machine.state());
            debug!(
                status = %status,
                hold_hotkey = %config.hotkeys.hold_label.as_deref().unwrap_or("disabled"),
                toggle_hotkey = %config.hotkeys.toggle_label.as_deref().unwrap_or("disabled"),
                "Heartbeat"
            );
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn drive_session(
    machine: &mut core::recording_session::RecordingSessionMachine,
    initial_event: core::recording_session::SessionEvent,
    recorder: &mut audio::AudioRecorder,
    orchestrator: &core::orchestrator::SessionOrchestrator,
    tray: &mut input::tray::TrayManager,
    post_processor: &postprocess::PostProcessor,
    typer: &dyn input::typer::TextTyper,
) -> bool {
    use audio::{RecorderStartOutcome, RecorderStopOutcome};
    use core::recording_session::{SessionEffect, SessionEvent};
    use std::collections::VecDeque;

    let mut events = VecDeque::from([initial_event]);
    let mut ready_to_exit = false;
    while let Some(event) = events.pop_front() {
        for effect in machine.handle(event) {
            match effect {
                SessionEffect::StartRecorder { session_id } => {
                    let event = match recorder.start_recording(session_id) {
                        RecorderStartOutcome::Started { session_id } => {
                            SessionEvent::RecorderStarted { session_id }
                        }
                        RecorderStartOutcome::AlreadyRecording {
                            requested_session_id,
                            active_session_id,
                        } => SessionEvent::RecorderAlreadyRecording {
                            requested_session_id,
                            active_session_id,
                        },
                        RecorderStartOutcome::Failed { session_id, error } => {
                            error!(
                                session_id = session_id.0,
                                error, "Failed to start recording"
                            );
                            SessionEvent::RecorderStartFailed { session_id, error }
                        }
                    };
                    events.push_back(event);
                }
                SessionEffect::StartOrchestrator { session_id, mode } => {
                    match orchestrator.start_session(session_id, mode) {
                        Ok(()) => {
                            events.push_back(SessionEvent::OrchestratorStarted { session_id })
                        }
                        Err(error) => {
                            let active_session_id = match &error {
                                core::orchestrator::SessionStartError::ActiveSession {
                                    active,
                                    ..
                                } => Some(*active),
                            };
                            error!(session_id = session_id.0, error = %error, "Failed to start session orchestrator");
                            events.push_back(SessionEvent::OrchestratorStartFailed {
                                requested_session_id: session_id,
                                active_session_id,
                                error: error.to_string(),
                            });
                        }
                    }
                }
                SessionEffect::StopRecorder { session_id } => {
                    let event = match recorder.stop_recording(session_id) {
                        RecorderStopOutcome::Stopped {
                            session_id,
                            chunks,
                            warning,
                        } => {
                            if let Some(warning) = warning.as_deref() {
                                warn!(
                                    session_id = session_id.0,
                                    warning, "Recorder stopped with a warning"
                                );
                            }
                            SessionEvent::RecorderStopped {
                                session_id,
                                chunks,
                                warning,
                            }
                        }
                        RecorderStopOutcome::StillRecording { session_id, error } => {
                            error!(session_id = session_id.0, error, "Failed to stop recorder");
                            SessionEvent::RecorderStillRecording { session_id, error }
                        }
                        RecorderStopOutcome::NotRecording {
                            requested_session_id,
                        } => SessionEvent::RecorderNotRecording {
                            session_id: requested_session_id,
                        },
                    };
                    events.push_back(event);
                }
                SessionEffect::SubmitChunk { session_id, chunk } => {
                    if let Err(error) = orchestrator.on_chunk_ready(session_id, chunk) {
                        warn!(session_id = session_id.0, error = %error, "Chunk was rejected");
                    }
                }
                SessionEffect::FinishOrchestrator { session_id } => {
                    finish_transcription(
                        orchestrator.finish_session(session_id),
                        post_processor,
                        typer,
                    );
                    events.push_back(SessionEvent::OrchestratorFinished { session_id });
                }
                SessionEffect::CancelRecorder { session_id } => {
                    let outcome = recorder.cancel_recording(session_id);
                    debug!(
                        session_id = session_id.0,
                        ?outcome,
                        "Recorder cancellation handled"
                    );
                }
                SessionEffect::AbortOrchestrator { session_id } => {
                    if let Err(error) = orchestrator.abort_session(session_id) {
                        debug!(session_id = session_id.0, error = %error, "No matching orchestrator session to abort");
                    }
                }
                SessionEffect::SetTrayRecording(recording) => tray.set_recording(recording),
                SessionEffect::ReadyToExit => ready_to_exit = true,
            }
        }
    }
    ready_to_exit
}

fn finish_transcription(
    result: Result<String, core::orchestrator::SessionError>,
    post_processor: &postprocess::PostProcessor,
    typer: &dyn input::typer::TextTyper,
) {
    use core::orchestrator::SessionError;

    match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return;
            }
            let text = {
                let mut session = post_processor.start_session();
                session.push_stable_chunk(&stt_text);
                match session.finish() {
                    Ok(processed) if !processed.is_empty() => processed,
                    Ok(_) => {
                        warn!("Post-processing returned empty text, using original STT text");
                        stt_text
                    }
                    Err(error) => {
                        warn!(error = %error, "Post-processing failed, using original STT text");
                        stt_text
                    }
                }
            };
            info!(text = %text, "Typing transcribed text");
            if let Err(error) = typer.type_text(&text) {
                error!(error = %error, "Failed to type text");
            }
        }
        Err(SessionError::NoChunks) => warn!("No audio chunks to transcribe"),
        Err(SessionError::Routing(error)) => {
            error!(error = %error, "Session routing failed while finalizing")
        }
        Err(SessionError::PartialFailure {
            errors,
            partial_text,
        }) => {
            error!(
                failed_chunks = errors.len(),
                "Partial transcription failure"
            );
            if !partial_text.is_empty()
                && let Err(error) = typer.type_text(&partial_text)
            {
                error!(error = %error, "Failed to type partial text");
            }
        }
        Err(SessionError::ConvergenceTimeout {
            pending_count,
            partial_text,
        }) => {
            warn!(pending_count, "Convergence timeout");
            if !partial_text.is_empty()
                && let Err(error) = typer.type_text(&partial_text)
            {
                error!(error = %error, "Failed to type partial text");
            }
        }
    }
}

fn handle_config(action: ConfigAction) -> Result<(), Box<dyn std::error::Error>> {
    let store = ConfigStore::discover()?;
    if matches!(action, ConfigAction::Path) {
        println!("{}", store.path().display());
        return Ok(());
    }

    let mut document = store.load()?;
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

    println!("Transcribing: {}", input);

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
    let text = match post_processor.process(&stt_text) {
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
    };
    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &text) {
                eprintln!("Failed to write file: {}", e);
                return Err(e.into());
            }
            println!("Saved to: {}", path);
        }
        None => println!("{}", text),
    }
    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use transcriber::{MockTranscriber, Transcriber};

    #[test]
    fn test_full_pipeline_mock() {
        use input::typer::{MockTyper, TextTyper};
        let transcriber = MockTranscriber;
        let typer = MockTyper;
        let chunk = audio::WavChunk::from_encoded_bytes(b"fake wav".to_vec());
        let text = transcriber.transcribe(&chunk).unwrap();
        assert!(typer.type_text(&text).is_ok());
    }

    #[test]
    fn test_orchestrator_integration_single_chunk() {
        use self::core::orchestrator::{OrchestratorConfig, SessionMode, SessionOrchestrator};
        use std::sync::Arc;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(
            t,
            OrchestratorConfig::validate(
                &crate::core::config::SessionSection::default(),
                Some("en".to_string()),
            )
            .unwrap(),
        );

        orch.start_session(
            crate::core::recording_session::SessionId(1),
            SessionMode::Hold,
        )
        .unwrap();
        let chunk = audio::WavChunk::from_encoded_bytes(b"fake wav".to_vec());
        let _ = orch.on_chunk_ready(crate::core::recording_session::SessionId(1), chunk);
        let result = orch.finish_session(crate::core::recording_session::SessionId(1));

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_orchestrator_no_chunks() {
        use self::core::orchestrator::{
            OrchestratorConfig, SessionError, SessionMode, SessionOrchestrator,
        };
        use std::sync::Arc;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(
            t,
            OrchestratorConfig::validate(&crate::core::config::SessionSection::default(), None)
                .unwrap(),
        );

        orch.start_session(
            crate::core::recording_session::SessionId(1),
            SessionMode::Toggle,
        )
        .unwrap();
        let result = orch.finish_session(crate::core::recording_session::SessionId(1));
        assert!(matches!(result, Err(SessionError::NoChunks)));
    }
}
