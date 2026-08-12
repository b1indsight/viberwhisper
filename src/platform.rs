mod backend;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback;
#[cfg(target_os = "macos")]
mod macos;
mod runtime;
#[cfg(target_os = "windows")]
mod windows;

use std::path::PathBuf;

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

/// Resolves persisted hotkey names with the policy selected for this build target.
pub(crate) fn validate_hotkeys(
    section: &InputSection,
) -> Result<HotkeyConfig, Vec<ValidationIssue>> {
    HotkeyConfig::validate::<<SelectedBackend as PlatformBackend>::Hotkeys>(section)
}
