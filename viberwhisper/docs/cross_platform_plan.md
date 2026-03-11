# 多平台支持计划 (macOS + Windows)

## 现状分析

### 平台相关模块

| 模块 | 文件 | 现状 | 需要改动 |
|------|------|------|----------|
| typer | `src/typer.rs` | 只有 Windows 实现 (`SendInput` + `user32.dll`)，macOS 上链接失败 | 需要 macOS 实现 |
| audio | `src/audio.rs` | 使用 `cpal`，已跨平台 | 无需改动 |
| hotkey | `src/hotkey.rs` | 使用 `rdev`，已跨平台 | 无需改动 |
| config | `src/config.rs` | 纯 Rust，跨平台 | 无需改动 |
| transcriber | `src/transcriber.rs` | HTTP 调用，跨平台 | 无需改动 |
| cli | `src/cli.rs` | 纯 Rust，跨平台 | 无需改动 |

### 核心问题

**唯一的平台障碍是 `typer.rs`**：
- `WindowsTyper` 直接调用 Windows `user32.dll` 的 `SendInput` API
- 在 macOS 上链接 `user32` 失败，导致整个项目无法编译
- macOS 上没有对应的文字输入实现

## 方案

### 1. typer.rs 重构

使用 `#[cfg(target_os)]` 条件编译，将平台实现分离：

```
src/typer.rs          — trait 定义 + MockTyper + 平台分发
src/typer_macos.rs    — macOS 实现 (CGEvent / AppleScript)
src/typer_windows.rs  — Windows 实现 (SendInput，现有代码迁移)
```

#### macOS 文字输入方案选择

| 方案 | 优点 | 缺点 |
|------|------|------|
| **A: CGEvent (Core Graphics)** | 底层、快速、无延迟 | 需要辅助功能权限，实现复杂 |
| **B: AppleScript / osascript** | 简单可靠，一行命令 | 需要 `System Events` 权限，有微小延迟 |
| **C: cliclick 等外部工具** | 简单 | 需要额外安装依赖 |

**推荐方案 B (AppleScript)**：
```rust
// macOS 实现核心逻辑
fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::thread::sleep(std::time::Duration::from_millis(100));
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        r#"tell application "System Events" to keystroke "{}""#,
        escaped
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()?;
    if !output.status.success() {
        return Err(format!("osascript 失败: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(())
}
```

优点：
- 无需额外 crate 依赖
- 支持中文等 Unicode 字符
- 实现简单可靠

注意：
- 首次使用需要在「系统偏好设置 → 隐私与安全性 → 辅助功能」中授权终端
- `keystroke` 有长度限制，超长文本需要改用剪贴板方案 (`set the clipboard to` + `keystroke "v" using command down`)

### 2. main.rs 修改

```rust
// 现有代码
let typer = WindowsTyper;

// 改为条件编译
#[cfg(target_os = "macos")]
let typer = MacTyper;
#[cfg(target_os = "windows")]
let typer = WindowsTyper;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
let typer = MockTyper;
```

### 3. Cargo.toml

无需新增依赖。`std::process::Command` 和 `osascript` 均为系统自带。

## 文件变更清单

1. **新增** `src/typer_macos.rs` — macOS 文字输入实现 (`MacTyper`)
2. **新增** `src/typer_windows.rs` — Windows 文字输入实现（从 `typer.rs` 迁移）
3. **修改** `src/typer.rs` — 只保留 trait 定义、MockTyper、条件编译 re-export
4. **修改** `src/main.rs` — 条件编译选择 typer 实现
5. **修改** `changelog` — 新增条目

## 测试计划

- [ ] macOS 上 `cargo build` 成功
- [ ] macOS 上 `cargo test` 成功
- [ ] 验证 macOS 上 `MacTyper` 可以向当前窗口输入中英文文字
- [ ] 验证 Windows 上 `WindowsTyper` 行为不变（需要 Windows 环境）

## 后续优化（本 PR 不做）

- macOS 上长文本改用剪贴板方案避免 keystroke 限制
- Linux 支持（`xdotool` 或 `ydotool`）
- 自动检测并提示辅助功能权限
