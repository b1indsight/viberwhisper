use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use rdev::{Event, EventType, Key, listen};
use tracing::{debug, error, info};

use crate::core::config::{ConfigKey, InputSection, ValidationIssue};

#[derive(Debug)]
pub struct HotkeyConfig {
    hold_key: Option<Key>,
    toggle_key: Option<Key>,
    pub(crate) hold_label: Option<String>,
    pub(crate) toggle_label: Option<String>,
}

impl HotkeyConfig {
    pub(crate) fn validate(section: &InputSection) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        let hold_key = validate_binding(
            ConfigKey::InputHoldHotkey,
            &section.hold_hotkey,
            &mut issues,
        );
        let toggle_key = validate_binding(
            ConfigKey::InputToggleHotkey,
            &section.toggle_hotkey,
            &mut issues,
        );

        if hold_key.is_some() && hold_key == toggle_key {
            issues.push(ValidationIssue::new(
                ConfigKey::InputToggleHotkey,
                "hotkey.duplicate",
                "hold and toggle hotkeys must use different keys",
            ));
        }

        if !issues.is_empty() {
            return Err(issues);
        }
        Ok(Self {
            hold_key,
            toggle_key,
            hold_label: binding_label(&section.hold_hotkey),
            toggle_label: binding_label(&section.toggle_hotkey),
        })
    }
}

fn validate_binding(key: ConfigKey, value: &str, issues: &mut Vec<ValidationIssue>) -> Option<Key> {
    if value.trim().is_empty() {
        return None;
    }
    match parse_key(value) {
        Some(parsed) => Some(parsed),
        None => {
            issues.push(ValidationIssue::new(
                key,
                "hotkey.invalid",
                format!("invalid hotkey `{value}`; expected F1 through F12"),
            ));
            None
        }
    }
}

fn binding_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_ascii_uppercase())
}

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
    pub fn new(config: &HotkeyConfig) -> Self {
        let hold_key = config.hold_key;
        let toggle_key = config.toggle_key;
        let (sender, events) = mpsc::channel();
        spawn_listener(EventMapper::new(hold_key, toggle_key), sender);

        if let Some(label) = config.hold_label.as_deref() {
            info!(hotkey = %label, "hold hotkey registered");
        }
        if let Some(label) = config.toggle_label.as_deref() {
            info!(hotkey = %label, "toggle hotkey registered");
        }

        Self { events }
    }

    /// Return the oldest pending event without losing later events.
    pub fn check_event(&self) -> Option<HotkeyEvent> {
        self.events.try_recv().ok()
    }
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
    use crate::core::config::{ConfigKey, InputSection};

    #[test]
    fn validates_hotkey_section_before_manager_construction() {
        let config = HotkeyConfig::validate(&InputSection::default()).unwrap();
        assert_eq!(config.hold_key, Some(Key::F8));
        assert_eq!(config.toggle_key, Some(Key::F9));

        let tray_only = InputSection {
            hold_hotkey: String::new(),
            toggle_hotkey: String::new(),
        };
        let config = HotkeyConfig::validate(&tray_only).unwrap();
        assert_eq!(config.hold_key, None);
        assert_eq!(config.toggle_key, None);

        let invalid = InputSection {
            hold_hotkey: "F13".to_string(),
            toggle_hotkey: "F13".to_string(),
        };
        let issues = HotkeyConfig::validate(&invalid).unwrap_err();
        assert_eq!(issues[0].key, ConfigKey::InputHoldHotkey);

        let duplicate = InputSection {
            hold_hotkey: "F8".to_string(),
            toggle_hotkey: "f8".to_string(),
        };
        assert!(HotkeyConfig::validate(&duplicate).is_err());
    }

    #[test]
    fn parses_keys_case_insensitively_and_ignores_outer_whitespace() {
        assert_eq!(parse_key("F8"), Some(Key::F8));
        assert_eq!(parse_key(" f9 "), Some(Key::F9));
        assert_eq!(parse_key("F12"), Some(Key::F12));
        assert_eq!(parse_key("invalid"), None);
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
