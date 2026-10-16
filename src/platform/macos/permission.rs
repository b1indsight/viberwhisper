use anyhow::{Result, anyhow, ensure};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSWorkspace};
use objc2_application_services::AXIsProcessTrusted;
use objc2_foundation::{NSString, NSURL};

pub(in crate::platform) fn text_delivery_available() -> bool {
    // SAFETY: AXIsProcessTrusted has no pointer parameters or calling-thread requirements.
    unsafe { AXIsProcessTrusted() }
}

/// Displays instructions on the AppKit main thread; opens settings only on a button click.
pub(in crate::platform) fn show_guidance() -> Result<()> {
    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("Accessibility guidance must run on the main thread"))?;
    objc2::rc::autoreleasepool(|_| {
        let alert = NSAlert::new(main_thread);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str("ViberWhisper 需要辅助功能权限"));
        alert.setInformativeText(&NSString::from_str(
            "自动输入文字需要辅助功能权限。\n\n请在「系统设置 → 隐私与安全性 → 辅助功能」中启用当前运行的 ViberWhisper；从终端运行时，也请检查该终端的授权。\n\n如果更新应用后失效，请移除旧条目，重新添加当前应用，并退出后重新启动。\n\n识别结果会保留在待确认气泡中，可选中文字按 Cmd+C 复制，再手动粘贴。",
        ));
        alert.addButtonWithTitle(&NSString::from_str("打开设置"));
        alert.addButtonWithTitle(&NSString::from_str("稍后"));
        if alert.runModal() == NSAlertFirstButtonReturn {
            let url = NSURL::URLWithString(&NSString::from_str(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            ))
            .ok_or_else(|| anyhow!("invalid Accessibility settings URL"))?;
            ensure!(
                NSWorkspace::sharedWorkspace().openURL(&url),
                "Could not open Accessibility settings; use System Settings → Privacy & Security → Accessibility"
            );
        }
        Ok(())
    })
}
