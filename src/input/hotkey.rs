use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use rdev::{Event, EventType, Key, listen};
use tracing::{debug, error, info};

/// Parse a configured function key. Empty strings disable a binding.
pub fn parse_key(value: &str) -> Option<Key> {
    match value.trim().to_ascii_uppercase().as_str() {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySource {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed(HotkeySource),
    Released(HotkeySource),
}

#[derive(Debug)]
struct EventMapper {
    hold_key: Option<Key>,
    toggle_key: Option<Key>,
    hold_down: bool,
    toggle_down: bool,
}

impl EventMapper {
    fn new(hold_key: Option<Key>, toggle_key: Option<Key>) -> Self {
        Self {
            hold_key,
            toggle_key,
            hold_down: false,
            toggle_down: false,
        }
    }

    fn map(&mut self, event_type: &EventType) -> Option<HotkeyEvent> {
        match event_type {
            EventType::KeyPress(key) if Some(*key) == self.hold_key && !self.hold_down => {
                self.hold_down = true;
                Some(HotkeyEvent::Pressed(HotkeySource::Hold))
            }
            EventType::KeyRelease(key) if Some(*key) == self.hold_key && self.hold_down => {
                self.hold_down = false;
                Some(HotkeyEvent::Released(HotkeySource::Hold))
            }
            EventType::KeyPress(key) if Some(*key) == self.toggle_key && !self.toggle_down => {
                self.toggle_down = true;
                Some(HotkeyEvent::Pressed(HotkeySource::Toggle))
            }
            EventType::KeyRelease(key) if Some(*key) == self.toggle_key && self.toggle_down => {
                self.toggle_down = false;
                None
            }
            _ => None,
        }
    }
}

pub struct HotkeyManager {
    events: Receiver<HotkeyEvent>,
}

impl HotkeyManager {
    pub fn new(hold_hotkey: &str, toggle_hotkey: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let hold_key = parse_binding("hold_hotkey", hold_hotkey)?;
        let toggle_key = parse_binding("toggle_hotkey", toggle_hotkey)?;

        if hold_key.is_none() && toggle_key.is_none() {
            return Err("at least one hotkey must be configured".into());
        }
        if hold_key == toggle_key {
            return Err("hold_hotkey and toggle_hotkey must use different keys".into());
        }

        let (sender, events) = mpsc::channel();
        spawn_listener(EventMapper::new(hold_key, toggle_key), sender);

        if hold_key.is_some() {
            info!(hotkey = %hold_hotkey.trim(), "hold hotkey registered");
        }
        if toggle_key.is_some() {
            info!(hotkey = %toggle_hotkey.trim(), "toggle hotkey registered");
        }

        Ok(Self { events })
    }

    /// Return the oldest pending event without losing later events.
    pub fn check_event(&self) -> Option<HotkeyEvent> {
        self.events.try_recv().ok()
    }
}

fn parse_binding(name: &str, value: &str) -> Result<Option<Key>, Box<dyn std::error::Error>> {
    if value.trim().is_empty() {
        return Ok(None);
    }

    parse_key(value)
        .map(Some)
        .ok_or_else(|| format!("invalid {name} `{value}`; expected F1 through F12").into())
}

fn spawn_listener(mapper: EventMapper, sender: Sender<HotkeyEvent>) {
    thread::spawn(move || {
        debug!("rdev listener thread started");
        let mapper = Arc::new(Mutex::new(mapper));
        let callback = move |event: Event| {
            let mapped = match mapper.lock() {
                Ok(mut mapper) => mapper.map(&event.event_type),
                Err(poisoned) => poisoned.into_inner().map(&event.event_type),
            };
            if let Some(event) = mapped {
                let _ = sender.send(event);
            }
        };

        if let Err(err) = listen(callback) {
            error!(error = ?err, "rdev listen failed");
        }
        debug!("rdev listener thread exiting");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_case_insensitively_and_ignores_outer_whitespace() {
        assert_eq!(parse_key("F8"), Some(Key::F8));
        assert_eq!(parse_key(" f9 "), Some(Key::F9));
        assert_eq!(parse_key("F12"), Some(Key::F12));
        assert_eq!(parse_key("invalid"), None);
    }

    #[test]
    fn rejects_invalid_and_ambiguous_bindings_before_starting_listener() {
        assert!(HotkeyManager::new("F13", "F9").is_err());
        assert!(HotkeyManager::new("", "invalid").is_err());
        assert!(HotkeyManager::new("", "").is_err());
        assert!(HotkeyManager::new("F8", "f8").is_err());
    }

    #[test]
    fn maps_events_in_order_and_suppresses_key_repeat() {
        let mut mapper = EventMapper::new(Some(Key::F8), Some(Key::F9));

        assert_eq!(
            mapper.map(&EventType::KeyPress(Key::F8)),
            Some(HotkeyEvent::Pressed(HotkeySource::Hold))
        );
        assert_eq!(mapper.map(&EventType::KeyPress(Key::F8)), None);
        assert_eq!(
            mapper.map(&EventType::KeyRelease(Key::F8)),
            Some(HotkeyEvent::Released(HotkeySource::Hold))
        );
        assert_eq!(
            mapper.map(&EventType::KeyPress(Key::F9)),
            Some(HotkeyEvent::Pressed(HotkeySource::Toggle))
        );
        assert_eq!(mapper.map(&EventType::KeyPress(Key::F9)), None);
        assert_eq!(mapper.map(&EventType::KeyRelease(Key::F9)), None);
        assert_eq!(
            mapper.map(&EventType::KeyPress(Key::F9)),
            Some(HotkeyEvent::Pressed(HotkeySource::Toggle))
        );
    }
}
