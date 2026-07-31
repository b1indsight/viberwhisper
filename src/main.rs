mod audio;
mod core;
mod input;
mod local;
mod platform;
mod postprocess;
mod text;
mod transcriber;

use clap::Parser;
use core::cli::{Cli, Commands, ConfigAction, LocalCommand};
use core::config::AppConfig;
use local::{
    LocalServiceManager, PythonRuntime, dependencies_installed, detect_python_runtime,
    download_model, install_requirements, model_weights_present, setup_venv, verify_install,
};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

const LOCAL_MODEL_NAME: &str = "gemma-4-E2B-it";

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
            handle_config(action);
        }
        Some(Commands::Local { action }) => {
            handle_local(action)?;
        }
        Some(Commands::Convert { input, output }) => {
            handle_convert(&input, output.as_deref());
        }
    }

    Ok(())
}

fn handle_local(action: LocalCommand) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = AppConfig::load();
    match action {
        LocalCommand::Install => {
            ensure_local_install(&config, true)?;
            println!("Local Gemma runtime is installed.");
            Ok(())
        }
        LocalCommand::Start => {
            config.local_mode = true;
            run_listener_with_config(config)
        }
        LocalCommand::Stop => {
            let paths = local_paths(&config)?;
            let mut manager =
                LocalServiceManager::new(config.local_server_port, paths.model_dir, paths.venv_dir);
            manager.stop();
            println!("Local Gemma service stopped.");
            Ok(())
        }
        LocalCommand::Status => {
            let paths = local_paths(&config)?;
            let manager =
                LocalServiceManager::new(config.local_server_port, paths.model_dir, paths.venv_dir);
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

struct LocalPaths {
    venv_dir: PathBuf,
    model_dir: PathBuf,
}

fn prepare_runtime_config(
    mut config: AppConfig,
) -> Result<(AppConfig, Option<LocalServiceManager>), Box<dyn std::error::Error>> {
    if !config.local_mode {
        return Ok((config, None));
    }

    let paths = ensure_local_install(&config, false)?;
    let mut manager = LocalServiceManager::with_quantization(
        config.local_server_port,
        paths.model_dir,
        paths.venv_dir,
        config.local_quantization.clone(),
    );
    manager.start()?;
    config = apply_local_endpoint_overrides(&config, &manager.base_url());

    Ok((config, Some(manager)))
}

fn ensure_local_install(
    config: &AppConfig,
    install_deps: bool,
) -> Result<LocalPaths, Box<dyn std::error::Error>> {
    let paths = local_paths(config)?;
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

    Ok(paths)
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

fn local_paths(config: &AppConfig) -> Result<LocalPaths, Box<dyn std::error::Error>> {
    let data_dir = match &config.local_data_dir {
        Some(path) if path.starts_with("~/") => {
            let home = dirs::home_dir().ok_or("could not determine home directory")?;
            home.join(&path[2..])
        }
        Some(path) => PathBuf::from(path),
        None => default_local_data_dir()?,
    };

    Ok(LocalPaths {
        venv_dir: data_dir.join("venv"),
        model_dir: data_dir.join("model"),
    })
}

fn default_local_data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok(home_dir.join(".viberwhisper"))
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
    run_listener_with_config(AppConfig::load())
}

fn run_listener_with_config(config: AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    use audio::AudioRecorder;
    use core::orchestrator::SessionOrchestrator;
    use core::recording_session::{
        ControlAction, ControlEvent, ControlSource, RecordingSessionMachine, SessionEvent,
        SessionMode,
    };
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::tray::{TrayAction, TrayManager};
    use postprocess::create_post_processor;
    use std::sync::Arc;
    use std::time::Duration;
    use transcriber::{Transcriber, create_transcriber};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let (config, local_manager) = prepare_runtime_config(config)?;
    let _local_manager = LocalServiceGuard::new(local_manager);
    info!(
        hold_hotkey = %config.hold_hotkey,
        toggle_hotkey = %config.toggle_hotkey,
        model = %config.model,
        language = %config.language.as_deref().unwrap_or("auto"),
        api_url = %config.transcription_api_url,
        max_chunk_duration_secs = config.max_chunk_duration_secs,
        max_chunk_size_bytes = config.max_chunk_size_bytes,
        max_retries = config.max_retries,
        convergence_timeout_secs = config.convergence_timeout_secs,
        post_process_enabled = config.post_process_enabled,
        "Config loaded"
    );

    let hotkey_manager = HotkeyManager::new(&config.hold_hotkey, &config.toggle_hotkey)?;

    let mut recorder = AudioRecorder::with_config(
        config.mic_gain,
        config.max_chunk_duration_secs,
        config.max_chunk_size_bytes,
    )?;

    // Build transcriber and wrap in Arc<dyn Transcriber> for orchestrator injection.
    let transcriber: Arc<dyn Transcriber> = Arc::from(create_transcriber(&config)?);

    let post_processor = create_post_processor(&config);

    let orchestrator = SessionOrchestrator::new(
        Arc::clone(&transcriber),
        config.language.clone(),
        Duration::from_secs(config.convergence_timeout_secs),
    );

    #[cfg(target_os = "macos")]
    let typer = platform::macos::MacTyper;
    #[cfg(target_os = "windows")]
    let typer = platform::windows::WindowsTyper;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let typer = input::typer::MockTyper;

    let mut tray = TrayManager::new()?;
    info!("System tray icon started");
    let mut session_machine = RecordingSessionMachine::new();

    println!(
        "Hold {} to record, release to transcribe.",
        config.hold_hotkey
    );
    println!(
        "Press {} to start recording, press again to stop.",
        config.toggle_hotkey
    );
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
                post_processor.as_ref(),
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
                    post_processor.as_ref(),
                    &typer,
                );
            }
        }

        if let Some(chunk) = recorder.take_ready_chunk() {
            let _ = drive_session(
                &mut session_machine,
                SessionEvent::ChunkReady {
                    session_id: chunk.session_id,
                    path: chunk.path,
                },
                &mut recorder,
                &orchestrator,
                &mut tray,
                post_processor.as_ref(),
                &typer,
            );
        }

        counter += 1;
        if counter % 300 == 0 {
            let status = format!("{:?}", session_machine.state());
            debug!(
                status = %status,
                hold_hotkey = %config.hold_hotkey,
                toggle_hotkey = %config.toggle_hotkey,
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
    post_processor: &dyn postprocess::TextPostProcessor,
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
                SessionEffect::SubmitChunk { session_id, path } => {
                    if let Err(error) = orchestrator.on_chunk_ready(session_id, path) {
                        warn!(session_id = session_id.0, error = %error, "Chunk was rejected");
                    }
                }
                SessionEffect::DeleteChunk { path } => {
                    if let Err(error) = audio::remove_temp_audio_file(&path) {
                        debug!(path, error = %error, "Could not delete stale chunk");
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
    post_processor: &dyn postprocess::TextPostProcessor,
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

fn handle_config(action: ConfigAction) {
    use crate::core::config::AppConfig;

    let mut config = AppConfig::load();

    match action {
        ConfigAction::List => {
            println!("{:<25} Value", "Key");
            println!("{}", "-".repeat(60));
            for key in &[
                "api_key",
                "transcription_api_url",
                "model",
                "hold_hotkey",
                "toggle_hotkey",
                "language",
                "prompt",
                "temperature",
                "mic_gain",
                "max_chunk_duration_secs",
                "max_chunk_size_bytes",
                "max_retries",
                "convergence_timeout_secs",
                "post_process_enabled",
                "post_process_streaming_enabled",
                "post_process_api_url",
                "post_process_api_key",
                "post_process_model",
                "post_process_prompt",
                "post_process_temperature",
                "local_mode",
                "local_data_dir",
                "local_server_port",
                "local_quantization",
            ] {
                let value = config
                    .get_field(key)
                    .unwrap_or_else(|| "(not set)".to_string());
                println!("{:<25} {}", key, value);
            }
        }

        ConfigAction::Get { key } => match config.get_field(&key) {
            Some(value) => println!("{}", value),
            None => {
                eprintln!("Error: unknown config key '{}'", key);
                std::process::exit(1);
            }
        },

        ConfigAction::Set { key, value } => match config.set_field(&key, &value) {
            Ok(()) => {
                if let Err(e) = config.save() {
                    eprintln!("Failed to save config: {}", e);
                    std::process::exit(1);
                }
                println!("Set {} = {}", key, value);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
    }
}

fn handle_convert(input: &str, output: Option<&str>) {
    use postprocess::create_post_processor;
    use transcriber::{Transcriber, create_transcriber};

    println!("Transcribing: {}", input);

    let config = AppConfig::load();
    let (config, local_manager) = match prepare_runtime_config(config) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to prepare runtime: {}", e);
            std::process::exit(1);
        }
    };
    let _local_manager = LocalServiceGuard::new(local_manager);
    let transcriber: Box<dyn Transcriber> = match create_transcriber(&config) {
        Ok(transcriber) => transcriber,
        Err(e) => {
            eprintln!("Failed to initialize transcriber: {}", e);
            std::process::exit(1);
        }
    };
    let post_processor = create_post_processor(&config);

    match transcriber.transcribe(input) {
        Ok(stt_text) => {
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
                        std::process::exit(1);
                    }
                    println!("Saved to: {}", path);
                }
                None => println!("{}", text),
            }
        }
        Err(e) => {
            eprintln!("Transcription failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn apply_local_endpoint_overrides(config: &AppConfig, base_url: &str) -> AppConfig {
    let mut local = config.clone();
    local.api_key = Some("local".to_string());
    local.transcription_api_url = format!("{base_url}/v1/audio/transcriptions");
    if local.post_process_enabled {
        local.post_process_api_key = Some("local".to_string());
        local.post_process_api_url = Some(format!("{base_url}/v1/chat/completions"));
        local.post_process_model = Some(LOCAL_MODEL_NAME.to_string());
    }
    local
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::core::config::AppConfig;
    use audio::AudioRecorder;
    use transcriber::{MockTranscriber, Transcriber};

    #[test]
    fn test_audio_module_loads() {
        let audio_result = AudioRecorder::new(1.0);
        assert!(audio_result.is_ok());
    }

    #[test]
    fn test_full_pipeline_mock() {
        use input::typer::{MockTyper, TextTyper};
        let transcriber = MockTranscriber;
        let typer = MockTyper;
        let text = transcriber.transcribe("fake.wav").unwrap();
        assert!(typer.type_text(&text).is_ok());
    }

    #[test]
    fn test_orchestrator_integration_single_chunk() {
        use self::core::orchestrator::{SessionMode, SessionOrchestrator};
        use std::sync::Arc;
        use std::time::Duration;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(t, Some("en".to_string()), Duration::from_secs(5));

        orch.start_session(
            crate::core::recording_session::SessionId(1),
            SessionMode::Hold,
        )
        .unwrap();
        // MockTranscriber ignores the path, so a non-existent path is fine.
        let _ = orch.on_chunk_ready(
            crate::core::recording_session::SessionId(1),
            "fake_chunk.wav".to_string(),
        );
        let result = orch.finish_session(crate::core::recording_session::SessionId(1));

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_orchestrator_no_chunks() {
        use self::core::orchestrator::{SessionError, SessionMode, SessionOrchestrator};
        use std::sync::Arc;
        use std::time::Duration;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(t, None, Duration::from_secs(5));

        orch.start_session(
            crate::core::recording_session::SessionId(1),
            SessionMode::Toggle,
        )
        .unwrap();
        let result = orch.finish_session(crate::core::recording_session::SessionId(1));
        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn test_apply_local_endpoint_overrides_enabled_post_process() {
        let config = AppConfig {
            local_server_port: 17265,
            post_process_enabled: true,
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        };

        let local = apply_local_endpoint_overrides(&config, "http://127.0.0.1:17265");

        assert_eq!(
            local.transcription_api_url,
            "http://127.0.0.1:17265/v1/audio/transcriptions"
        );
        assert_eq!(
            local.post_process_api_url.as_deref(),
            Some("http://127.0.0.1:17265/v1/chat/completions")
        );
        assert!(local.post_process_enabled);
        assert_eq!(local.post_process_model.as_deref(), Some("gemma-4-E2B-it"));
        assert_eq!(local.api_key.as_deref(), Some("local"));
        assert_eq!(local.post_process_api_key.as_deref(), Some("local"));
    }

    #[test]
    fn test_apply_local_endpoint_overrides_keeps_post_process_disabled() {
        let config = AppConfig {
            local_server_port: 17265,
            post_process_enabled: false,
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        };

        let local = apply_local_endpoint_overrides(&config, "http://127.0.0.1:17265");

        assert_eq!(
            local.transcription_api_url,
            "http://127.0.0.1:17265/v1/audio/transcriptions"
        );
        assert!(!local.post_process_enabled);
        assert_eq!(local.post_process_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(local.post_process_api_url, None);
        assert_eq!(local.post_process_api_key, None);
        assert_eq!(local.api_key.as_deref(), Some("local"));
    }
}
