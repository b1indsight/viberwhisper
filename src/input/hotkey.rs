use std::thread;

use rdev::{Event, EventType, Key, listen};
use tracing::{debug, error, info, warn};

use crate::core::config::{ConfigKey, InputSection};

/// Parsed and platform-validated Hold/Toggle bindings for the recording listener.
#[derive(Debug)]
pub struct HotkeyConfig {
    pub(crate) hold: Option<NamedKey>,
    pub(crate) toggle: Option<NamedKey>,
}

/// Target policy used when constructing hotkeys and reporting listener diagnostics.
pub(crate) trait HotkeyPolicy: Send + 'static {
    fn unsupported_reason(key: Key) -> Option<&'static str>;

    fn pair_conflict(_first: Option<Key>, _second: Option<Key>) -> Option<&'static str> {
        None
    }

    fn additional_warning(_key: Key) -> Option<&'static str> {
        None
    }
}

/// A parsed physical key paired with its canonical configuration spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedKey {
    key: Key,
    pub(crate) canonical: &'static str,
}

impl HotkeyConfig {
    /// Parses configured names into distinct, supported bindings for the target.
    pub(crate) fn from_section<P: HotkeyPolicy>(section: &InputSection) -> anyhow::Result<Self> {
        let hold = parse_binding::<P>(ConfigKey::InputHoldHotkey, &section.hold_hotkey)?;
        let toggle = parse_binding::<P>(ConfigKey::InputToggleHotkey, &section.toggle_hotkey)?;

        if let (Some(hold), Some(toggle)) = (hold, toggle)
            && hold.key == toggle.key
        {
            anyhow::bail!("input.toggle_hotkey: hold and toggle hotkeys must use different keys");
        }
        if let Some(message) = P::pair_conflict(
            hold.map(|binding| binding.key),
            toggle.map(|binding| binding.key),
        ) {
            anyhow::bail!("input.toggle_hotkey: {message}");
        }

        Ok(Self { hold, toggle })
    }
}

fn parse_binding<P: HotkeyPolicy>(key: ConfigKey, value: &str) -> anyhow::Result<Option<NamedKey>> {
    if value.trim().is_empty() {
        return Ok(None);
    }

    let field = key.as_str();
    let parsed = parse_named_key(value).ok_or_else(|| {
        anyhow::anyhow!(
            "{field}: invalid hotkey `{value}`; expected a named single key such as F8 or RIGHTALT"
        )
    })?;
    if let Some(reason) = P::unsupported_reason(parsed.key) {
        anyhow::bail!(
            "{field}: hotkey `{}` is unsupported: {reason}",
            parsed.canonical
        );
    }

    Ok(Some(parsed))
}

/// Parse a configured, named physical key. Empty strings disable a binding.
pub fn parse_key(value: &str) -> Option<Key> {
    parse_named_key(value).map(|named| named.key)
}

