use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rdev::{listen, Event, EventType, Key};

// Thread-safe state for tracking key states
static F8_PRESSED: AtomicBool = AtomicBool::new(false);
static F8_RELEASED: AtomicBool = AtomicBool::new(false);

pub struct HotkeyManager {
    running: Arc<AtomicBool>,
}

pub enum HotkeyEvent {
    Pressed,
    Released,
}

impl HotkeyManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let running = Arc::new(AtomicBool::new(true));

        // Start rdev listener in a new thread
        thread::spawn(move || {
            println!("[DEBUG] rdev listener thread started");

            let callback = move |event: Event| {
                // Log ALL key events for debugging
                match &event.event_type {
                    EventType::KeyPress(key) => {
                        println!("[rdev] KeyPress: {:?}", key);
                        if *key == Key::F8 {
                            println!("[rdev] >>> F8 key PRESSED <<<");
                            F8_PRESSED.store(true, Ordering::Relaxed);
                        }
                    }
                    EventType::KeyRelease(key) => {
                        println!("[rdev] KeyRelease: {:?}", key);
                        if *key == Key::F8 {
                            println!("[rdev] >>> F8 key RELEASED <<<");
                            F8_RELEASED.store(true, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
            };

            if let Err(e) = listen(callback) {
                eprintln!("[ERROR] rdev listen failed: {:?}", e);
            }

            println!("[DEBUG] rdev listener thread exiting");
        });

        // Give the listener a moment to start
        thread::sleep(Duration::from_millis(100));

        println!("Registered global hotkey: F8");

        Ok(HotkeyManager { running })
    }

    pub fn check_event(&self) -> Option<HotkeyEvent> {
        // Check for F8 press event
        if F8_PRESSED.swap(false, Ordering::Relaxed) {
            return Some(HotkeyEvent::Pressed);
        }

        // Check for F8 release event
        if F8_RELEASED.swap(false, Ordering::Relaxed) {
            return Some(HotkeyEvent::Released);
        }

        None
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        println!("[DEBUG] HotkeyManager::drop() called");
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_manager_creation() {
        // Note: rdev listener requires appropriate permissions
        let _ = HotkeyManager::new();
    }
}
