use crate::input::typer::TextTyper;
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{info, warn};

pub struct MacTyper;

/// Set the clipboard to `text` via `pbcopy` (no shell/AppleScript escaping needed).
fn set_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or("pbcopy stdin unavailable")?
        .write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        return Err("pbcopy failed".into());
    }
    Ok(())
}

/// Read the current clipboard as text, best-effort. Returns `None` for an
/// empty clipboard or non-text content (images etc. cannot be restored this
/// way, so we deliberately do not touch them afterwards).
fn read_clipboard_text() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    if text.is_empty() { None } else { Some(text) }
}

impl TextTyper for MacTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        if text.is_empty() {
            return Ok(());
        }

        // Give the target window time to regain focus
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Clipboard-paste approach: avoids keystroke length limits and special
        // character issues. The user's text clipboard is captured first and
        // restored after the paste lands.
        let previous_clipboard = read_clipboard_text();
        set_clipboard(text)?;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Events" to keystroke "v" using command down"#)
            .output();

        let restore = |reason: &str| {
            if let Some(previous) = &previous_clipboard
                && let Err(e) = set_clipboard(previous)
            {
                warn!(error = %e, reason, "Failed to restore previous clipboard");
            }
        };

        let output = match output {
            Ok(output) => output,
            Err(e) => {
                restore("paste command failed to run");
                return Err(e.into());
            }
        };
        if !output.status.success() {
            restore("paste command failed");
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("osascript failed: {}", stderr).into());
        }

        // Let the target app read the clipboard before putting the old content
        // back. This runs on the main loop thread, so skip the delay entirely
        // when there is nothing to restore.
        if previous_clipboard.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(300));
            restore("after paste");
        }

        info!(text = %text, "Text typed");
        Ok(())
    }
}