// Shared spellings keep configuration parsing and captured-key labels in sync.
const NAMED_KEYS: &[(Key, &str, &[&str])] = &[
    (Key::F1, "F1", &[]),
    (Key::F2, "F2", &[]),
    (Key::F3, "F3", &[]),
    (Key::F4, "F4", &[]),
    (Key::F5, "F5", &[]),
    (Key::F6, "F6", &[]),
    (Key::F7, "F7", &[]),
    (Key::F8, "F8", &[]),
    (Key::F9, "F9", &[]),
    (Key::F10, "F10", &[]),
    (Key::F11, "F11", &[]),
    (Key::F12, "F12", &[]),
    (Key::KeyA, "A", &[]),
    (Key::KeyB, "B", &[]),
    (Key::KeyC, "C", &[]),
    (Key::KeyD, "D", &[]),
    (Key::KeyE, "E", &[]),
    (Key::KeyF, "F", &[]),
    (Key::KeyG, "G", &[]),
    (Key::KeyH, "H", &[]),
    (Key::KeyI, "I", &[]),
    (Key::KeyJ, "J", &[]),
    (Key::KeyK, "K", &[]),
    (Key::KeyL, "L", &[]),
    (Key::KeyM, "M", &[]),
    (Key::KeyN, "N", &[]),
    (Key::KeyO, "O", &[]),
    (Key::KeyP, "P", &[]),
    (Key::KeyQ, "Q", &[]),
    (Key::KeyR, "R", &[]),
    (Key::KeyS, "S", &[]),
    (Key::KeyT, "T", &[]),
    (Key::KeyU, "U", &[]),
    (Key::KeyV, "V", &[]),
    (Key::KeyW, "W", &[]),
    (Key::KeyX, "X", &[]),
    (Key::KeyY, "Y", &[]),
    (Key::KeyZ, "Z", &[]),
    (Key::Num0, "0", &[]),
    (Key::Num1, "1", &[]),
    (Key::Num2, "2", &[]),
    (Key::Num3, "3", &[]),
    (Key::Num4, "4", &[]),
    (Key::Num5, "5", &[]),
    (Key::Num6, "6", &[]),
    (Key::Num7, "7", &[]),
    (Key::Num8, "8", &[]),
    (Key::Num9, "9", &[]),
    (Key::Backspace, "BACKSPACE", &[]),
    (Key::Delete, "DELETE", &[]),
    (Key::Insert, "INSERT", &[]),
    (Key::Return, "ENTER", &["RETURN"]),
    (Key::Space, "SPACE", &[]),
    (Key::Tab, "TAB", &[]),
    (Key::Escape, "ESCAPE", &["ESC"]),
    (Key::UpArrow, "UP", &["UPARROW"]),
    (Key::DownArrow, "DOWN", &["DOWNARROW"]),
    (Key::LeftArrow, "LEFT", &["LEFTARROW"]),
    (Key::RightArrow, "RIGHT", &["RIGHTARROW"]),
    (Key::Home, "HOME", &[]),
    (Key::End, "END", &[]),
    (Key::PageUp, "PAGEUP", &[]),
    (Key::PageDown, "PAGEDOWN", &[]),
    (Key::Alt, "LEFTALT", &["ALT", "LEFTOPTION", "OPTION"]),
    (Key::AltGr, "RIGHTALT", &["ALTGR", "RIGHTOPTION"]),
    (Key::ControlLeft, "LEFTCTRL", &[]),
    (Key::ControlRight, "RIGHTCTRL", &[]),
    (Key::ShiftLeft, "LEFTSHIFT", &[]),
    (Key::ShiftRight, "RIGHTSHIFT", &[]),
    (Key::MetaLeft, "LEFTMETA", &["COMMAND", "WIN", "SUPER"]),
    (Key::MetaRight, "RIGHTMETA", &[]),
    (Key::CapsLock, "CAPSLOCK", &[]),
    (Key::NumLock, "NUMLOCK", &[]),
    (Key::ScrollLock, "SCROLLLOCK", &[]),
    (Key::PrintScreen, "PRINTSCREEN", &[]),
    (Key::Pause, "PAUSE", &[]),
    (Key::Function, "FUNCTION", &[]),
    (Key::BackQuote, "BACKQUOTE", &[]),
    (Key::Minus, "MINUS", &[]),
    (Key::Equal, "EQUAL", &[]),
    (Key::LeftBracket, "LEFTBRACKET", &[]),
    (Key::RightBracket, "RIGHTBRACKET", &[]),
    (Key::SemiColon, "SEMICOLON", &[]),
    (Key::Quote, "QUOTE", &[]),
    (Key::BackSlash, "BACKSLASH", &[]),
    (Key::IntlBackslash, "INTLBACKSLASH", &[]),
    (Key::Comma, "COMMA", &[]),
    (Key::Dot, "DOT", &[]),
    (Key::Slash, "SLASH", &[]),
    (Key::Kp0, "NUMPAD0", &["KP0"]),
    (Key::Kp1, "NUMPAD1", &["KP1"]),
    (Key::Kp2, "NUMPAD2", &["KP2"]),
    (Key::Kp3, "NUMPAD3", &["KP3"]),
    (Key::Kp4, "NUMPAD4", &["KP4"]),
    (Key::Kp5, "NUMPAD5", &["KP5"]),
    (Key::Kp6, "NUMPAD6", &["KP6"]),
    (Key::Kp7, "NUMPAD7", &["KP7"]),
    (Key::Kp8, "NUMPAD8", &["KP8"]),
    (Key::Kp9, "NUMPAD9", &["KP9"]),
    (Key::KpReturn, "NUMPADENTER", &["KPENTER"]),
    (Key::KpMinus, "NUMPADMINUS", &["KPMINUS"]),
    (Key::KpPlus, "NUMPADPLUS", &["KPPLUS"]),
    (Key::KpMultiply, "NUMPADMULTIPLY", &["KPMULTIPLY"]),
    (Key::KpDivide, "NUMPADDIVIDE", &["KPDIVIDE"]),
    (Key::KpDelete, "NUMPADDELETE", &["KPDELETE"]),
];

/// Converts a captured physical key into the canonical configuration spelling.
pub(crate) fn canonical_key_name(key: Key) -> Option<&'static str> {
    NAMED_KEYS
        .iter()
        .find_map(|&(candidate, canonical, _)| (candidate == key).then_some(canonical))
}

fn parse_named_key(value: &str) -> Option<NamedKey> {
    let value = value.trim();
    NAMED_KEYS
        .iter()
        .find(|(_, canonical, aliases)| {
            canonical.eq_ignore_ascii_case(value)
                || aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(value))
        })
        .map(|&(key, canonical, _)| NamedKey { key, canonical })
}

