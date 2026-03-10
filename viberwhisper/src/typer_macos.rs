use crate::typer::TextTyper;

pub struct MacTyper;

impl TextTyper for MacTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        if text.is_empty() {
            return Ok(());
        }

        // 给目标窗口一点时间重新获得焦点
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 使用剪贴板方案：先设置剪贴板内容，再模拟 Cmd+V 粘贴
        // 这样可以避免 keystroke 的长度限制和特殊字符问题
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
            return Err(format!("osascript 失败: {}", stderr).into());
        }

        println!("[MacTyper] 已输入: {}", text);
        Ok(())
    }
}
