use std::sync::Mutex;
use std::thread;

use rdev::{Event, EventType, Key, listen};
use tracing::{debug, error, info, warn};

use crate::core::config::{ConfigKey, InputSection, ValidationIssue};

#[derive(Debug)]
pub struct HotkeyConfig {
    hold_key: Option<Key>,
    toggle_key: Option<Key>,
    pub(crate) hold_label: Option<String>,
    pub(crate) toggle_label: Option<String>,
}

/// Target policy used by the shared parser, validator, and listener diagnostics.
pub(crate) trait HotkeyPolicy: Send + 'static {
    fn unsupported_reason(key: Key) -> Option<&'static str>;

    fn pair_conflict(
        _first: Option<Key>,
        _second: Option<Key>,
    ) -> Option<(&'static str, &'static str)> {
        None
    }

    fn additional_warning(_key: Key) -> Option<&'static str> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NamedKey {
    key: Key,
    canonical: &'static str,
}

impl HotkeyConfig {
    pub(crate) fn validate<P: HotkeyPolicy>(
        section: &InputSection,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        let hold_binding = validate_binding::<P>(
            ConfigKey::InputHoldHotkey,
            &section.hold_hotkey,
            &mut issues,
        );
        let toggle_binding = validate_binding::<P>(
            ConfigKey::InputToggleHotkey,
            &section.toggle_hotkey,
            &mut issues,
        );

        if hold_binding.is_some()
            && hold_binding.map(|binding| binding.key) == toggle_binding.map(|binding| binding.key)
        {
            issues.push(ValidationIssue::new(
                ConfigKey::InputToggleHotkey,
                "hotkey.duplicate",
                "hold and toggle hotkeys must use different keys",
            ));
        }
        if let Some((code, message)) = P::pair_conflict(
            hold_binding.map(|binding| binding.key),
            toggle_binding.map(|binding| binding.key),
        ) {
            issues.push(ValidationIssue::new(
                ConfigKey::InputToggleHotkey,
                code,
                message,
            ));
        }

        if !issues.is_empty() {
            return Err(issues);
        }
        Ok(Self {
            hold_key: hold_binding.map(|binding| binding.key),
            toggle_key: toggle_binding.map(|binding| binding.key),
            hold_label: hold_binding.map(|binding| binding.canonical.to_string()),
            toggle_label: toggle_binding.map(|binding| binding.canonical.to_string()),
        })
    }
}

fn validate_binding<P: HotkeyPolicy>(
    key: ConfigKey,
    value: &str,
    issues: &mut Vec<ValidationIssue>,
) -> Option<NamedKey> {
    if value.trim().is_empty() {
        return None;
    }

    let Some(parsed_key) = parse_key(value) else {
        issues.push(ValidationIssue::new(
            key,
            "hotkey.invalid",
            format!("invalid hotkey `{value}`; expected a named single key such as F8 or RIGHTALT"),
        ));
        return None;
    };
    let parsed = parse_named_key(value).expect("parse_key delegates to parse_named_key");
    debug_assert_eq!(parsed.key, parsed_key);
    if let Some(reason) = P::unsupported_reason(parsed.key) {
        issues.push(ValidationIssue::new(
            key,
            "hotkey.unsupported",
            format!("hotkey `{}` is unsupported: {reason}", parsed.canonical),
        ));
        return None;
    }

    Some(parsed)
}

/// Parse a configured, named physical key. Empty strings disable a binding.
pub fn parse_key(value: &str) -> Option<Key> {
    parse_named_key(value).map(|named| named.key)
}

