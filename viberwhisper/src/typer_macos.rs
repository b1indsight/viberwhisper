use crate::typer::TextTyper;
use tracing::info;

pub struct MacTyper;

impl TextTyper for MacTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        if text.is_empty() {
            return Ok(());
        }

        // Give the target window time to regain focus
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Use clipboard approach: set clipboard content then simulate Cmd+V paste
        // This avoids keystroke length limits and special character issues
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"set the clipboard to "{}"
tell application "System Events" to keystroke "v" using command down"#,
            escaped
        );

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("osascript failed: {}", stderr).into());
        }

        info!(text = %text, "Text typed");
        Ok(())
    }
}
