mod audio;
mod core;
mod input;
mod platform;
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

/// Manages background transcription tasks spawned during a streaming (toggle) recording.
///
/// Each time a live chunk is ready, `dispatch` spawns a thread to transcribe it.
/// `collect` waits for all spawned threads and returns their results in order.
struct StreamingSession {
    handles: Vec<(usize, std::thread::JoinHandle<Result<String, String>>)>,
    next_index: usize,
}

impl StreamingSession {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
            next_index: 0,
        }
    }

    /// Spawn a background thread to transcribe `chunk_path`.
    fn dispatch(
        &mut self,
        chunk_path: String,
        transcriber: std::sync::Arc<Box<dyn transcriber::Transcriber>>,
    ) {
        let index = self.next_index;
        self.next_index += 1;
        info!(index = index, path = %chunk_path, "Dispatching background chunk transcription");
        let handle = std::thread::spawn(move || {
            transcriber
                .transcribe(&chunk_path)
                .map_err(|e| e.to_string())
                // Clean up the chunk file after transcription.
                .inspect(|_| {
                    if let Err(e) = std::fs::remove_file(&chunk_path) {
                        // Non-fatal: file might already be gone.
                        let _ = e;
                    }
                })
                .inspect_err(|_| {
                    let _ = std::fs::remove_file(&chunk_path);
                })
        });
        self.handles.push((index, handle));
    }

    fn has_pending(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Wait for all background threads and return results sorted by chunk index.
    fn collect(self) -> Vec<Result<String, String>> {
        let mut indexed: Vec<(usize, Result<String, String>)> = self
            .handles
            .into_iter()
            .map(|(idx, h)| {
                let result = h.join().unwrap_or_else(|_| Err("thread panicked".to_string()));
                (idx, result)
            })
            .collect();
        indexed.sort_by_key(|(idx, _)| *idx);
        indexed.into_iter().map(|(_, r)| r).collect()
    }
}

fn run_listener() -> Result<(), Box<dyn std::error::Error>> {
    use audio::{AudioRecorder, StopResult};
    use core::config::AppConfig;
    use core::orchestrator::{SessionError, SessionMode, SessionOrchestrator};
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::tray::TrayManager;
    use input::typer::TextTyper;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use transcriber::{create_transcriber, Transcriber};

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

    println!("Hold {} to record, release to transcribe.", config.hold_hotkey);
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
            Ok(text) => {
                if text.is_empty() {
                    info!("Transcription returned empty text");
                    return;
                }
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
                            finalize(stop_result);
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to stop recording");
                            tray.set_recording(false);
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
                                finalize(stop_result);
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to stop recording");
                                tray.set_recording(false);
                            }
                        }
                    } else {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, starting recording");
                        match rec.start_recording() {
                            Ok(()) => {
                                orchestrator.start_session(SessionMode::Toggle);
                                info!("Recording started (toggle mode)");
                                tray.set_recording(true);
                            }
                            Err(e) => error!(error = %e, "Failed to start recording"),
                        }
                    }
                }
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
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
    use transcriber::{create_transcriber, Transcriber};

    println!("Transcribing: {}", input);

    let config = AppConfig::load();
    let transcriber: Box<dyn Transcriber> = create_transcriber(&config);

    match transcriber.transcribe(input) {
        Ok(text) => match output {
            Some(path) => {
                if let Err(e) = std::fs::write(path, &text) {
                    eprintln!("Failed to write file: {}", e);
                    std::process::exit(1);
                }
                println!("Saved to: {}", path);
            }
            None => println!("{}", text),
        },
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
