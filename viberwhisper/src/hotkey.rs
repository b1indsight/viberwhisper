use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rdev::{listen, Event, EventType, Key};

// Thread-safe state for tracking key states
static HOLD_PRESSED: AtomicBool = AtomicBool::new(false);
static HOLD_RELEASED: AtomicBool = AtomicBool::new(false);
static TOGGLE_PRESSED: AtomicBool = AtomicBool::new(false);

/// 将热键字符串解析为 rdev::Key
pub fn parse_key(s: &str) -> Option<Key> {
    match s.to_uppercase().as_str() {
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HotkeySource {
    Hold,
    Toggle,
}

pub enum HotkeyEvent {
    Pressed(HotkeySource),
    Released(HotkeySource),
}

pub struct HotkeyManager {
    running: Arc<AtomicBool>,
}

impl HotkeyManager {
    pub fn new(
        hold_hotkey: &str,
        toggle_hotkey: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let hold_key = parse_key(hold_hotkey);
        let toggle_key = parse_key(toggle_hotkey);

        if hold_key.is_none() && toggle_key.is_none() {
            return Err("至少需要配置一个有效的热键 (hold_hotkey 或 toggle_hotkey)".into());
        }

        let running = Arc::new(AtomicBool::new(true));

        thread::spawn(move || {
            println!("[DEBUG] rdev listener thread started");

            let callback = move |event: Event| {
                match &event.event_type {
                    EventType::KeyPress(key) => {
                        if let Some(hk) = hold_key {
                            if *key == hk {
                                HOLD_PRESSED.store(true, Ordering::Relaxed);
                            }
                        }
                        if let Some(tk) = toggle_key {
                            if *key == tk {
                                TOGGLE_PRESSED.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        if let Some(hk) = hold_key {
                            if *key == hk {
                                HOLD_RELEASED.store(true, Ordering::Relaxed);
                            }
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

        if let Some(_) = hold_key {
            println!("Registered hold hotkey: {}", hold_hotkey);
        }
        if let Some(_) = toggle_key {
            println!("Registered toggle hotkey: {}", toggle_hotkey);
        }

        Ok(HotkeyManager { running })
    }

    pub fn check_event(&self) -> Option<HotkeyEvent> {
        if HOLD_PRESSED.swap(false, Ordering::Relaxed) {
            return Some(HotkeyEvent::Pressed(HotkeySource::Hold));
        }
        if HOLD_RELEASED.swap(false, Ordering::Relaxed) {
            return Some(HotkeyEvent::Released(HotkeySource::Hold));
        }
        if TOGGLE_PRESSED.swap(false, Ordering::Relaxed) {
            return Some(HotkeyEvent::Pressed(HotkeySource::Toggle));
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
    fn test_parse_key() {
        assert_eq!(parse_key("F8"), Some(Key::F8));
        assert_eq!(parse_key("f9"), Some(Key::F9));
        assert_eq!(parse_key("F12"), Some(Key::F12));
        assert_eq!(parse_key("invalid"), None);
    }

    #[test]
    fn test_hotkey_manager_creation() {
        // Note: rdev listener requires appropriate permissions
        let _ = HotkeyManager::new("F8", "F9");
    }
}
