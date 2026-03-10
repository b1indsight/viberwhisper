# Toggle Recording 功能计划

## 需求

支持两种录音模式，通过**不同的热键**触发：

- **Hold 模式**: 按住热键录音，松开停止并转录（现有行为）
- **Toggle 模式**: 按一下热键开始录音，再按一下停止并转录（新增）

两种模式可以**同时存在**，各自绑定不同的热键。

## 配置变更

### 旧配置
```json
{
  "hotkey": "F8",
  "recording_mode": "toggle"
}
```

### 新配置
```json
{
  "hold_hotkey": "F8",
  "toggle_hotkey": "F9"
}
```

- `hold_hotkey`: Hold 模式的热键（默认 F8），设为空字符串禁用
- `toggle_hotkey`: Toggle 模式的热键（默认 F9），设为空字符串禁用
- 移除 `recording_mode` 字段和 `RecordingMode` 枚举

## 代码修改

### 1. config.rs
- 移除 `RecordingMode` 枚举及其 Display impl
- `AppConfig` 中将 `hotkey: String` + `recording_mode: RecordingMode` 替换为：
  - `hold_hotkey: String` (默认 "F8")
  - `toggle_hotkey: String` (默认 "F9")
- 更新 `get_field`、`set_field`、`apply_json` 支持新字段
- 更新测试

### 2. hotkey.rs
- 支持监听两个不同的按键
- `HotkeyEvent` 增加热键来源信息，区分是 hold 还是 toggle 触发
- 新结构：
  ```rust
  pub enum HotkeySource {
      Hold,
      Toggle,
  }

  pub enum HotkeyEvent {
      Pressed(HotkeySource),
      Released(HotkeySource),
  }
  ```
- `HotkeyManager::new()` 接收两个热键字符串参数
- 将硬编码 F8 改为根据配置的热键解析对应的 `rdev::Key`

### 3. main.rs
- `run_listener()` 中同时处理两种热键事件：
  - `Pressed(Hold)` → 开始录音
  - `Released(Hold)` → 停止录音并转录
  - `Pressed(Toggle)` → 根据 `is_recording()` 切换状态
  - `Released(Toggle)` → 忽略
- 移除 `recording_mode` 的 match 分支，改为基于 event source 分发

### 4. cli.rs
- config list 中展示 `hold_hotkey` 和 `toggle_hotkey`
- 移除 `recording_mode` 相关展示

## 向后兼容

- `apply_json` 中保留对旧 `hotkey` 字段的兼容读取（映射到 `hold_hotkey`）
- 旧 `recording_mode` 字段在加载时忽略

## 热键字符串到 rdev::Key 的映射

需要新增一个解析函数 `parse_key(s: &str) -> Option<rdev::Key>`，支持：
- F1-F12
- 可扩展其他按键
