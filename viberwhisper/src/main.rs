mod audio;
mod hotkey;

use audio::AudioRecorder;
use hotkey::{HotkeyEvent, HotkeyManager};
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    // Initialize hotkey manager
    let hotkey_manager = HotkeyManager::new()?;

    // Initialize audio recorder (wrapped in Arc<Mutex> for shared state)
    let recorder = Arc::new(Mutex::new(AudioRecorder::new()?));
    let recorder_for_press = Arc::clone(&recorder);
    let recorder_for_release = Arc::clone(&recorder);

    println!("Hold F8 to record, release to save.");
    println!("Press Ctrl+C to exit.");
    println!();

    // Event loop
    loop {
        if let Some(event) = hotkey_manager.check_event() {
            match event {
                HotkeyEvent::Pressed => {
                    let mut rec = recorder_for_press.lock().unwrap();
                    if let Err(e) = rec.start_recording() {
                        eprintln!("Failed to start recording: {}", e);
                    }
                }
                HotkeyEvent::Released => {
                    let mut rec = recorder_for_release.lock().unwrap();
                    match rec.stop_recording() {
                        Ok(filename) => println!("Saved to: {}", filename),
                        Err(e) => eprintln!("Failed to stop recording: {}", e),
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_audio_module_loads() {
        // Test that audio module can be instantiated
        let audio_result = AudioRecorder::new();
        assert!(audio_result.is_ok());
    }
}
