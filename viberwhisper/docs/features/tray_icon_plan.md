# 悬浮图标标识转换状态 — 实现计划

## 目标
在系统托盘（macOS 菜单栏 / Windows 任务栏）显示 ViberWhisper 状态图标，实时反映录音状态（空闲 / 录音中）。

## 技术选型
使用 `tray-icon` crate（Tauri 团队维护，v0.21+）：
- 跨平台：macOS + Windows
- 支持运行时动态切换图标
- 支持右键菜单
- 活跃维护，下载量大

## 实现方案

### 1. 新增依赖
```toml
tray-icon = "0.21"
image = "0.25"  # 用于加载图标
```

### 2. 图标资源
在 `assets/` 目录下放置两个图标：
- `icon_idle.png` — 空闲状态（灰色/白色麦克风）
- `icon_recording.png` — 录音中（红色麦克风）

图标尺寸：32x32 像素（macOS 菜单栏和 Windows 托盘标准尺寸）

### 3. 新增模块 `src/tray.rs`
```rust
pub struct TrayManager {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
}

impl TrayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>>;
    pub fn set_recording(&self, recording: bool);  // 切换图标
}
```

### 4. 主线程架构调整
**关键约束**：macOS 要求 tray icon 事件循环必须在主线程运行。

当前架构：主线程运行 hotkey 轮询循环。

调整方案：
- 主线程：运行 tray icon 事件循环 + hotkey 轮询（合并到同一循环）
- `TrayManager` 作为 `run_listener()` 的一部分初始化
- 录音状态变化时调用 `tray.set_recording(true/false)` 切换图标

### 5. 右键菜单
- "ViberWhisper" （标题，禁用）
- "状态：空闲" / "状态：录音中"
- 分隔线
- "退出"

### 6. 代码改动清单
| 文件 | 改动 |
|------|------|
| `Cargo.toml` | 添加 `tray-icon`, `image` 依赖 |
| `src/tray.rs` | 新增：TrayManager 实现 |
| `src/main.rs` | 引入 tray 模块，在 run_listener 中初始化并联动状态 |
| `assets/icon_idle.png` | 新增：空闲图标 |
| `assets/icon_recording.png` | 新增：录音中图标 |
| `changelog` | 添加记录 |

### 7. 条件编译
tray icon 功能在所有平台可用（tray-icon crate 本身跨平台），不需要额外的 `#[cfg]` 处理。

## 测试计划
- 单元测试：TrayManager 初始化、状态切换
- 手动测试：macOS 菜单栏图标显示、录音时图标切换、右键菜单功能
