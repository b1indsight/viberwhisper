#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::OverlayManager;

#[cfg(target_os = "windows")]
mod windows_impl;

#[cfg(target_os = "windows")]
pub use windows_impl::OverlayManager;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use stub::OverlayManager;