fn parse_named_key(value: &str) -> Option<NamedKey> {
    let (key, canonical) = match value.trim().to_ascii_uppercase().as_str() {
        "F1" => (Key::F1, "F1"),
        "F2" => (Key::F2, "F2"),
        "F3" => (Key::F3, "F3"),
        "F4" => (Key::F4, "F4"),
        "F5" => (Key::F5, "F5"),
        "F6" => (Key::F6, "F6"),
        "F7" => (Key::F7, "F7"),
        "F8" => (Key::F8, "F8"),
        "F9" => (Key::F9, "F9"),
        "F10" => (Key::F10, "F10"),
        "F11" => (Key::F11, "F11"),
        "F12" => (Key::F12, "F12"),
        "A" => (Key::KeyA, "A"),
        "B" => (Key::KeyB, "B"),
        "C" => (Key::KeyC, "C"),
        "D" => (Key::KeyD, "D"),
        "E" => (Key::KeyE, "E"),
        "F" => (Key::KeyF, "F"),
        "G" => (Key::KeyG, "G"),
        "H" => (Key::KeyH, "H"),
        "I" => (Key::KeyI, "I"),
        "J" => (Key::KeyJ, "J"),
        "K" => (Key::KeyK, "K"),
        "L" => (Key::KeyL, "L"),
        "M" => (Key::KeyM, "M"),
        "N" => (Key::KeyN, "N"),
        "O" => (Key::KeyO, "O"),
        "P" => (Key::KeyP, "P"),
        "Q" => (Key::KeyQ, "Q"),
        "R" => (Key::KeyR, "R"),
        "S" => (Key::KeyS, "S"),
        "T" => (Key::KeyT, "T"),
        "U" => (Key::KeyU, "U"),
        "V" => (Key::KeyV, "V"),
        "W" => (Key::KeyW, "W"),
        "X" => (Key::KeyX, "X"),
        "Y" => (Key::KeyY, "Y"),
        "Z" => (Key::KeyZ, "Z"),
        "0" => (Key::Num0, "0"),
        "1" => (Key::Num1, "1"),
        "2" => (Key::Num2, "2"),
        "3" => (Key::Num3, "3"),
        "4" => (Key::Num4, "4"),
        "5" => (Key::Num5, "5"),
        "6" => (Key::Num6, "6"),
        "7" => (Key::Num7, "7"),
        "8" => (Key::Num8, "8"),
        "9" => (Key::Num9, "9"),
        "BACKSPACE" => (Key::Backspace, "BACKSPACE"),
        "DELETE" => (Key::Delete, "DELETE"),
        "INSERT" => (Key::Insert, "INSERT"),
        "ENTER" | "RETURN" => (Key::Return, "ENTER"),
        "SPACE" => (Key::Space, "SPACE"),
        "TAB" => (Key::Tab, "TAB"),
        "ESCAPE" | "ESC" => (Key::Escape, "ESCAPE"),
        "UP" | "UPARROW" => (Key::UpArrow, "UP"),
        "DOWN" | "DOWNARROW" => (Key::DownArrow, "DOWN"),
        "LEFT" | "LEFTARROW" => (Key::LeftArrow, "LEFT"),
        "RIGHT" | "RIGHTARROW" => (Key::RightArrow, "RIGHT"),
        "HOME" => (Key::Home, "HOME"),
        "END" => (Key::End, "END"),
        "PAGEUP" => (Key::PageUp, "PAGEUP"),
        "PAGEDOWN" => (Key::PageDown, "PAGEDOWN"),
        "LEFTALT" | "ALT" | "LEFTOPTION" | "OPTION" => (Key::Alt, "LEFTALT"),
        "RIGHTALT" | "ALTGR" | "RIGHTOPTION" => (Key::AltGr, "RIGHTALT"),
        "LEFTCTRL" => (Key::ControlLeft, "LEFTCTRL"),
        "RIGHTCTRL" => (Key::ControlRight, "RIGHTCTRL"),
        "LEFTSHIFT" => (Key::ShiftLeft, "LEFTSHIFT"),
        "RIGHTSHIFT" => (Key::ShiftRight, "RIGHTSHIFT"),
        "LEFTMETA" | "COMMAND" | "WIN" | "SUPER" => (Key::MetaLeft, "LEFTMETA"),
        "RIGHTMETA" => (Key::MetaRight, "RIGHTMETA"),
        "CAPSLOCK" => (Key::CapsLock, "CAPSLOCK"),
        "NUMLOCK" => (Key::NumLock, "NUMLOCK"),
        "SCROLLLOCK" => (Key::ScrollLock, "SCROLLLOCK"),
        "PRINTSCREEN" => (Key::PrintScreen, "PRINTSCREEN"),
        "PAUSE" => (Key::Pause, "PAUSE"),
        "FUNCTION" => (Key::Function, "FUNCTION"),
        "BACKQUOTE" => (Key::BackQuote, "BACKQUOTE"),
        "MINUS" => (Key::Minus, "MINUS"),
        "EQUAL" => (Key::Equal, "EQUAL"),
        "LEFTBRACKET" => (Key::LeftBracket, "LEFTBRACKET"),
        "RIGHTBRACKET" => (Key::RightBracket, "RIGHTBRACKET"),
        "SEMICOLON" => (Key::SemiColon, "SEMICOLON"),
        "QUOTE" => (Key::Quote, "QUOTE"),
        "BACKSLASH" => (Key::BackSlash, "BACKSLASH"),
        "INTLBACKSLASH" => (Key::IntlBackslash, "INTLBACKSLASH"),
        "COMMA" => (Key::Comma, "COMMA"),
        "DOT" => (Key::Dot, "DOT"),
        "SLASH" => (Key::Slash, "SLASH"),
        "NUMPAD0" | "KP0" => (Key::Kp0, "NUMPAD0"),
        "NUMPAD1" | "KP1" => (Key::Kp1, "NUMPAD1"),
        "NUMPAD2" | "KP2" => (Key::Kp2, "NUMPAD2"),
        "NUMPAD3" | "KP3" => (Key::Kp3, "NUMPAD3"),
        "NUMPAD4" | "KP4" => (Key::Kp4, "NUMPAD4"),
        "NUMPAD5" | "KP5" => (Key::Kp5, "NUMPAD5"),
        "NUMPAD6" | "KP6" => (Key::Kp6, "NUMPAD6"),
        "NUMPAD7" | "KP7" => (Key::Kp7, "NUMPAD7"),
        "NUMPAD8" | "KP8" => (Key::Kp8, "NUMPAD8"),
        "NUMPAD9" | "KP9" => (Key::Kp9, "NUMPAD9"),
        "NUMPADENTER" | "KPENTER" => (Key::KpReturn, "NUMPADENTER"),
        "NUMPADMINUS" | "KPMINUS" => (Key::KpMinus, "NUMPADMINUS"),
        "NUMPADPLUS" | "KPPLUS" => (Key::KpPlus, "NUMPADPLUS"),
        "NUMPADMULTIPLY" | "KPMULTIPLY" => (Key::KpMultiply, "NUMPADMULTIPLY"),
        "NUMPADDIVIDE" | "KPDIVIDE" => (Key::KpDivide, "NUMPADDIVIDE"),
        "NUMPADDELETE" | "KPDELETE" => (Key::KpDelete, "NUMPADDELETE"),
        _ => return None,
    };
    Some(NamedKey { key, canonical })
}

