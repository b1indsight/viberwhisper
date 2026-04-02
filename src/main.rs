mod audio;
mod core;
mod input;
mod platform;
mod postprocess;
mod transcriber;

use clap::Parser;
use core::cli::{Cli, Commands, ConfigAction};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

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
        Some(Commands::Convert { input, output }) => {
            handle_convert(&input, output.as_deref());
        }
    }

    Ok(())
}

fn run_listener() -> Result<(), Box<dyn std::error::Error>> {
    use audio::{AudioRecorder, StopResult};
    use core::config::AppConfig;
    use core::orchestrator::{SessionError, SessionMode, SessionOrchestrator};
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::overlay::OverlayManager;
    use input::tray::TrayManager;
    use input::typer::TextTyper;
    use postprocess::create_post_processor;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use transcriber::{Transcriber, create_transcriber};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let config = AppConfig::load();
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

    let recorder = Arc::new(Mutex::new(AudioRecorder::with_config(
        config.mic_gain,
        config.max_chunk_duration_secs,
        config.max_chunk_size_bytes,
    )?));

    // Build transcriber and wrap in Arc<dyn Transcriber> for orchestrator injection.
    let transcriber: Arc<dyn Transcriber> = Arc::from(create_transcriber(&config));

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

    let mut overlay = OverlayManager::new()?;
    info!("Floating overlay window started");

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

    // Finalize a stopped recording: submit the tail chunk (if any) to the orchestrator,
    // then wait for convergence and type (or log) the result.
    let finalize = |stop_result: StopResult| {
        match stop_result {
            StopResult::SingleFile(path) | StopResult::TailChunk(path) => {
                orchestrator.on_chunk_ready(path);
            }
            StopResult::ChunksOnly => {
                debug!("All audio was flushed to background chunks during recording; no tail");
            }
        }

        match orchestrator.stop_session() {
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
                        Err(e) => {
                            warn!(error = %e, "Post-processing failed, using original STT text");
                            stt_text
                        }
                    }
                };
                info!(text = %text, "Typing transcribed text");
                if let Err(e) = typer.type_text(&text) {
                    error!(error = %e, "Failed to type text");
                }
            }
            Err(SessionError::NoChunks) => {
                warn!("No audio chunks to transcribe (recording too short?)");
            }
            Err(SessionError::PartialFailure {
                errors,
                partial_text,
            }) => {
                error!(
                    failed_chunks = errors.len(),
                    "Partial transcription failure; typing available text"
                );
                if !partial_text.is_empty() {
                    if let Err(e) = typer.type_text(&partial_text) {
                        error!(error = %e, "Failed to type partial text");
                    }
                }
            }
            Err(SessionError::ConvergenceTimeout {
                pending_count,
                partial_text,
            }) => {
                warn!(
                    pending_count = pending_count,
                    "Convergence timeout; typing available partial text"
                );
                if !partial_text.is_empty() {
                    if let Err(e) = typer.type_text(&partial_text) {
                        error!(error = %e, "Failed to type partial text");
                    }
                }
            }
        }
    };

    let mut counter = 0;
    loop {
        if let Some(event) = hotkey_manager.check_event() {
            match event {
                HotkeyEvent::Pressed(HotkeySource::Hold) => {
                    info!(hotkey = %config.hold_hotkey, "Hold key pressed, starting recording");
                    let mut rec = recorder.lock().unwrap();
                    match rec.start_recording() {
                        Ok(()) => {
                            orchestrator.start_session(SessionMode::Hold);
                            info!("Recording started (hold mode)");
                            tray.set_recording(true);
                            overlay.set_recording(true);
                        }
                        Err(e) => error!(error = %e, "Failed to start recording"),
                    }
                }
                HotkeyEvent::Released(HotkeySource::Hold) => {
                    info!(hotkey = %config.hold_hotkey, "Hold key released, stopping recording");
                    let mut rec = recorder.lock().unwrap();
                    match rec.stop_recording() {
                        Ok(stop_result) => {
                            tray.set_recording(false);
                            overlay.set_recording(false);
                            finalize(stop_result);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to stop recording");
                            tray.set_recording(false);
                            overlay.set_recording(false);
                        }
                    }
                }
                HotkeyEvent::Pressed(HotkeySource::Toggle) => {
                    let mut rec = recorder.lock().unwrap();
                    if rec.is_recording() {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, stopping recording");
                        match rec.stop_recording() {
                            Ok(stop_result) => {
                                tray.set_recording(false);
                                overlay.set_recording(false);
                                finalize(stop_result);
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to stop recording");
                                tray.set_recording(false);
                                overlay.set_recording(false);
                            }
                        }
                    } else {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, starting recording");
                        match rec.start_recording() {
                            Ok(()) => {
                                orchestrator.start_session(SessionMode::Toggle);
                                info!("Recording started (toggle mode)");
                                tray.set_recording(true);
                                overlay.set_recording(true);
                            }
                            Err(e) => error!(error = %e, "Failed to start recording"),
                        }
                    }
                }
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
            }
        }

        // Check overlay click (acts like toggle hotkey)
        if overlay.check_click() {
            let mut rec = recorder.lock().unwrap();
            if rec.is_recording() {
                info!("Overlay clicked, stopping recording");
                match rec.stop_recording() {
                    Ok(stop_result) => {
                        tray.set_recording(false);
                        overlay.set_recording(false);
                        finalize(stop_result);
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to stop recording");
                        tray.set_recording(false);
                        overlay.set_recording(false);
                    }
                }
            } else {
                info!("Overlay clicked, starting recording");
                match rec.start_recording() {
                    Ok(()) => {
                        orchestrator.start_session(SessionMode::Toggle);
                        info!("Recording started (overlay toggle)");
                        tray.set_recording(true);
                        overlay.set_recording(true);
                    }
                    Err(e) => error!(error = %e, "Failed to start recording"),
                }
            }
        }

        // Poll for in-recording chunks from the recorder and forward to the orchestrator.
        {
            let chunk_path = recorder.lock().unwrap().take_ready_chunk();
            if let Some(path) = chunk_path {
                info!(path = %path, "Ready chunk detected, forwarding to orchestrator");
                orchestrator.on_chunk_ready(path);
            }
        }

        if tray.check_exit() {
            info!("User clicked exit from tray");
            break Ok(());
        }

        counter += 1;
        if counter % 300 == 0 {
            let status = if recorder.lock().unwrap().is_recording() {
                "recording"
            } else {
                "idle"
            };
            debug!(
                status = status,
                hold_hotkey = %config.hold_hotkey,
                toggle_hotkey = %config.toggle_hotkey,
                "Heartbeat"
            );
        }

        overlay.update();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn handle_config(action: ConfigAction) {
    use core::config::AppConfig;

    let mut config = AppConfig::load();

    match action {
        ConfigAction::List => {
            println!("{:<25} {}", "Key", "Value");
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
                "post_process_api_format",
                "post_process_model",
                "post_process_prompt",
                "post_process_temperature",
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
    use core::config::AppConfig;
    use postprocess::create_post_processor;
    use transcriber::{Transcriber, create_transcriber};

    println!("Transcribing: {}", input);

    let config = AppConfig::load();
    let transcriber: Box<dyn Transcriber> = create_transcriber(&config);
    let post_processor = create_post_processor(&config);

    match transcriber.transcribe(input) {
        Ok(stt_text) => {
            let text = match post_processor.process(&stt_text) {
                Ok(processed) if !processed.is_empty() => processed,
                Ok(_) => {
                    warn!("Post-processing returned empty text, using original STT text");
                    stt_text
                }
                Err(e) => {
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

#[cfg(test)]
mod integration_tests {
    use super::*;
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

        orch.start_session(SessionMode::Hold);
        // MockTranscriber ignores the path, so a non-existent path is fine.
        orch.on_chunk_ready("fake_chunk.wav".to_string());
        let result = orch.stop_session();

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

        orch.start_session(SessionMode::Toggle);
        let result = orch.stop_session();
        assert!(matches!(result, Err(SessionError::NoChunks)));
    }
}
