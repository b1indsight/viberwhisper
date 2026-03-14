# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，按住热键即可将语音实时转录并输入到任意文本框。

灵感来源于 [Typeless](https://typeless.ai/)，默认使用 [Groq Whisper API](https://console.groq.com) 进行语音识别，也可通过配置切换到任何兼容 OpenAI multipart 格式的转写接口。

## 功能特性

- **全局热键录音**：按住 F8（可配置）开始录音，松开自动停止
- **AI 语音识别**：通过可配置的 HTTP 转写接口将语音转为文字（默认 Groq Whisper）
- **自动文本输入**：识别结果自动输入到当前光标位置（支持中文等 Unicode 字符）
- **灵活配置**：支持自定义热键、模型、语言、API 地址、麦克风增益等
- **自动清理**：自动保留最新 10 条录音，旧文件自动删除

## 系统要求

- **操作系统**：macOS 或 Windows
  - macOS：文字输入通过 System Events（osascript）实现，需在「系统设置 → 隐私与安全性 → 辅助功能」中授权终端应用
  - Windows：使用 SendInput API，无需额外权限
- **Rust**：1.70 及以上版本

## 快速开始

### 1. 获取 API 密钥

默认使用 Groq：前往 [Groq Console](https://console.groq.com) 注册并获取 API 密钥。

### 2. 配置

项目已提供示例配置文件 `config.example.json`，先复制一份：

```bash
cp config.example.json config.json
```

然后按需修改 `config.json` 中的字段，例如：

```json
{
  "api_key": "YOUR_API_KEY_HERE",
  "transcription_api_url": "https://api.groq.com/openai/v1/audio/transcriptions",
  "model": "whisper-large-v3-turbo",
  "language": "zh",
  "prompt": "以下是一段简体中文的普通话句子，去掉首尾的语气词",
  "temperature": 0,
  "hold_hotkey": "F8",
  "toggle_hotkey": "F9",
  "mic_gain": 3.0
}
```

> **向后兼容**：旧版配置中的 `groq_api_key` 和 `hotkey` 字段仍可识别，自动映射到 `api_key` 和 `hold_hotkey`。

也可以通过环境变量设置 API 密钥（优先级高于配置文件中的 `groq_api_key`）：

```bash
export GROQ_API_KEY=your_api_key_here          # 旧版兼容
export TRANSCRIPTION_API_KEY=your_api_key_here  # 新版推荐（优先级更高）
```

### 3. 构建并运行

```bash
cargo build --release
cargo run --release
```

### 4. 使用

1. 启动程序，系统托盘会出现灰色图标
2. 将光标定位到任意文本输入框（浏览器、编辑器、聊天框等）
3. **按住 F8** 开始录音（图标变红），松开后自动转录并输入文字（Hold 模式）
4. 或按一下 **F9** 开始录音，再按一下停止（Toggle 模式）
5. 退出：右键点击托盘图标选择「退出」，或按 **Ctrl+C**

> macOS 首次运行时，系统会弹出辅助功能授权请求，需要允许才能完成文字输入。

## 配置说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `api_key` | 字符串 | 无 | 转写服务 API 密钥（必填，不写入配置文件） |
| `transcription_api_url` | 字符串 | Groq Whisper URL | 转写 API 地址（兼容 OpenAI multipart 格式） |
| `model` | 字符串 | `whisper-large-v3-turbo` | 转录模型 |
| `language` | 字符串 | `zh` | 语言代码，留空为自动检测 |
| `prompt` | 字符串 | 中文提示词 | 指导转录风格和格式 |
| `temperature` | 数字 | `0` | 随机性（0-1） |
| `hold_hotkey` | 字符串 | `F8` | 按住录音热键（Hold 模式） |
| `toggle_hotkey` | 字符串 | `F9` | 切换录音热键（Toggle 模式） |
| `mic_gain` | 数字 | `1.0` | 麦克风增益倍数 |

> **注意**：`config.json` 已在 `.gitignore` 中排除，避免误提交真实密钥；建议从 `config.example.json` 复制后再填写自己的配置。`api_key` 不会被程序写回磁盘，但如果你手动填进 `config.json`，文件里依然会存在明文密钥。

### 切换转写服务

只需修改 `transcription_api_url` 和 `api_key` 即可切换到任何兼容 OpenAI Whisper multipart 格式的接口：

```bash
# 命令行方式
./viberwhisper config set transcription_api_url https://api.openai.com/v1/audio/transcriptions
./viberwhisper config set model whisper-1
```

## 依赖项

- [rdev](https://crates.io/crates/rdev) - 全局热键监听
- [cpal](https://crates.io/crates/cpal) - 跨平台音频录制
- [hound](https://crates.io/crates/hound) - WAV 音频文件处理
- [dirs](https://crates.io/crates/dirs) - 跨平台目录路径获取
- [reqwest](https://crates.io/crates/reqwest) - HTTP 客户端，用于调用转写 API
- [serde_json](https://crates.io/crates/serde_json) - JSON 序列化/反序列化
- [tracing](https://crates.io/crates/tracing) - 结构化日志

## 开发

```bash
# 运行测试
cargo test

# 代码检查
cargo clippy

# 代码格式化
cargo fmt
```

## 许可证

MIT
