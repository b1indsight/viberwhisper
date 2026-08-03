#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

use std::path::PathBuf;

/// Returns the platform-specific application configuration directory.
pub(crate) fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return macos::config_dir();

    #[cfg(target_os = "windows")]
    return windows::config_dir();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    dirs::config_dir().map(|base| base.join("viberwhisper"))
}
