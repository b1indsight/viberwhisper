use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rdev::Key;

use super::backend::{HotkeyFilter, PlatformBackend};
use crate::input::hotkey::HotkeyPolicy;
use crate::input::tray::TrayPolicy;
use crate::input::typer::{MockTyper, TextTyper};

pub(crate) struct FallbackBackend;
pub(crate) struct FallbackHotkeys;
pub(crate) struct FallbackTray;

impl PlatformBackend for FallbackBackend {
    type Hotkeys = FallbackHotkeys;
    type Tray = FallbackTray;

    fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|base| base.join("viberwhisper"))
    }

    fn text_typer_and_hotkey_filter() -> (Arc<dyn TextTyper>, HotkeyFilter) {
        (Arc::new(MockTyper), Box::new(Some))
    }

    fn copy_to_clipboard(text: &str) -> Result<()> {
        tracing::info!(
            text_bytes = text.len(),
            "mock clipboard received history text"
        );
        Ok(())
    }
}

impl HotkeyPolicy for FallbackHotkeys {
    fn unsupported_reason(_key: Key) -> Option<&'static str> {
        None
    }
}

impl TrayPolicy for FallbackTray {
    fn idle_icon_is_template() -> bool {
        false
    }

    #[cfg(not(test))]
    fn prepare_application() -> Result<()> {
        Ok(())
    }

    #[cfg(not(test))]
    fn double_click_interval() -> Option<std::time::Duration> {
        None
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
