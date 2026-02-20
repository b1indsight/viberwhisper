mod audio;
mod hotkey;
mod transcriber;
mod typer;

use audio::AudioRecorder;
use hotkey::{HotkeyEvent, HotkeyManager};
use std::sync::{Arc, Mutex};
use transcriber::{MockTranscriber, Transcriber};
use typer::{TextTyper, WindowsTyper};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let hotkey_manager = HotkeyManager::new()?;
    let recorder = Arc::new(Mutex::new(AudioRecorder::new()?));
    let transcriber = MockTranscriber;
    let typer = WindowsTyper;

    println!("Hold F8 to record, release to transcribe and type.");
    println!("Press Ctrl+C to exit.");
    println!();

    let mut counter = 0;
    loop {
        if let Some(event) = hotkey_manager.check_event() {
            match event {
                HotkeyEvent::Pressed => {
                    println!("F8 pressed, starting recording...");
                    let mut rec = recorder.lock().unwrap();
                    match rec.start_recording() {
                        Ok(()) => println!("Recording started."),
                        Err(e) => eprintln!("Failed to start recording: {}", e),
                    }
                }
                HotkeyEvent::Released => {
                    println!("F8 released, stopping recording...");
                    let mut rec = recorder.lock().unwrap();
                    match rec.stop_recording() {
                        Ok(wav_path) => {
                            println!("Recording saved: {}", wav_path);
                            match transcriber.transcribe(&wav_path) {
                                Ok(text) => {
                                    if let Err(e) = typer.type_text(&text) {
                                        eprintln!("Failed to type text: {}", e);
                                    }
                                }
                                Err(e) => eprintln!("Transcription failed: {}", e),
                            }
                        }
                        Err(e) => eprintln!("Failed to stop recording: {}", e),
                    }
                }
            }
        }

        counter += 1;
        if counter % 300 == 0 {
            println!("[Heartbeat] Running... Hold F8 to record");
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_audio_module_loads() {
        let audio_result = AudioRecorder::new();
        assert!(audio_result.is_ok());
    }

    #[test]
    fn test_full_pipeline_mock() {
        use typer::MockTyper;
        let transcriber = MockTranscriber;
        let typer = MockTyper;
        let text = transcriber.transcribe("fake.wav").unwrap();
        assert!(typer.type_text(&text).is_ok());
    }
}
