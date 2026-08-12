# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，按住热键即可将语音实时转录并输入到任意文本框。

灵感来源于 [Typeless](https://typeless.ai/)，默认使用 [Groq Whisper API](https://console.groq.com) 进行语音识别，也可通过配置切换到任何兼容 OpenAI multipart 格式的转写接口；也支持启动本地 Gemma 服务，在本机完成转写与文本整理。

## 功能特性

- **全局热键录音**：默认按住 F8 开始、松开停止（Hold 模式），按一下 F9 开始、再按一下停止（Toggle 模式）；两者均可改为具名单键，包括单独的右 Alt/Option
- **AI 语音识别**：通过可配置的 HTTP 转写接口将语音转为文字（默认 Groq Whisper）
- **长录音自动分片**：超过时长/大小限制的录音自动切分，后台并行转写，结果智能合并
- **LLM 文本后处理**：可选的 LLM 后处理层，自动补标点、去语气词、清理中断与重复
- **本地推理模式**：通过内置 `local` 子命令拉起 Python FastAPI 服务，使用 Gemma 4 本地模型提供 `/v1/audio/transcriptions` 与 `/v1/chat/completions`
- **自动文本输入**：识别结果自动输入到当前光标位置（支持中文等 Unicode 字符）
- **状态栏录音控制**：左键点击五柱 V 形声波图标即可开始或停止录音，右键打开退出菜单
- **灵活配置**：支持自定义热键、模型、语言、API 地址、麦克风增益等
- **自动清理**：自动保留最新 10 条录音，旧文件自动删除

## 系统要求

- **操作系统**：macOS 或 Windows
  - macOS：文字输入通过 System Events（osascript）实现，需在「系统设置 → 隐私与安全性 → 辅助功能」中授权终端应用
  - Windows：使用 SendInput API，无需额外权限
- **Rust**：仅源码构建需要支持 Rust 2024 edition 的 stable toolchain；安装发布包不需要 Rust
- **Python**：本地模式需要 Python 3.10+；安装时优先使用 `uv`，若未安装则回退到系统 Python（用于 FastAPI + Transformers 服务）

## 下载安装包

版本发布后可从 [GitHub Releases](https://github.com/b1indsight/viberwhisper/releases) 下载：

- macOS：`ViberWhisper-v<version>-macos-universal.dmg`，同时支持 Apple Silicon 与 Intel；
  `.tar.gz` 是保留应用权限的便携 `.app` 归档。
- Windows：`ViberWhisper-v<version>-windows-x86_64.msi`；`.zip` 是包含
  `viberwhisper.exe` 和 `LICENSE` 的便携版本。
- `SHA256SUMS` 包含四个发行文件的 SHA-256，可用于下载后校验；GitHub Release 同时提供
  artifact provenance。

当前发行包尚未使用 Apple Developer ID/notarization 或 Windows Authenticode 签名，因此系统可能显示
Gatekeeper/SmartScreen 警告。请先核对校验和与 GitHub provenance，再通过系统提供的确认入口运行。

> **当前发行范围**：DMG、MSI 和便携归档只包含 Rust 应用，支持 API inference profile；不携带
> `server/` 下的 Python/Gemma runtime。发行包中的 `local install`、`local start` 和 Local profile
> 暂不支持；本地模式目前仍需从源码目录运行。

## 快速开始

### 1. 获取 API 密钥

默认使用 Groq：前往 [Groq Console](https://console.groq.com) 注册并获取 API 密钥。

### 2. 配置

程序只读取严格的 nested v2 配置，不探测当前目录中的旧版平铺 `config.json`，也不会自动迁移。实际路径可用下面的命令查看：

```bash
viberwhisper config path
```

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/config.json`
- Windows：`%APPDATA%\ViberWhisper\config.json`

配置文件不存在时使用内置默认值。可将 [`config.example.json`](config.example.json) 复制到上述路径，或使用
`config set` 创建完整配置。已有文件必须包含 `schema_version: 2` 和当前完整结构；缺字段、未知字段、损坏
JSON 或其他 schema 版本都会明确报错，不会静默回退默认值。已退役的 `chunking`、`session` 和
`inference.api.provider` 均视为未知字段。

API 密钥优先从环境变量读取，其次才读取配置文件中的对应 secret 字段：

```bash
export TRANSCRIPTION_API_KEY=your_api_key_here
export POST_PROCESS_API_KEY=your_key_here
```

环境变量中的密钥只参与运行时解析，绝不会写回磁盘。`config get/list` 只显示密钥来源状态，不显示值；
secret key 也不能通过 `config set` 修改。

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

`local install` 会先校验本机 Python 版本是否为 3.10 或以上，然后优先使用 `uv` 创建虚拟环境和安装 `server/requirements.txt` 中的依赖；若未安装 `uv`，则回退到系统 Python。随后会下载 `google/gemma-4-E2B-it` 模型并校验安装结果。默认数据目录为 `~/.viberwhisper`，可通过 `inference.local.data_dir` 覆盖；如需 Hugging Face 镜像，可在安装前设置 `HF_ENDPOINT`。

### 4. 使用

1. 启动程序，系统托盘/状态栏会出现五柱声波图标，不再显示悬浮窗
2. 将光标定位到任意文本输入框（浏览器、编辑器、聊天框等）
3. 左键点击状态图标开始录音（五柱声波变红），再次点击停止
4. 也可以**按住 F8**开始录音，松开后自动转录并输入文字（Hold 模式）
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

# 显示配置路径 / 检查当前 profile 的运行配置
viberwhisper config path
viberwhisper config check

# 查看单个配置项
viberwhisper config get <key>

# 修改配置项
viberwhisper config set <key> <value>

# 离线转写 WAV 文件
viberwhisper convert input.wav
viberwhisper convert input.wav --output output.txt
```

## 配置说明

CLI 只接受下表中的 canonical dotted key；`hotkey`、`model`、`local_mode` 等旧别名不再支持。

### 输入与音频

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `input.hold_hotkey` | 字符串 | `F8` | 按住录音的具名单键；空字符串可禁用 |
| `input.toggle_hotkey` | 字符串 | `F9` | 切换录音的具名单键；空字符串可禁用 |
| `audio.mic_gain` | 数字 | `1.0` | 麦克风增益倍数 |

#### 单键热键名称

两个热键字段都接受大小写不敏感的物理单键名称。例如，将 Hold 热键设为右 Alt/Option：

```bash
viberwhisper config set input.hold_hotkey RIGHTALT
viberwhisper config check
```

`config set` 保留输入的原始字符串；`config check` 和程序启动负责验证名称、平台支持情况以及 Hold/Toggle
是否重复。运行时消息使用 canonical 名称，因此 `altgr`、`rightoption` 会显示为 `RIGHTALT`。

| 分组 | Canonical 名称 |
|------|----------------|
| 功能键 | `F1`–`F12` |
| 字母与数字 | `A`–`Z`、主键盘区 `0`–`9` |
| 编辑与空白 | `BACKSPACE`、`DELETE`、`INSERT`、`ENTER`、`SPACE`、`TAB`、`ESCAPE` |
| 导航 | `UP`、`DOWN`、`LEFT`、`RIGHT`、`HOME`、`END`、`PAGEUP`、`PAGEDOWN` |
| 修饰键 | `LEFTALT`、`RIGHTALT`、`LEFTCTRL`、`RIGHTCTRL`、`LEFTSHIFT`、`RIGHTSHIFT`、`LEFTMETA`、`RIGHTMETA` |
| 锁定与系统键 | `CAPSLOCK`、`NUMLOCK`、`SCROLLLOCK`、`PRINTSCREEN`、`PAUSE`、`FUNCTION` |
| 标点 | `BACKQUOTE`、`MINUS`、`EQUAL`、`LEFTBRACKET`、`RIGHTBRACKET`、`SEMICOLON`、`QUOTE`、`BACKSLASH`、`INTLBACKSLASH`、`COMMA`、`DOT`、`SLASH` |
| 数字键盘 | `NUMPAD0`–`NUMPAD9`、`NUMPADENTER`、`NUMPADMINUS`、`NUMPADPLUS`、`NUMPADMULTIPLY`、`NUMPADDIVIDE`、`NUMPADDELETE` |

常用别名包括：`ALTGR`/`RIGHTOPTION` → `RIGHTALT`，`ALT`/`OPTION` → `LEFTALT`，
`RETURN` → `ENTER`，`ESC` → `ESCAPE`，`COMMAND`/`WIN`/`SUPER` → `LEFTMETA`，箭头名称可追加
`ARROW`，数字键盘名称可将 `NUMPAD` 缩写为 `KP`。

名称对应 `rdev::Key` 的键位标识，而不是输入后产生的字符。macOS 后端按硬件键码映射；Windows 后端
使用虚拟键值，因此字母和标点在非 QWERTY 布局上可能跟随活动布局，使用前应实际确认。监听只观察而不
拦截按键，所以字母、数字、标点、编辑键和修饰键仍会传给当前应用或操作系统；程序会为这类绑定记录
warning。Windows 的 AltGr 通常表现为左 Ctrl + 右 Alt，因此 `RIGHTALT` 能正常匹配，但单独配置的
`LEFTCTRL` 也可能被 AltGr 触发；为避免一次按键产生两个录音动作，Windows 会拒绝同时配置
`LEFTCTRL` 和 `RIGHTALT`。

当前 `rdev 0.5.3` 后端还有以下明确限制，`config check` 会拒绝对应名称：

- macOS：`CAPSLOCK`、`RIGHTCTRL`、`DELETE`、`INSERT`、`HOME`、`END`、`PAGEUP`、
  `PAGEDOWN`、`NUMLOCK`、`SCROLLLOCK`、`PRINTSCREEN`、`PAUSE`、`INTLBACKSLASH` 和全部
  `NUMPAD*` 名称。
- Windows：`RIGHTMETA`、`FUNCTION`、`NUMPADENTER`；数字键盘的报告方式还会受到 Num Lock 影响。

内部可靠性策略不作为用户配置：实时与离线音频统一按 30 秒或 23 MiB 的较小限制分片；每个 STT 请求
最多等待 12 秒，网络错误或 HTTP 5xx 最多重试一次；停止录音后的 session 收敛窗口固定为 30 秒。

### 转写与 API profile

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `transcription.language` | 字符串/null | `zh` | 语言代码；`null` 为自动检测 |
| `transcription.prompt` | 字符串/null | 中文提示词 | 转写提示词 |
| `transcription.temperature` | 数字 | `0` | 转写温度 |
| `inference.active` | `api`/`local` | `api` | 默认使用的推理 profile |
| `inference.api.transcription.api_url` | URL | Groq Whisper URL | OpenAI-compatible multipart 地址 |
| `inference.api.transcription.model` | 字符串 | `whisper-large-v3-turbo` | 转写模型 |
| `inference.api.transcription.api_key` | secret | 无 | 只读状态；环境变量为 `TRANSCRIPTION_API_KEY` |

### LLM 后处理

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `post_process.enabled` | 布尔 | `false` | 是否启用 LLM 后处理 |
| `post_process.preheat_enabled` | 布尔 | `true` | 是否在录音中提前发送 LLM 请求 |
| `post_process.prompt` | 字符串/null | 内置默认 | 后处理系统提示词 |
| `post_process.temperature` | 数字 | `0.0` | 后处理温度 |
| `inference.api.post_process.api_url` | URL/null | 无 | OpenAI-compatible chat completions 地址 |
| `inference.api.post_process.model` | 字符串/null | 无 | LLM 模型名 |
| `inference.api.post_process.api_key` | secret | 无 | 只读状态；环境变量为 `POST_PROCESS_API_KEY` |

> **注意**：`config.json` 已在 `.gitignore` 中排除，避免误提交真实密钥。程序不会把环境变量中的密钥写入磁盘；手工写在 `config.json` 中的密钥会在更新其他配置项时原样保留。
> 后处理当前固定使用 OpenAI-compatible chat completions 请求格式。

### 本地模式

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `inference.local.data_dir` | 字符串/null | `~/.viberwhisper` | 模型、虚拟环境、PID 和日志目录 |
| `inference.local.server_port` | 数字 | `17265` | 本地 FastAPI 服务端口 |
| `inference.local.quantization` | 字符串 | `int8` | 可选 `int4` / `int8` / `bf16` |

将 `inference.active` 设为 `local` 后，默认监听和 `convert` 使用 Local profile。`local start` 只对本次运行做
Local override，不修改持久化配置。Local 请求使用显式无认证模式，不发送 `Authorization` 头。

### 切换转写服务

修改 API profile 的 endpoint、model 和环境密钥即可切换兼容接口：

```bash
./viberwhisper config set inference.api.transcription.api_url https://api.openai.com/v1/audio/transcriptions
./viberwhisper config set inference.api.transcription.model whisper-1
```

## LLM 后处理

启用后，转写结果会在输出前经过 LLM 整理，自动补标点、去除语气词、清理中断与重复。

### 启用方法

```bash
# 启用后处理
viberwhisper config set post_process.enabled true

# 配置 LLM API
viberwhisper config set inference.api.post_process.api_url https://api.openai.com/v1/chat/completions
viberwhisper config set inference.api.post_process.model gpt-4o-mini

# 设置 API 密钥（通过环境变量）
export POST_PROCESS_API_KEY=your_key_here
```

### 两种模式

- **预热模式**（默认，`post_process.preheat_enabled = true`）：录音过程中每收到一段稳定文本就提前发送 LLM 请求，录音结束后几乎零等待
- **保守模式**（`post_process.preheat_enabled = false`）：录音全部结束后一次性发送，零 token 浪费

后处理失败时自动降级为输出原始转写文本，不会导致整次录音失败。

## 本地 Gemma 服务

本地服务位于 [`server/server.py`](server/server.py)，通过 FastAPI 暴露两个 OpenAI 兼容端点：

- `POST /v1/audio/transcriptions`：接收 WAV 音频并调用 Gemma 音频理解能力返回转写结果
- `POST /v1/chat/completions`：供后处理模块复用，返回整理后的文本

Rust 侧的 `LocalServiceManager` 负责启动、健康检查、PID 记录、日志文件和关闭流程。`viberwhisper local start` 会先拉起服务，再进入正常监听循环；将 `inference.active` 设为 `local` 也会在启动主程序时自动做同样的准备。

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
- [winit](https://crates.io/crates/winit) - 跨平台原生事件循环
- [tracing](https://crates.io/crates/tracing) - 结构化日志

## 开发

```bash
cargo test     # 运行测试
cargo clippy   # 代码检查
cargo fmt      # 代码格式化
uv run pytest  # 运行 Python server 测试
```

维护者打包、版本校验、tag 发布、产物验证和失败恢复流程见
[`docs/releasing.md`](docs/releasing.md)。

## 许可证

MIT
