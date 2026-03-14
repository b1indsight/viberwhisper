# 多平台支持 (macOS + Windows)

## 实现现状

**已完成**。当前代码通过条件编译同时支持 macOS 和 Windows。

| 模块 | 文件 | 现状 |
|------|------|------|
| typer trait | `src/input/typer.rs` | `TextTyper` trait + `MockTyper` |
| macOS 实现 | `src/platform/macos.rs` | `MacTyper`，使用 osascript + 剪贴板方案 |
| Windows 实现 | `src/platform/windows.rs` | `WindowsTyper`，使用 `SendInput` + `user32.dll` |
| audio | `src/audio/recorder.rs` | 使用 `cpal`，已跨平台 |
| hotkey | `src/input/hotkey.rs` | 使用 `rdev`，已跨平台 |
| config | `src/core/config.rs` | 纯 Rust，跨平台 |
| transcriber | `src/transcriber/groq.rs` | HTTP 调用，跨平台 |
| cli | `src/core/cli.rs` | 纯 Rust，跨平台 |

### macOS 权限说明

macOS 文字输入通过 `osascript` 调用 System Events 实现，需要辅助功能权限：

- 路径：「系统设置 → 隐私与安全性 → 辅助功能」
- 首次运行时系统会弹出授权对话框，允许即可
- 未授权时 osascript 会报错，文字输入失败但录音转录仍会完成

---

## 原始方案（存档）

### 核心问题（已解决）

原 `typer.rs` 只有 Windows 实现，`WindowsTyper` 直接调用 `user32.dll` 的 `SendInput` API，在 macOS 上链接失败。

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

**已采用方案 B（AppleScript + 剪贴板）**：实际实现使用剪贴板方案（`set the clipboard to` + `keystroke "v" using command down`），避免了 `keystroke` 的长度限制，支持任意长度的中英文文字。

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

## 文件变更清单（实际完成）

1. **新增** `src/platform/macos.rs` — macOS 文字输入实现 (`MacTyper`，剪贴板方案)
2. **新增** `src/platform/windows.rs` — Windows 文字输入实现 (`WindowsTyper`)
3. **新增** `src/platform/mod.rs` — 平台模块声明
4. **新增** `src/input/typer.rs` — `TextTyper` trait 定义 + `MockTyper`
5. **修改** `src/main.rs` — 条件编译选择 typer 实现
6. **修改** `changelog` — 新增条目

## 测试计划

- [x] macOS 上 `cargo build` 成功（实现已存在）
- [ ] macOS 上 `cargo test` 成功
- [ ] 验证 macOS 上 `MacTyper` 可以向当前窗口输入中英文文字
- [ ] 验证 Windows 上 `WindowsTyper` 行为不变（需要 Windows 环境）

## 后续优化

- Linux 支持（`xdotool` 或 `ydotool`）
- macOS 权限未授权时给出更明确的错误提示
