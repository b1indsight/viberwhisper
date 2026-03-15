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
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::tray::TrayManager;
    use input::typer::TextTyper;
    use std::sync::{Arc, Mutex};
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
        "Config loaded"
    );

    let hotkey_manager = HotkeyManager::new(&config.hold_hotkey, &config.toggle_hotkey)?;

    let recorder = Arc::new(Mutex::new(AudioRecorder::with_config(
        config.mic_gain,
        config.max_chunk_duration_secs,
        config.max_chunk_size_bytes,
    )?));

    // Wrap transcriber in Arc so it can be shared across background threads.
    let transcriber: Arc<Box<dyn Transcriber>> = Arc::new(create_transcriber(&config));

    let language = config.language.clone();

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

    // Active streaming session for background transcription (used by both hold and toggle modes).
    let mut streaming: Option<StreamingSession> = None;

    /// Merge text segments using language-aware separator.
    fn merge_segments(segments: &[String], language: Option<&str>) -> String {
        let sep = match language {
            Some(lang) if lang.starts_with("zh") => "",
            _ => " ",
        };
        segments
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(sep)
    }

    // Stop recording, collect all chunk results (background + tail), merge and type.
    let stop_and_finalize =
        |rec: &mut AudioRecorder,
         session: Option<StreamingSession>,
         transcriber: &Arc<Box<dyn Transcriber>>,
         language: &Option<String>| {
            match rec.stop_recording() {
                Err(e) => {
                    error!(error = %e, "Failed to stop recording");
                    // Still drain any background session to avoid leaking threads.
                    if let Some(s) = session {
                        let _ = s.collect();
                    }
                }
                Ok(stop_result) => {
                    // Collect background chunk results first (in-order).
                    let mut all_texts: Vec<String> = match session {
                        None => Vec::new(),
                        Some(s) => {
                            if s.has_pending() {
                                info!("Waiting for background chunk transcriptions to complete");
                            }
                            s.collect()
                                .into_iter()
                                .filter_map(|r| match r {
                                    Ok(t) => Some(t),
                                    Err(e) => {
                                        error!(error = %e, "Background chunk transcription failed");
                                        None
                                    }
                                })
                                .collect()
                        }
                    };

                    // Transcribe the tail (or single full file).
                    match stop_result {
                        StopResult::SingleFile(path) | StopResult::TailChunk(path) => {
                            debug!(path = %path, "Transcribing tail/full recording");
                            match transcriber.transcribe(&path) {
                                Ok(text) => all_texts.push(text),
                                Err(e) => error!(error = %e, "Tail transcription failed"),
                            }
                            // Clean up the tail WAV.
                            if let Err(e) = std::fs::remove_file(&path) {
                                warn!(path = %path, error = %e, "Failed to delete tail WAV");
                            }
                        }
                        StopResult::ChunksOnly => {
                            debug!("All audio was flushed to background chunks; no tail");
                        }
                    }

                    if all_texts.is_empty() {
                        warn!("No transcription results to type");
                        return;
                    }

                    let merged = merge_segments(&all_texts, language.as_deref());
                    if merged.is_empty() {
                        info!("Transcription returned empty text");
                        return;
                    }

                    info!(text = %merged, "Typing transcribed text");
                    if let Err(e) = typer.type_text(&merged) {
                        error!(error = %e, "Failed to type text");
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
                            streaming = Some(StreamingSession::new());
                            info!("Recording started");
                            tray.set_recording(true);
                        }
                        Err(e) => error!(error = %e, "Failed to start recording"),
                    }
                }
                HotkeyEvent::Released(HotkeySource::Hold) => {
                    info!(hotkey = %config.hold_hotkey, "Hold key released, stopping recording");
                    let mut rec = recorder.lock().unwrap();
                    let session = streaming.take();
                    stop_and_finalize(&mut rec, session, &transcriber, &language);
                    tray.set_recording(false);
                }
                HotkeyEvent::Pressed(HotkeySource::Toggle) => {
                    let mut rec = recorder.lock().unwrap();
                    if rec.is_recording() {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, stopping recording");
                        let session = streaming.take();
                        stop_and_finalize(&mut rec, session, &transcriber, &language);
                        tray.set_recording(false);
                    } else {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, starting recording");
                        match rec.start_recording() {
                            Ok(()) => {
                                streaming = Some(StreamingSession::new());
                                info!("Recording started (streaming mode)");
                                tray.set_recording(true);
                            }
                            Err(e) => error!(error = %e, "Failed to start recording"),
                        }
                    }
                }
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
            }
        }

        // Poll for ready chunks from any active recording (hold or toggle) and dispatch them.
        if streaming.is_some() {
            let chunk_path = recorder.lock().unwrap().take_ready_chunk();
            if let Some(path) = chunk_path {
                info!(path = %path, "Ready chunk detected, dispatching background transcription");
                if let Some(session) = &mut streaming {
                    session.dispatch(path, Arc::clone(&transcriber));
                }
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
    fn test_streaming_session_collect_empty() {
        let session = StreamingSession::new();
        let results = session.collect();
        assert!(results.is_empty());
    }

    #[test]
    fn test_streaming_session_dispatch_and_collect() {
        use std::sync::Arc;
        let mut session = StreamingSession::new();
        let transcriber: Arc<Box<dyn Transcriber>> = Arc::new(Box::new(MockTranscriber));
        // Dispatch against a non-existent path — MockTranscriber ignores the path.
        session.dispatch("fake_chunk.wav".to_string(), Arc::clone(&transcriber));
        let results = session.collect();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }
}
