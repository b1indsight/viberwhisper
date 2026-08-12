use std::path::PathBuf;
use std::sync::Arc;

use rdev::EventType;

use crate::input::hotkey::HotkeyPolicy;
use crate::input::tray::TrayPolicy;
use crate::input::typer::TextTyper;

pub(crate) type HotkeyFilter = Box<dyn Fn(EventType) -> Option<EventType> + Send + 'static>;

/// Private contract implemented by the one backend selected for the build target.
pub(crate) trait PlatformBackend: 'static {
    type Hotkeys: HotkeyPolicy;
    type Tray: TrayPolicy;

    fn config_dir() -> Option<PathBuf>;
    fn text_typer_and_hotkey_filter() -> (Arc<dyn TextTyper>, HotkeyFilter);
}
