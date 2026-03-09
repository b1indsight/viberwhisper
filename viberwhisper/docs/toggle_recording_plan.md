# 按键切换录音（Toggle Recording）实现计划

## 当前行为分析

### 热键系统 (`src/hotkey.rs`)
- 使用 `rdev` 库监听全局键盘事件
- 两个静态 `AtomicBool`：`F8_PRESSED` 和 `F8_RELEASED`
- `check_event()` 返回 `HotkeyEvent::Pressed` 或 `HotkeyEvent::Released`

### 主循环 (`src/main.rs` → `run_listener()`)
- `Pressed` → `start_recording()`
- `Released` → `stop_recording()` → `transcribe()` → `type_text()`
- 这是**长按模式**（hold-to-record）

### 配置 (`src/config.rs`)
- `hotkey: String` — 热键名称（默认 "F8"）
- 没有录音模式的配置项

## 需求

支持**切换模式**（toggle）：按一下开始录音，再按一下结束录音并开始识别。

## 实现方案

### 1. 配置新增 `recording_mode` 字段

**文件**: `src/config.rs`

```rust
// 新增枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RecordingMode {
    Hold,    // 长按录音（当前行为）
    Toggle,  // 按一下开始，再按一下结束
}

// AppConfig 新增字段
pub recording_mode: RecordingMode,

// Default 中设为 Toggle（用户要求的新行为）
recording_mode: RecordingMode::Toggle,
```

- `get_field` / `set_field` / `apply_json` 都需要支持 `recording_mode`

### 2. 修改主循环以支持两种模式

**文件**: `src/main.rs` → `run_listener()`

```rust
// Toggle 模式下的状态机：
// Idle → (按下F8) → Recording → (按下F8) → Stopping/Transcribing → Idle
//
// Hold 模式（现有行为）：
// Idle → (按下F8) → Recording → (释放F8) → Stopping/Transcribing → Idle

match config.recording_mode {
    RecordingMode::Hold => {
        // 现有逻辑不变
        match event {
            Pressed => start_recording(),
            Released => stop_and_transcribe(),
        }
    }
    RecordingMode::Toggle => {
        // 只响应 Pressed 事件，忽略 Released
        if let Pressed = event {
            if recorder.is_recording() {
                stop_and_transcribe();
            } else {
                start_recording();
            }
        }
    }
}
```

### 3. 边界情况处理

1. **快速双击**：Toggle 模式下快速按两次 → 录音时间极短 → `stop_recording()` 可能返回空 buffer → 已有 "No audio data recorded" 错误处理，OK
2. **录音中切换模式**：不支持运行时切换，需要重启程序
3. **按键重复**（长按产生连续 KeyPress）：`audio.rs` 的 `start_recording()` 已经有重复检测（"Already recording, ignoring duplicate start request"），所以 Toggle 模式下长按不会出问题
4. **提示文字**：根据模式显示不同的操作提示

### 4. 涉及文件变更清单

| 文件 | 变更内容 |
|------|---------|
| `src/config.rs` | 新增 `RecordingMode` 枚举，`AppConfig` 加 `recording_mode` 字段，更新 get/set/apply |
| `src/main.rs` | `run_listener()` 根据模式分发不同的热键处理逻辑 |

注意：`hotkey.rs` 和 `audio.rs` **不需要改动**，Toggle 逻辑完全在 `main.rs` 中处理。
