mod backend;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback;
#[cfg(target_os = "macos")]
mod macos;
mod runtime;
#[cfg(target_os = "windows")]
mod windows;

use std::path::PathBuf;

use rdev::EventType;

use crate::core::config::{InputSection, ValidationIssue};
use crate::input::hotkey::HotkeyConfig;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fallback::FallbackBackend as SelectedBackend;
#[cfg(target_os = "macos")]
use macos::MacBackend as SelectedBackend;
#[cfg(target_os = "windows")]
use windows::WindowsBackend as SelectedBackend;

use backend::PlatformBackend;
pub(crate) use runtime::{PlatformAction, PlatformEvent, PlatformInterface};

pub(crate) type NativePlatform = runtime::PlatformRuntime<SelectedBackend>;

/// Returns the current target's application configuration directory.
pub(crate) fn config_dir() -> Option<PathBuf> {
    SelectedBackend::config_dir()
}

/// Gives the desktop process valid output handles without requiring a visible console.
pub(crate) fn prepare_desktop_output() -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    return windows::prepare_desktop_output();

    #[cfg(not(target_os = "windows"))]
    Ok(())
}

/// Reports a fatal error at the desktop process boundary where stderr may not be visible.
pub(crate) fn report_desktop_startup_error(error: &dyn std::error::Error) {
    #[cfg(target_os = "windows")]
    windows::show_startup_error(error);

    #[cfg(not(target_os = "windows"))]
    eprintln!("ViberWhisper failed to start: {error}");
}

/// Resolves persisted hotkey names with the policy selected for this build target.
pub(crate) fn validate_hotkeys(
    section: &InputSection,
) -> Result<HotkeyConfig, Vec<ValidationIssue>> {
    HotkeyConfig::validate::<<SelectedBackend as PlatformBackend>::Hotkeys>(section)
}

/// Applies the target's physical-key normalization without starting the desktop runtime.
pub(crate) fn normalize_setup_hotkey_event(event_type: EventType) -> EventType {
    #[cfg(target_os = "macos")]
    return macos::hotkey::normalize_event(event_type);

    #[cfg(not(target_os = "macos"))]
    event_type
}