fn log_binding_warnings<P: HotkeyPolicy>(mode: &'static str, binding: Option<NamedKey>) {
    let Some(NamedKey {
        key,
        canonical: label,
    }) = binding
    else {
        return;
    };
    if !matches!(
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
    ) {
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
pub(crate) struct EventMapper {
    hold_key: Option<Key>,
    toggle_key: Option<Key>,
    hold_down: bool,
    toggle_down: bool,
}

impl EventMapper {
    pub(crate) fn new(hold_key: Option<Key>, toggle_key: Option<Key>) -> Self {
        Self {
            hold_key,
            toggle_key,
            hold_down: false,
            toggle_down: false,
        }
    }

    pub(crate) fn map(&mut self, event_type: &EventType) -> Option<HotkeyEvent> {
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

    fn map_filtered(&mut self, event_type: Option<EventType>) -> Option<HotkeyEvent> {
        match event_type {
            Some(event_type) => self.map(&event_type),
            None => {
                self.hold_down = false;
                self.toggle_down = false;
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
    let hold_key = config.hold.map(|binding| binding.key);
    let toggle_key = config.toggle.map(|binding| binding.key);
    spawn_listener(EventMapper::new(hold_key, toggle_key), filter, notify);

    log_binding_warnings::<P>("hold", config.hold);
    log_binding_warnings::<P>("toggle", config.toggle);

    if let Some(binding) = config.hold {
        info!(hotkey = %binding.canonical, "hold hotkey registered");
    }
    if let Some(binding) = config.toggle {
        info!(hotkey = %binding.canonical, "toggle hotkey registered");
    }
}

fn spawn_listener<F>(
    mut mapper: EventMapper,
    filter: F,
    notify: impl Fn(HotkeyEvent) + Send + 'static,
) where
    F: Fn(EventType) -> Option<EventType> + Send + 'static,
{
    thread::spawn(move || {
        debug!("rdev listener thread started");
        let callback = move |event: Event| {
            let event_type = filter(event.event_type);
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
    fn constructs_default_disabled_and_invalid_hotkey_sections() {
        let config =
            HotkeyConfig::from_section::<TestHotkeyPolicy>(&InputSection::default()).unwrap();
        assert_eq!(config.hold.map(|binding| binding.key), Some(Key::F8));
        assert_eq!(config.toggle.map(|binding| binding.key), Some(Key::F9));

        let tray_only = InputSection {
            hold_hotkey: String::new(),
            toggle_hotkey: String::new(),
        };
        let config = HotkeyConfig::from_section::<TestHotkeyPolicy>(&tray_only).unwrap();
        assert_eq!(config.hold, None);
        assert_eq!(config.toggle, None);

        let invalid = InputSection {
            hold_hotkey: "F13".to_string(),
            toggle_hotkey: "F13".to_string(),
        };
        let error = HotkeyConfig::from_section::<TestHotkeyPolicy>(&invalid).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with(ConfigKey::InputHoldHotkey.as_str())
        );

        let duplicate = InputSection {
            hold_hotkey: "RIGHTALT".to_string(),
            toggle_hotkey: "altgr".to_string(),
        };
        let error = HotkeyConfig::from_section::<TestHotkeyPolicy>(&duplicate).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("hold and toggle hotkeys must use different keys")
        );
    }

    #[test]
    fn named_key_catalog_has_unique_keys_and_spellings() {
        // A duplicate row or alias would make capture/parsing depend on catalog order.
        let mut keys = std::collections::HashSet::new();
        let mut spellings = std::collections::HashSet::new();
        for &(key, canonical, aliases) in NAMED_KEYS {
            assert!(keys.insert(key), "duplicate key: {key:?}");
            for name in std::iter::once(canonical).chain(aliases.iter().copied()) {
                assert!(
                    spellings.insert(name.to_ascii_uppercase()),
                    "duplicate spelling: {name}"
                );
            }
        }
    }

    #[test]
    fn parses_and_emits_every_canonical_named_key() {
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
            assert_eq!(canonical_key_name(expected), Some(name));
        }
        assert_eq!(canonical_key_name(Key::Unknown(42)), None);
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
            ("KP1", Key::Kp1),
            ("KP2", Key::Kp2),
            ("KP3", Key::Kp3),
            ("KP4", Key::Kp4),
            ("KP5", Key::Kp5),
            ("KP6", Key::Kp6),
            ("KP7", Key::Kp7),
            ("KP8", Key::Kp8),
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

        let config = HotkeyConfig::from_section::<TestHotkeyPolicy>(&InputSection {
            hold_hotkey: " rightoption ".to_string(),
            toggle_hotkey: "f9".to_string(),
        })
        .unwrap();
        assert_eq!(
            config.hold.map(|binding| binding.canonical),
            Some("RIGHTALT")
        );
        assert_eq!(config.toggle.map(|binding| binding.canonical), Some("F9"));
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
        // Paste suppression can hide a key-up event; either binding must accept the next press.
        for (key, source) in [
            (Key::KeyV, HotkeySource::Hold),
            (Key::F9, HotkeySource::Toggle),
        ] {
            let mut mapper = EventMapper::new(Some(Key::KeyV), Some(Key::F9));
            assert_eq!(
                mapper.map_filtered(Some(EventType::KeyPress(key))),
                Some(HotkeyEvent::Pressed(source))
            );
            assert_eq!(mapper.map_filtered(None), None);
            assert_eq!(
                mapper.map_filtered(Some(EventType::KeyPress(key))),
                Some(HotkeyEvent::Pressed(source))
            );
        }
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
