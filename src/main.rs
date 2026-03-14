mod audio;
mod core;
mod input;
mod platform;
mod transcriber;

use clap::Parser;
use core::cli::{Cli, Commands, ConfigAction};
use tracing::{debug, error, info};
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
    use audio::AudioRecorder;
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
        provider = %config.provider,
        model = %config.model,
        language = %config.language.as_deref().unwrap_or("auto"),
        "Config loaded"
    );

    let hotkey_manager = HotkeyManager::new(&config.hold_hotkey, &config.toggle_hotkey)?;
    let recorder = Arc::new(Mutex::new(AudioRecorder::new(config.mic_gain)?));
    let transcriber: Box<dyn Transcriber> = create_transcriber(&config);

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

    let stop_and_transcribe = |rec: &mut AudioRecorder| {
        match rec.stop_recording() {
            Ok(wav_path) => {
                debug!(path = %wav_path, "Recording saved");
                match transcriber.transcribe(&wav_path) {
                    Ok(text) => {
                        if let Err(e) = typer.type_text(&text) {
                            error!(error = %e, "Failed to type text");
                        }
                    }
                    Err(e) => error!(error = %e, "Transcription failed"),
                }
            }
            Err(e) => error!(error = %e, "Failed to stop recording"),
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
                            info!("Recording started");
                            tray.set_recording(true);
                        }
                        Err(e) => error!(error = %e, "Failed to start recording"),
                    }
                }
                HotkeyEvent::Released(HotkeySource::Hold) => {
                    info!(hotkey = %config.hold_hotkey, "Hold key released, stopping recording");
                    let mut rec = recorder.lock().unwrap();
                    stop_and_transcribe(&mut rec);
                    tray.set_recording(false);
                }
                HotkeyEvent::Pressed(HotkeySource::Toggle) => {
                    let mut rec = recorder.lock().unwrap();
                    if rec.is_recording() {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, stopping recording");
                        stop_and_transcribe(&mut rec);
                        tray.set_recording(false);
                    } else {
                        info!(hotkey = %config.toggle_hotkey, "Toggle key pressed, starting recording");
                        match rec.start_recording() {
                            Ok(()) => {
                                info!("Recording started");
                                tray.set_recording(true);
                            }
                            Err(e) => error!(error = %e, "Failed to start recording"),
                        }
                    }
                }
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
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
            println!("{:<15} {}", "Key", "Value");
            println!("{}", "-".repeat(50));
            for key in &[
                "provider",
                "model",
                "hold_hotkey",
                "toggle_hotkey",
                "language",
                "prompt",
                "temperature",
                "mic_gain",
                "groq_api_key",
            ] {
                let value = config
                    .get_field(key)
                    .unwrap_or_else(|| "(not set)".to_string());
                println!("{:<15} {}", key, value);
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
}
