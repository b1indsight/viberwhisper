# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，按住热键即可将语音实时转录并输入到任意文本框。

灵感来源于 [Typeless](https://typeless.ai/)，默认使用 [Groq Whisper API](https://console.groq.com) 进行语音识别，也可通过配置切换到任何兼容 OpenAI multipart 格式的转写接口；也支持启动本地 Gemma 服务，在本机完成转写与文本整理。

## 功能特性

- **全局热键录音**：按住 F8 开始录音，松开自动停止（Hold 模式）；按一下 F9 开始，再按一下停止（Toggle 模式）
- **AI 语音识别**：通过可配置的 HTTP 转写接口将语音转为文字（默认 Groq Whisper）
- **长录音自动分片**：超过时长/大小限制的录音自动切分，后台并行转写，结果智能合并
- **LLM 文本后处理**：可选的 LLM 后处理层，自动补标点、去语气词、清理中断与重复
- **本地推理模式**：通过内置 `local` 子命令拉起 Python FastAPI 服务，使用 Gemma 4 本地模型提供 `/v1/audio/transcriptions` 与 `/v1/chat/completions`
- **自动文本输入**：识别结果自动输入到当前光标位置（支持中文等 Unicode 字符）
- **悬浮录音按钮**：启动后显示可拖拽悬浮窗，点击即可像 Toggle 热键一样开始/停止录音
- **灵活配置**：支持自定义热键、模型、语言、API 地址、麦克风增益等
- **自动清理**：自动保留最新 10 条录音，旧文件自动删除

## 系统要求

- **操作系统**：macOS 或 Windows
  - macOS：文字输入通过 System Events（osascript）实现，需在「系统设置 → 隐私与安全性 → 辅助功能」中授权终端应用
  - Windows：使用 SendInput API，无需额外权限
- **Rust**：1.70 及以上版本
- **Python**：本地模式需要 Python 3.11+（用于 FastAPI + Transformers 服务）

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
  "provider": "groq",
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

也可以通过环境变量设置 API 密钥（优先级高于配置文件）：

```bash
export GROQ_API_KEY=your_api_key_here          # 旧版兼容
export TRANSCRIPTION_API_KEY=your_api_key_here  # 转写 API 密钥（优先级最高）
export POST_PROCESS_API_KEY=your_key_here       # LLM 后处理 API 密钥
```

### 3. 构建并运行

```bash
cargo build --release
cargo run --release
```

如果要使用本地 Gemma 模式，先执行：

```bash
cargo run -- local install
cargo run -- local start
```

`local install` 会创建虚拟环境、安装 `server/requirements.txt` 中的依赖、下载 `google/gemma-4-E4B-it` 模型并校验安装结果。默认数据目录为 `~/.viberwhisper`，可通过 `local_data_dir` 覆盖；如需 Hugging Face 镜像，可在安装前设置 `HF_ENDPOINT`。

### 4. 使用

1. 启动程序，系统托盘会出现灰色图标
2. 屏幕上会出现一个可拖拽的悬浮录音按钮，点击行为等同于 Toggle 模式
3. 将光标定位到任意文本输入框（浏览器、编辑器、聊天框等）
4. **按住 F8** 开始录音（图标变红），松开后自动转录并输入文字（Hold 模式）
5. 或按一下 **F9** 开始录音，再按一下停止（Toggle 模式）
6. 退出：右键点击托盘图标选择「退出」，或按 **Ctrl+C**

> macOS 首次运行时，系统会弹出辅助功能授权请求，需要允许才能完成文字输入。

## CLI 命令

```bash
# 启动录音监听（默认，无子命令）
viberwhisper

# 安装 / 启动 / 停止 / 查看本地 Gemma 服务
viberwhisper local install
viberwhisper local start
viberwhisper local stop
viberwhisper local status

# 查看所有配置
viberwhisper config list

# 查看单个配置项
viberwhisper config get <key>

# 修改配置项
viberwhisper config set <key> <value>

# 离线转写 WAV 文件
viberwhisper convert input.wav
viberwhisper convert input.wav --output output.txt
```

## 配置说明

### 转写服务

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `api_key` | 字符串 | 无 | 转写服务 API 密钥（必填，不写入配置文件） |
| `transcription_api_url` | 字符串 | Groq Whisper URL | 转写 API 地址（兼容 OpenAI multipart 格式） |
| `provider` | 字符串 | 无 | 服务商标签（仅用于标注，不影响行为） |
| `model` | 字符串 | `whisper-large-v3-turbo` | 转录模型 |
| `language` | 字符串 | `zh` | 语言代码，留空为自动检测 |
| `prompt` | 字符串 | 中文提示词 | 指导转录风格和格式 |
| `temperature` | 数字 | `0` | 随机性（0-1） |

### 热键与音频

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `hold_hotkey` | 字符串 | `F8` | 按住录音热键（Hold 模式） |
| `toggle_hotkey` | 字符串 | `F9` | 切换录音热键（Toggle 模式） |
| `mic_gain` | 数字 | `1.0` | 麦克风增益倍数 |

### 音频分片

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `max_chunk_duration_secs` | 数字 | `30` | 每个分片最大秒数（0 = 不限） |
| `max_chunk_size_bytes` | 数字 | `24117248` | 每个分片最大字节数（0 = 不限） |
| `max_retries` | 数字 | `3` | 分片上传失败最大重试次数 |
| `convergence_timeout_secs` | 数字 | `30` | 录音结束后等待所有分片转写完成的超时秒数 |

### LLM 后处理

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `post_process_enabled` | 布尔 | `false` | 是否启用 LLM 后处理 |
| `post_process_streaming_enabled` | 布尔 | `true` | 是否启用预热模式（录音中提前发送 LLM 请求） |
| `post_process_api_url` | 字符串 | 无 | LLM chat completions API 地址 |
| `post_process_api_key` | 字符串 | 无 | LLM API 密钥（不写入配置文件，可通过 `POST_PROCESS_API_KEY` 环境变量设置） |
| `post_process_api_format` | 字符串 | `openai` | API 格式（目前仅支持 openai） |
| `post_process_model` | 字符串 | 无 | LLM 模型名（如 `gpt-4o-mini`） |
| `post_process_prompt` | 字符串 | 内置默认 | 后处理系统提示词 |
| `post_process_temperature` | 数字 | `0.0` | 后处理温度 |

> **注意**：`config.json` 已在 `.gitignore` 中排除，避免误提交真实密钥。`api_key` 和 `post_process_api_key` 不会被程序写回磁盘。

### 本地模式

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `local_mode` | 布尔 | `false` | 是否启用本地 Gemma 服务 |
| `local_data_dir` | 字符串 | `~/.viberwhisper` | 本地模型、虚拟环境、PID 和日志目录 |
| `local_server_port` | 数字 | `17265` | 本地 FastAPI 服务端口 |
| `local_quantization` | 字符串 | `int8` | 量化模式，可选 `int4` / `int8` / `bf16` |

启用 `local_mode` 后，程序会在启动监听前自动确保本地运行时已安装、拉起本地服务，并把转写端点重写为本地服务地址；如果同时启用了 `post_process_enabled`，后处理端点也会改写到本地 `/v1/chat/completions`。当前本地服务固定使用 `gemma-4-E4B-it` 作为模型名。

### 切换转写服务

只需修改 `transcription_api_url` 和 `api_key` 即可切换到任何兼容 OpenAI Whisper multipart 格式的接口：

```bash
./viberwhisper config set transcription_api_url https://api.openai.com/v1/audio/transcriptions
./viberwhisper config set model whisper-1
```

## LLM 后处理

启用后，转写结果会在输出前经过 LLM 整理，自动补标点、去除语气词、清理中断与重复。

### 启用方法

```bash
# 启用后处理
viberwhisper config set post_process_enabled true

# 配置 LLM API
viberwhisper config set post_process_api_url https://api.openai.com/v1/chat/completions
viberwhisper config set post_process_model gpt-4o-mini

# 设置 API 密钥（通过环境变量）
export POST_PROCESS_API_KEY=your_key_here
```

### 两种模式

- **预热模式**（默认，`post_process_streaming_enabled = true`）：录音过程中每收到一段稳定文本就提前发送 LLM 请求，录音结束后几乎零等待
- **保守模式**（`post_process_streaming_enabled = false`）：录音全部结束后一次性发送，零 token 浪费

后处理失败时自动降级为输出原始转写文本，不会导致整次录音失败。

## 本地 Gemma 服务

本地服务位于 [`server/server.py`](server/server.py)，通过 FastAPI 暴露两个 OpenAI 兼容端点：

- `POST /v1/audio/transcriptions`：接收 WAV 音频并调用 Gemma 音频理解能力返回转写结果
- `POST /v1/chat/completions`：供后处理模块复用，返回整理后的文本

Rust 侧的 `LocalServiceManager` 负责启动、健康检查、PID 记录、日志文件和关闭流程。`viberwhisper local start` 会先拉起服务，再进入正常监听循环；直接把 `local_mode` 设为 `true` 也会在启动主程序时自动做同样的准备。

当前本地服务限制单次音频请求最长 30 秒，因此长录音仍由 Rust 端先分片，再逐片提交给本地端点。

## 依赖项

- [rdev](https://crates.io/crates/rdev) - 全局热键监听
- [cpal](https://crates.io/crates/cpal) - 跨平台音频录制
- [hound](https://crates.io/crates/hound) - WAV 音频文件处理
- [dirs](https://crates.io/crates/dirs) - 跨平台目录路径获取
- [reqwest](https://crates.io/crates/reqwest) - HTTP 客户端
- [serde_json](https://crates.io/crates/serde_json) - JSON 序列化/反序列化
- [clap](https://crates.io/crates/clap) - CLI 参数解析
- [tray-icon](https://crates.io/crates/tray-icon) - 系统托盘图标
- [tracing](https://crates.io/crates/tracing) - 结构化日志

## 开发

```bash
cargo test     # 运行测试
cargo clippy   # 代码检查
cargo fmt      # 代码格式化
uv run pytest  # 运行 Python server 测试
```

## 许可证

MIT