fn needs_passthrough_warning(key: Key) -> bool {
    !matches!(
        key,
        Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::F11
            | Key::F12
    )
}

fn log_binding_warnings<P: HotkeyPolicy>(
    mode: &'static str,
    key: Option<Key>,
    label: Option<&str>,
) {
    let (Some(key), Some(label)) = (key, label) else {
        return;
    };
    if needs_passthrough_warning(key) {
        warn!(
            mode,
            hotkey = %label,
            "hotkey input is observed but not suppressed and may also affect the focused application or operating system"
        );
    }
    if let Some(message) = P::additional_warning(key) {
        warn!(
            mode,
            hotkey = %label,
            "{message}"
        );
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

    fn reset(&mut self) {
        self.hold_down = false;
        self.toggle_down = false;
    }

    fn map_filtered(&mut self, event_type: Option<EventType>) -> Option<HotkeyEvent> {
        match event_type {
            Some(event_type) => self.map(&event_type),
            None => {
                self.reset();
                None
            }
        }
    }
}

/// Start the process-lifetime global hotkey listener.
///
/// The detached `rdev` thread cannot be stopped independently; process shutdown is its lifetime
/// boundary. The filter returns `Some` to map an event or `None` to drop it and reset mapper
/// key-down bookkeeping.
pub(crate) fn start_hotkey_listener<P, F>(
    config: &HotkeyConfig,
    filter: F,
    notify: impl Fn(HotkeyEvent) + Send + 'static,
) where
    P: HotkeyPolicy,
    F: Fn(EventType) -> Option<EventType> + Send + 'static,
{
    let hold_key = config.hold_key;
    let toggle_key = config.toggle_key;
    spawn_listener(EventMapper::new(hold_key, toggle_key), filter, notify);

    log_binding_warnings::<P>("hold", hold_key, config.hold_label.as_deref());
    log_binding_warnings::<P>("toggle", toggle_key, config.toggle_label.as_deref());

    if let Some(label) = config.hold_label.as_deref() {
        info!(hotkey = %label, "hold hotkey registered");
    }
    if let Some(label) = config.toggle_label.as_deref() {
        info!(hotkey = %label, "toggle hotkey registered");
    }
}

fn spawn_listener<F>(mapper: EventMapper, filter: F, notify: impl Fn(HotkeyEvent) + Send + 'static)
where
    F: Fn(EventType) -> Option<EventType> + Send + 'static,
{
    thread::spawn(move || {
        debug!("rdev listener thread started");
        let mapper = Mutex::new(mapper);
        let callback = move |event: Event| {
            let event_type = filter(event.event_type);

            let mut mapper = mapper
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(event) = mapper.map_filtered(event_type) {
                notify(event);
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

    struct TestHotkeyPolicy;

    impl HotkeyPolicy for TestHotkeyPolicy {
        fn unsupported_reason(_key: Key) -> Option<&'static str> {
            None
        }
    }

    #[test]
    fn validates_default_disabled_and_invalid_hotkey_sections() {
        let config = HotkeyConfig::validate::<TestHotkeyPolicy>(&InputSection::default()).unwrap();
        assert_eq!(config.hold_key, Some(Key::F8));
        assert_eq!(config.toggle_key, Some(Key::F9));

        let tray_only = InputSection {
            hold_hotkey: String::new(),
            toggle_hotkey: String::new(),
        };
        let config = HotkeyConfig::validate::<TestHotkeyPolicy>(&tray_only).unwrap();
        assert_eq!(config.hold_key, None);
        assert_eq!(config.toggle_key, None);

        let invalid = InputSection {
            hold_hotkey: "F13".to_string(),
            toggle_hotkey: "F13".to_string(),
        };
        let issues = HotkeyConfig::validate::<TestHotkeyPolicy>(&invalid).unwrap_err();
        assert_eq!(issues[0].key, ConfigKey::InputHoldHotkey);

        let duplicate = InputSection {
            hold_hotkey: "RIGHTALT".to_string(),
            toggle_hotkey: "altgr".to_string(),
        };
        let issues = HotkeyConfig::validate::<TestHotkeyPolicy>(&duplicate).unwrap_err();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "hotkey.duplicate");
    }

    #[test]
    fn parses_every_canonical_named_key() {
        let cases = [
            ("F1", Key::F1),
            ("F2", Key::F2),
            ("F3", Key::F3),
            ("F4", Key::F4),
            ("F5", Key::F5),
            ("F6", Key::F6),
            ("F7", Key::F7),
            ("F8", Key::F8),
            ("F9", Key::F9),
            ("F10", Key::F10),
            ("F11", Key::F11),
            ("F12", Key::F12),
            ("A", Key::KeyA),
            ("B", Key::KeyB),
            ("C", Key::KeyC),
            ("D", Key::KeyD),
            ("E", Key::KeyE),
            ("F", Key::KeyF),
            ("G", Key::KeyG),
            ("H", Key::KeyH),
            ("I", Key::KeyI),
            ("J", Key::KeyJ),
            ("K", Key::KeyK),
            ("L", Key::KeyL),
            ("M", Key::KeyM),
            ("N", Key::KeyN),
            ("O", Key::KeyO),
            ("P", Key::KeyP),
            ("Q", Key::KeyQ),
            ("R", Key::KeyR),
            ("S", Key::KeyS),
            ("T", Key::KeyT),
            ("U", Key::KeyU),
            ("V", Key::KeyV),
            ("W", Key::KeyW),
            ("X", Key::KeyX),
            ("Y", Key::KeyY),
            ("Z", Key::KeyZ),
            ("0", Key::Num0),
            ("1", Key::Num1),
            ("2", Key::Num2),
            ("3", Key::Num3),
            ("4", Key::Num4),
            ("5", Key::Num5),
            ("6", Key::Num6),
            ("7", Key::Num7),
            ("8", Key::Num8),
            ("9", Key::Num9),
            ("BACKSPACE", Key::Backspace),
            ("DELETE", Key::Delete),
            ("INSERT", Key::Insert),
            ("ENTER", Key::Return),
            ("SPACE", Key::Space),
            ("TAB", Key::Tab),
            ("ESCAPE", Key::Escape),
            ("UP", Key::UpArrow),
            ("DOWN", Key::DownArrow),
            ("LEFT", Key::LeftArrow),
            ("RIGHT", Key::RightArrow),
            ("HOME", Key::Home),
            ("END", Key::End),
            ("PAGEUP", Key::PageUp),
            ("PAGEDOWN", Key::PageDown),
            ("LEFTALT", Key::Alt),
            ("RIGHTALT", Key::AltGr),
            ("LEFTCTRL", Key::ControlLeft),
            ("RIGHTCTRL", Key::ControlRight),
            ("LEFTSHIFT", Key::ShiftLeft),
            ("RIGHTSHIFT", Key::ShiftRight),
            ("LEFTMETA", Key::MetaLeft),
            ("RIGHTMETA", Key::MetaRight),
            ("CAPSLOCK", Key::CapsLock),
            ("NUMLOCK", Key::NumLock),
            ("SCROLLLOCK", Key::ScrollLock),
            ("PRINTSCREEN", Key::PrintScreen),
            ("PAUSE", Key::Pause),
            ("FUNCTION", Key::Function),
            ("BACKQUOTE", Key::BackQuote),
            ("MINUS", Key::Minus),
            ("EQUAL", Key::Equal),
            ("LEFTBRACKET", Key::LeftBracket),
            ("RIGHTBRACKET", Key::RightBracket),
            ("SEMICOLON", Key::SemiColon),
            ("QUOTE", Key::Quote),
            ("BACKSLASH", Key::BackSlash),
            ("INTLBACKSLASH", Key::IntlBackslash),
            ("COMMA", Key::Comma),
            ("DOT", Key::Dot),
            ("SLASH", Key::Slash),
            ("NUMPAD0", Key::Kp0),
            ("NUMPAD1", Key::Kp1),
            ("NUMPAD2", Key::Kp2),
            ("NUMPAD3", Key::Kp3),
            ("NUMPAD4", Key::Kp4),
            ("NUMPAD5", Key::Kp5),
            ("NUMPAD6", Key::Kp6),
            ("NUMPAD7", Key::Kp7),
            ("NUMPAD8", Key::Kp8),
            ("NUMPAD9", Key::Kp9),
            ("NUMPADENTER", Key::KpReturn),
            ("NUMPADMINUS", Key::KpMinus),
            ("NUMPADPLUS", Key::KpPlus),
            ("NUMPADMULTIPLY", Key::KpMultiply),
            ("NUMPADDIVIDE", Key::KpDivide),
            ("NUMPADDELETE", Key::KpDelete),
        ];

        for (name, expected) in cases {
            assert_eq!(parse_key(name), Some(expected), "name: {name}");
        }
    }

    #[test]
    fn parses_aliases_case_insensitively_and_canonicalizes_runtime_labels() {
        let aliases = [
            ("ALTGR", Key::AltGr),
            ("RIGHTOPTION", Key::AltGr),
            ("ALT", Key::Alt),
            ("LEFTOPTION", Key::Alt),
            ("OPTION", Key::Alt),
            ("RETURN", Key::Return),
            ("ESC", Key::Escape),
            ("COMMAND", Key::MetaLeft),
            ("WIN", Key::MetaLeft),
            ("SUPER", Key::MetaLeft),
            ("UPARROW", Key::UpArrow),
            ("DOWNARROW", Key::DownArrow),
            ("LEFTARROW", Key::LeftArrow),
            ("RIGHTARROW", Key::RightArrow),
            ("KP0", Key::Kp0),
            ("KP9", Key::Kp9),
            ("KPENTER", Key::KpReturn),
            ("KPMINUS", Key::KpMinus),
            ("KPPLUS", Key::KpPlus),
            ("KPMULTIPLY", Key::KpMultiply),
            ("KPDIVIDE", Key::KpDivide),
            ("KPDELETE", Key::KpDelete),
        ];

        for (name, expected) in aliases {
            assert_eq!(parse_key(name), Some(expected), "alias: {name}");
        }

        assert_eq!(parse_key(" rightoption "), Some(Key::AltGr));
        assert_eq!(parse_key("invalid"), None);

        let config = HotkeyConfig::validate::<TestHotkeyPolicy>(&InputSection {
            hold_hotkey: " rightoption ".to_string(),
            toggle_hotkey: "f9".to_string(),
        })
        .unwrap();
        assert_eq!(config.hold_label.as_deref(), Some("RIGHTALT"));
        assert_eq!(config.toggle_label.as_deref(), Some("F9"));
    }

    #[test]
    fn classifies_passthrough_risks() {
        assert!(!needs_passthrough_warning(Key::F8));
        assert!(needs_passthrough_warning(Key::KeyA));
        assert!(needs_passthrough_warning(Key::AltGr));
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

    #[test]
    fn callback_filter_can_drop_an_event_and_reset_mapper_state() {
        let mut mapper = EventMapper::new(Some(Key::KeyV), Some(Key::F9));

        assert_eq!(
            mapper.map_filtered(Some(EventType::KeyPress(Key::KeyV))),
            Some(HotkeyEvent::Pressed(HotkeySource::Hold))
        );
        assert_eq!(mapper.map_filtered(None), None);
        assert_eq!(
            mapper.map_filtered(Some(EventType::KeyPress(Key::KeyV))),
            Some(HotkeyEvent::Pressed(HotkeySource::Hold))
        );
    }

    #[test]
    fn maps_standalone_right_alt_hold_press_and_release() {
        let mut mapper = EventMapper::new(Some(Key::AltGr), Some(Key::F9));

        assert_eq!(
            mapper.map(&EventType::KeyPress(Key::AltGr)),
            Some(HotkeyEvent::Pressed(HotkeySource::Hold))
        );
        assert_eq!(
            mapper.map(&EventType::KeyRelease(Key::AltGr)),
            Some(HotkeyEvent::Released(HotkeySource::Hold))
        );
        assert_eq!(mapper.map(&EventType::KeyPress(Key::Alt)), None);
    }
}
