# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，按住热键即可将语音实时转录并输入到任意文本框。

灵感来源于 [Typeless](https://typeless.ai/)，使用 [Groq Whisper API](https://console.groq.com) 进行语音识别。

## 功能特性

- **实时录音** - 按住热键开始录音，松开自动停止
- **AI 转录** - 使用 Groq Whisper API 进行语音识别
- **自动输入** - 转录完成后自动将文字输入到当前光标位置
- **可配置** - 支持自定义热键、模型、语言等参数
- **麦克风增益** - 支持调整麦克风增益以适应不同环境
- **自动清理** - 自动保留最新 10 条录音，旧文件自动删除

## 安装说明

### 环境要求

- Rust 1.70+ (Edition 2024)
- Windows 系统（依赖 Windows API 进行文字输入）

### 构建步骤

```bash
# 构建开发版本
cargo build

# 构建发布版本（推荐）
cargo build --release
```

## 配置说明

首次运行前需要在项目目录下创建 `config.json` 文件：

```json
{
  "groq_api_key": "YOUR_GROQ_API_KEY_HERE",
  "model": "whisper-large-v3-turbo",
  "language": "zh",
  "prompt": "以下是一段简体中文的普通话句子，去掉首尾的语气词",
  "temperature": 0,
  "hotkey": "F8",
  "mic_gain": 3.0
}
```

也可以通过环境变量设置 API 密钥：

```bash
set GROQ_API_KEY=your_api_key_here
```

### 配置项说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `groq_api_key` | string | 无 | Groq API 密钥，必需 |
| `model` | string | `whisper-large-v3-turbo` | Whisper 模型名称 |
| `hotkey` | string | `F8` | 录音热键 |
| `language` | string | `zh` | 识别语言，`zh` 表示中文，留空为自动检测 |
| `prompt` | string | 中文提示词 | 指导转录风格和格式 |
| `temperature` | number | `0` | 随机性（0-1） |
| `mic_gain` | number | `1.0` | 麦克风增益，默认 1.0 |

> **注意**：`config.json` 包含 API 密钥等敏感信息，已在 `.gitignore` 中排除，请勿提交到版本控制。

## 使用方法

1. 确保 `config.json` 已正确配置
2. 运行程序：
   ```bash
   cargo run --release
   ```
3. 将光标放到需要输入文字的位置（浏览器、编辑器、聊天框等）
4. 按住配置的热键（默认 F8）开始录音
5. 松开热键，程序会自动将语音转录为文字并输入
6. 按 `Ctrl+C` 退出程序

## 项目结构

```
viberwhisper/
├── src/
│   ├── main.rs          # 主程序入口，事件循环
│   ├── config.rs        # 配置加载和管理
│   ├── hotkey.rs        # 全局热键监听
│   ├── audio.rs         # 音频录制和 WAV 文件处理
│   ├── transcriber.rs   # 语音识别（Groq API）
│   └── typer.rs         # 文本输入（Windows SendInput API）
├── doc/                 # 功能文档
├── tmp/                 # 临时录音文件（自动管理）
├── config.json          # 运行时配置（不纳入版本控制）
└── Cargo.toml           # 项目依赖
```

## 依赖项

- [rdev](https://crates.io/crates/rdev) - 全局热键监听
- [cpal](https://crates.io/crates/cpal) - 跨平台音频录制
- [hound](https://crates.io/crates/hound) - WAV 音频文件处理
- [dirs](https://crates.io/crates/dirs) - 跨平台目录路径获取
- [reqwest](https://crates.io/crates/reqwest) - HTTP 客户端，用于调用 Groq API
- [serde_json](https://crates.io/crates/serde_json) - JSON 序列化/反序列化

## 开发

```bash
# 运行测试
cargo test

# 代码检查
cargo clippy

# 格式化代码
cargo fmt
```

## License

MIT License
