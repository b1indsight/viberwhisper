mod backend;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback;
#[cfg(target_os = "macos")]
mod macos;
mod preview;
mod runtime;
#[cfg(target_os = "windows")]
mod windows;

use rdev::EventType;

use crate::core::config::InputSection;
use crate::input::hotkey::HotkeyConfig;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fallback::FallbackBackend as SelectedBackend;
#[cfg(target_os = "macos")]
use macos::MacBackend as SelectedBackend;
#[cfg(target_os = "windows")]
use windows::WindowsBackend as SelectedBackend;

use backend::PlatformBackend;
pub(crate) use preview::{
    DeliveryTarget, NativePreview, NativeTarget, PreviewAction, PreviewEvent, PreviewRect,
};
pub(crate) use runtime::{PlatformAction, PlatformEvent};

#[cfg(feature = "integration")]
pub(crate) fn preview_typer() -> std::sync::Arc<dyn crate::input::typer::TextTyper> {
    SelectedBackend::text_typer_and_hotkey_filter().0
}

pub(crate) type NativePlatform = runtime::PlatformRuntime<SelectedBackend>;

/// Establishes native DPI scaling before setup, startup-error dialogs, or the event loop.
pub(crate) fn initialize_dpi_awareness() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    return windows::initialize_dpi_awareness();

    #[cfg(not(target_os = "windows"))]
    Ok(())
}

/// Gives the desktop process valid output handles without requiring a visible console.
pub(crate) fn prepare_desktop_output() -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    return windows::prepare_desktop_output();

    #[cfg(not(target_os = "windows"))]
    Ok(())
}

/// Reports a fatal error at the desktop process boundary where stderr may not be visible.
pub(crate) fn report_desktop_startup_error(error: &anyhow::Error) {
    #[cfg(target_os = "windows")]
    windows::show_startup_error(error);

    #[cfg(not(target_os = "windows"))]
    eprintln!("ViberWhisper failed to start: {error:#}");
}

/// Resolves persisted hotkey names with the policy selected for this build target.
pub(crate) fn hotkey_config(section: &InputSection) -> anyhow::Result<HotkeyConfig> {
    HotkeyConfig::from_section::<<SelectedBackend as PlatformBackend>::Hotkeys>(section)
}

/// Applies the target's physical-key normalization without starting the desktop runtime.
pub(crate) fn normalize_setup_hotkey_event(event_type: EventType) -> EventType {
    #[cfg(target_os = "macos")]
    return macos::hotkey::normalize_event(event_type);

    #[cfg(not(target_os = "macos"))]
    event_type
}

/// Checks whether the current process can deliver desktop text on this platform.
pub(crate) fn text_delivery_permission_available() -> bool {
    #[cfg(target_os = "macos")]
    return macos::text_delivery_available();

    #[cfg(not(target_os = "macos"))]
    true
}

/// Recognizes the typed macOS denial through anyhow context, without matching error text.
pub(crate) fn is_text_delivery_permission_error(error: &anyhow::Error) -> bool {
    #[cfg(target_os = "macos")]
    return macos::is_text_delivery_permission_error(error);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = error;
        false
    }
}

/// Shows platform-owned instructions; callers must run on the desktop main thread.
pub(crate) fn show_text_delivery_permission_guidance() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    return macos::show_guidance();

    #[cfg(not(target_os = "macos"))]
    Ok(())
}

/// Reports a failed device save without exposing configuration values in the dialog.
pub(crate) fn show_input_device_error() -> anyhow::Result<()> {
    const MESSAGE: &str =
        "无法保存输入设备，已保留原来的选择。\n请检查配置文件是否存在且可写，或重新运行设置。";

    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSAlert, NSAlertStyle};
        use objc2_foundation::NSString;

        let main_thread = MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("input device error must run on the main thread"))?;
        objc2::rc::autoreleasepool(|_| {
            let alert = NSAlert::new(main_thread);
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&NSString::from_str("ViberWhisper 输入设备"));
            alert.setInformativeText(&NSString::from_str(MESSAGE));
            alert.addButtonWithTitle(&NSString::from_str("确定"));
            alert.runModal();
        });
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
        };

        let message: Vec<u16> = MESSAGE.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "ViberWhisper 输入设备"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: Both UTF-16 buffers remain live and NUL-terminated throughout the modal call.
        if unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    eprintln!("{MESSAGE}");
    Ok(())
}
