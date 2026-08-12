use std::path::PathBuf;
use std::sync::Arc;

use rdev::Key;

use super::backend::{HotkeyFilter, PlatformBackend};
use crate::input::hotkey::HotkeyPolicy;
use crate::input::tray::TrayPolicy;
use crate::input::typer::TextTyper;
use tracing::info;

pub(crate) struct WindowsBackend;
pub(crate) struct WindowsHotkeys;
pub(crate) struct WindowsTray;

struct WindowsTyper;

impl PlatformBackend for WindowsBackend {
    type Hotkeys = WindowsHotkeys;
    type Tray = WindowsTray;

    fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|base| base.join("ViberWhisper"))
    }

    fn text_typer_and_hotkey_filter() -> (Arc<dyn TextTyper>, HotkeyFilter) {
        (Arc::new(WindowsTyper), Box::new(Some))
    }
}

impl HotkeyPolicy for WindowsHotkeys {
    fn unsupported_reason(key: Key) -> Option<&'static str> {
        match key {
            Key::MetaRight => Some("the current rdev Windows backend does not emit RIGHTMETA"),
            Key::Function => Some("Windows does not expose the Fn key to rdev"),
            Key::KpReturn => {
                Some("the current rdev Windows backend cannot distinguish it from ENTER")
            }
            _ => None,
        }
    }

    fn pair_conflict(
        first: Option<Key>,
        second: Option<Key>,
    ) -> Option<(&'static str, &'static str)> {
        matches!(
            (first, second),
            (Some(Key::ControlLeft), Some(Key::AltGr)) | (Some(Key::AltGr), Some(Key::ControlLeft))
        )
        .then_some((
            "hotkey.altgr_conflict",
            "LEFTCTRL and RIGHTALT cannot be used together because Windows AltGr emits both keys",
        ))
    }

    fn additional_warning(key: Key) -> Option<&'static str> {
        (key == Key::ControlLeft)
            .then_some("LEFTCTRL may also be emitted when Windows handles RIGHTALT as AltGr")
    }
}

impl TrayPolicy for WindowsTray {
    fn idle_icon_is_template() -> bool {
        false
    }

    #[cfg(not(test))]
    fn prepare_application() -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    #[cfg(not(test))]
    fn double_click_interval() -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(
            unsafe { ffi::GetDoubleClickTime() } as u64,
        ))
    }

    #[cfg(not(test))]
    fn set_icon(
        tray_icon: &tray_icon::TrayIcon,
        icon: tray_icon::Icon,
        _is_template: bool,
    ) -> tray_icon::Result<()> {
        tray_icon.set_icon(Some(icon))
    }
}

fn unicode_events(text: &str) -> Vec<(u16, bool)> {
    text.encode_utf16()
        .flat_map(|code_unit| [(code_unit, false), (code_unit, true)])
        .collect()
}

impl TextTyper for WindowsTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Give the target window time to regain focus
        std::thread::sleep(std::time::Duration::from_millis(100));

        let events = unicode_events(text);
        if events.is_empty() {
            return Ok(());
        }

        let mut inputs: Vec<ffi::INPUT> = events
            .into_iter()
            .map(|(code_unit, key_up)| ffi::make_key_input(code_unit, key_up))
            .collect();

        let sent = unsafe {
            ffi::SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<ffi::INPUT>() as i32,
            )
        };

        if sent as usize != inputs.len() {
            return Err(format!("SendInput only sent {}/{} events", sent, inputs.len()).into());
        }

        info!(text = %text, "Text typed");
        Ok(())
    }
}

#[allow(clippy::upper_case_acronyms)]
mod ffi {
    use std::mem::ManuallyDrop;

    pub const INPUT_KEYBOARD: u32 = 1;
    pub const KEYEVENTF_UNICODE: u32 = 0x0004;
    pub const KEYEVENTF_KEYUP: u32 = 0x0002;

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(non_snake_case)]
    pub struct KEYBDINPUT {
        pub wVk: u16,
        pub wScan: u16,
        pub dwFlags: u32,
        pub time: u32,
        pub dwExtraInfo: usize,
    }

    #[repr(C)]
    pub union INPUT_UNION {
        pub ki: ManuallyDrop<KEYBDINPUT>,
        pub _padding: [u8; 32],
    }

    #[repr(C)]
    pub struct INPUT {
        pub r#type: u32,
        pub _union: INPUT_UNION,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn SendInput(nInputs: u32, pInputs: *mut INPUT, cbSize: i32) -> u32;
        pub fn GetDoubleClickTime() -> u32;
    }

    pub fn make_key_input(scan_code: u16, key_up: bool) -> INPUT {
        let flags = if key_up {
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
        } else {
            KEYEVENTF_UNICODE
        };
        INPUT {
            r#type: INPUT_KEYBOARD,
            _union: INPUT_UNION {
                ki: ManuallyDrop::new(KEYBDINPUT {
                    wVk: 0,
                    wScan: scan_code,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::InputSection;
    use crate::input::hotkey::HotkeyConfig;

    #[test]
    fn windows_policy_owns_altgr_key_and_icon_rules() {
        let issues = HotkeyConfig::validate::<WindowsHotkeys>(&InputSection {
            hold_hotkey: "LEFTCTRL".to_string(),
            toggle_hotkey: "RIGHTALT".to_string(),
        })
        .unwrap_err();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "hotkey.altgr_conflict");
        for key in [Key::MetaRight, Key::Function, Key::KpReturn] {
            assert!(WindowsHotkeys::unsupported_reason(key).is_some(), "{key:?}");
        }
        assert!(WindowsHotkeys::additional_warning(Key::ControlLeft).is_some());
        assert_eq!(WindowsHotkeys::additional_warning(Key::AltGr), None);
        assert!(!WindowsTray::idle_icon_is_template());
    }

    #[test]
    fn unicode_input_pairs_each_utf16_unit_including_surrogates() {
        assert_eq!(
            unicode_events("A😀"),
            vec![
                (0x0041, false),
                (0x0041, true),
                (0xd83d, false),
                (0xd83d, true),
                (0xde00, false),
                (0xde00, true),
            ]
        );
    }
}
