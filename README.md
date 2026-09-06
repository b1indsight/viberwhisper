# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，按住热键即可将语音实时转录并输入到任意文本框。

灵感来源于 [Typeless](https://typeless.ai/)，默认使用 [Groq Whisper API](https://console.groq.com) 进行语音识别，也可通过配置切换到任何兼容 OpenAI multipart 格式的转写接口。

## 功能特性

- **全局热键录音**：默认按住 F8 开始、松开停止（Hold 模式），按一下 F9 开始、再按一下停止（Toggle 模式）；两者均可改为具名单键，包括单独的右 Alt/Option
- **AI 语音识别**：通过可配置的 HTTP 转写接口将语音转为文字（默认 Groq Whisper）
- **静音幻听抑制**：在 STT 上传前本地过滤没有可听窗口的近静音分片，整段静音不会触发后处理、历史记录或文字输入
- **长录音自动分片**：超过时长/大小限制的录音自动切分，后台并行转写，结果智能合并
- **LLM 文本后处理**：可选的 LLM 后处理层，自动补标点、去语气词、清理中断与重复
- **自动文本输入**：识别结果自动输入到当前光标位置；macOS 原生控件优先使用辅助功能 API，Chromium 浏览器使用可恢复的剪贴板粘贴，支持中文等 Unicode 字符
- **本地识别历史**：最终用于输入的文本以 JSONL 追加到应用数据目录，右键菜单直接显示最近 5 条，点击即可复制完整原文
- **STT Prompt 回归调试**：采集并校正 WAV 测试集，全量重跑临时候选 prompt，以 WER、代理语义评分和专有名词准确率独立把关
- **状态栏录音控制**：左键点击五柱 V 形声波图标即可开始或停止录音，右键打开识别历史和退出菜单
- **灵活配置**：支持自定义热键、模型、语言、API 地址、麦克风增益等
- **自动清理**：自动保留最新 10 条录音，旧文件自动删除

## 系统要求

- **操作系统**：macOS 或 Windows
  - macOS：原生控件优先通过辅助功能 API 写入当前选择；Chromium 浏览器和不支持该能力的控件使用原生剪贴板与 Cmd+V。需在「系统设置 → 隐私与安全性 → 辅助功能」中授权正在运行的终端或 ViberWhisper
  - Windows：使用 SendInput API，无需额外权限
- **Rust**：仅源码构建需要支持 Rust 2024 edition 的 stable toolchain；安装发布包不需要 Rust

## 下载安装包

版本发布后可从 [GitHub Releases](https://github.com/b1indsight/viberwhisper/releases) 下载：

- macOS：`ViberWhisper-v<version>-macos-universal.dmg`，同时支持 Apple Silicon 与 Intel；
  `.tar.gz` 是保留应用权限的便携 `.app` 归档。
- Windows：`ViberWhisper-v<version>-windows-x86_64.msi`；开始菜单使用不显示命令行窗口的
  桌面入口。`.zip` 便携版本包含用于双击启动的 `viberwhisper-app.exe`、用于命令行操作的
  `viberwhisper.exe` 和 `LICENSE`。
- `SHA256SUMS` 包含四个发行文件的 SHA-256，可用于下载后校验；GitHub Release 同时提供
  artifact provenance。

当前发行包尚未使用 Apple Developer ID/notarization 或 Windows Authenticode 签名，因此系统可能显示
Gatekeeper/SmartScreen 警告。请先核对校验和与 GitHub provenance，再通过系统提供的确认入口运行。

> **当前发行范围**：DMG、MSI 和便携归档包含 Rust 应用，并通过可配置的
> OpenAI-compatible API endpoint 提供转写和可选文本后处理。

## 快速开始

### 1. 获取 API 密钥

默认使用 Groq：前往 [Groq Console](https://console.groq.com) 注册并获取 API 密钥。

### 2. 配置

程序只读取严格的 nested v3 配置，不探测当前目录中的旧版平铺 `config.json`，也不会自动迁移。实际路径可用下面的命令查看：

```bash
viberwhisper config path
```

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/config.json`
- Windows：`%APPDATA%\ViberWhisper\config.json`

正常监听启动时，如果配置文件不存在、无法读取或不能通过运行时校验，程序会显示跨平台的首次配置向导。
向导依次收集 STT、可选 LLM、热键和麦克风设置，并可录制一段真实语音验证转写。热键步骤先显示当前值；
需要修改时依次记录 Hold 和 Toggle 按键，最终确认后才应用。验证录音也使用这两个候选热键的正常语义，
验证确认窗口会说明关闭后如何说“测试”并结束录音，不通过对话框点击开始或停止。完成向导后才会原子
写入 `config.json`。选择跳过只会让本次启动使用内置默认值，不创建或覆盖文件。以后可随时重新运行：

```bash
viberwhisper setup
```

也可将 [`config.example.json`](config.example.json) 复制到上述路径，或使用 `config set` 修改普通字段。
已有文件必须包含 `schema_version: 3` 和当前结构；新增的可选 `audio.input_device` 缺失时兼容为系统默认设备。
其他缺字段、未知字段、损坏 JSON 或 schema 版本错误都会明确进入恢复向导。已退役的 `chunking`、`session`
和 `inference.api.provider` 均视为未知字段。由 v2 升级时，把 `schema_version` 改为 `3`，并删除
`inference.active` 与整个 `inference.local` 对象；原 Local 用户还需要填写可用的 API endpoint 和模型。

API 密钥优先从环境变量读取，其次才读取配置文件中的对应 secret 字段：

```bash
export TRANSCRIPTION_API_KEY=your_api_key_here
export POST_PROCESS_API_KEY=your_key_here
```

环境变量中的密钥只参与运行时解析，绝不会写回磁盘。`config get/list` 只显示密钥来源状态，不显示值；
secret key 也不能通过 `config set` 修改。向导密码框输入的密钥可以保存，但会在本机配置文件中以明文存在，
保存前会再次提示。

#### 识别历史文件

监听模式会把每次非空的最终文本追加到配置文件同目录下的 `history.jsonl`：

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/history.jsonl`
- Windows：`%APPDATA%\ViberWhisper\history.jsonl`

每行是一个按时间先后排列的独立 JSON 记录，例如
`{"text":"识别结果","metadata":{"created_at_unix_ms":1786612345678}}`。启动和追加前只校验
最后一行；如果 JSON 或元数据无效，就删除该行，再继续使用此前记录。文件限制为 5 MiB，
接近上限时淘汰最旧的完整记录，不截断正文。保存失败不会阻止自动输入。该文件是本机明文，
不包含音频、API 密钥或目标应用信息。

右键菜单直接显示最新 5 条；长文本在 40 个 Unicode 字符后缩略，但点击时复制的始终是完整原文。
复制动作不会自动粘贴，也不需要 macOS 辅助功能权限。

### 3. 构建并运行

```bash
cargo build --release
cargo run --release
```

### 4. 使用

1. 启动程序，系统托盘/状态栏会出现五柱声波图标，不再显示悬浮窗；Windows 安装版从开始
   菜单启动，便携版双击 `viberwhisper-app.exe`，均不会打开命令行窗口
2. 将光标定位到任意文本输入框（浏览器、编辑器、聊天框等）
3. 左键点击状态图标开始录音（五柱声波变红），再次点击停止
4. 也可以**按住 F8**开始录音，松开后自动转录并输入文字（Hold 模式）
5. 或按一下 **F9** 开始录音，再按一下停止（Toggle 模式）
6. 复制历史：右键点击托盘图标，选择「最近识别」下的任意一条；菜单摘要不会改变复制的完整原文
7. 退出：右键点击托盘图标选择「退出」，或按 **Ctrl+C**

> macOS 需要辅助功能授权才能完成文字输入。未授权时输入会明确失败且不会改动剪贴板。原生应用没有焦点输入框或焦点位于可识别的密码框时也会拒绝输入。Chromium 浏览器不依赖网页辅助功能树，而是使用原生剪贴板与 Cmd+V；转写文本会保留在剪贴板中，自动粘贴未生效时可手动粘贴。浏览器隐藏网页辅助功能树时，系统无法区分普通网页输入框和密码框；此时行为等同用户手动执行 Cmd+V。

## CLI 命令

```bash
# 启动录音监听（默认，无子命令）
viberwhisper

# 查看所有配置
viberwhisper config list

# 显示配置路径 / 检查当前 API 运行配置
viberwhisper config path
viberwhisper config check

# 查看单个配置项
viberwhisper config get <key>

# 修改配置项
viberwhisper config set <key> <value>

# 离线转写 WAV 文件
viberwhisper convert input.wav
viberwhisper convert input.wav --output output.txt

# 采集 STT prompt 测试样本（使用现有托盘与 Hold/Toggle 热键）
viberwhisper prompt-lab record --dataset /path/to/my-stt-dataset

# 查看、校正并验证样本
viberwhisper prompt-lab sample list --dataset /path/to/my-stt-dataset --status pending
viberwhisper prompt-lab sample show --dataset /path/to/my-stt-dataset <sample-id>
viberwhisper prompt-lab sample correct --dataset /path/to/my-stt-dataset <sample-id> \
  --reference-file expected.txt --proper-nouns-file proper-nouns.json
viberwhisper prompt-lab dataset validate --dataset /path/to/my-stt-dataset

# 用临时候选 prompt 全量重跑所有 ready 历史录音
viberwhisper prompt-lab evaluate --dataset /path/to/my-stt-dataset \
  --prompt-file candidate.txt \
  --max-wer-percent 8 --min-llm-score 95 --min-proper-noun-percent 98

# 将编码代理给出的逐样本语义复核应用到同一份报告
viberwhisper prompt-lab report apply-review \
  --report /path/to/my-stt-dataset/runs/<run-id>.json \
  --review /path/to/<run-id>.agent-review.json
```

`prompt-lab record` 只保存当前 STT API 返回的原始结果，不执行 LLM 后处理、不写普通
`history.jsonl`，也不向当前输入框注入文字。每次完成的录音在数据集的 `audio/` 下保存一个完整
WAV，并在 `samples/` 下保存同 ID 的 JSON；初始识别不是标准答案，必须通过 `sample correct`
写入人工参考文本后才会成为 `ready`。专有名词文件是由 `canonical`、`accepted`、
`case_sensitive` 和 `expected_occurrences` 组成的 JSON 数组；省略该文件表示该样本没有专有名词。

数据集中的录音、初始识别和人工参考文本均为本机明文。录音会发送给当前配置的 STT 服务；在后续
调试任务中，编码代理也会读取参考文本与候选结果。请只采集愿意通过这些路径处理的内容。首个版本
要求同一数据集同一时间只由一个 `prompt-lab` 进程使用，不提供锁、原子写入或中断自动恢复；
`dataset validate` 会报告损坏 JSON、截断 WAV、摘要不匹配和无侧车录音，需人工清理或重录。

### STT Prompt 回归调试

`prompt-lab evaluate` 每次都会按稳定 ID 顺序重新读取并转写全部 `ready` WAV，不复用旧 STT
结果，也不把人工参考文本发给 STT。`--prompt-file` 的原文只覆盖本次进程内的
`transcription.prompt`，不会修改 `config.json`；省略它会使用当前配置作为基线，`--no-prompt`
则明确不发送 multipart prompt。`--compare-to` 可指向一份已完成且数据集、后端与评分版本兼容的
旧报告；不兼容会在任何 STT 请求前失败。报告默认直接写入数据集的 `runs/`，只生成 JSON，
不生成 Markdown 或网页报告。

三项指标各自独立，不合成加权总分：

- **词错误率（WER）**：NFKC 规范化后，中文使用固定 Jieba 词典且关闭 HMM，其他文本使用
  Unicode 词边界；数据集值是替换、删除、插入总数除以参考词总数的微平均，可超过 100%。
- **语义差异评分**：编码代理只依据每条人工参考文本和本次 STT 结果，按固定 rubric 给出
  `0..=100` 整数、简短原因和结构化差异；程序不配置或调用 Judge API/model。
- **专有名词准确率**：按 canonical/accepted 形式、大小写策略、词边界和期望出现次数计算匹配数
  的微平均。整个 ready 集必须至少标注一次专有名词，否则回归会拒绝启动。

首次评估成功后，规范报告状态为 `awaiting_agent_review`，此时只有 WER 和专有名词指标，不能判定
通过。编码代理读取该 JSON，生成覆盖全部样本的 versioned review JSON，再执行 `apply-review`；
程序验证 run ID、rubric、完整样本覆盖、`0..=100` 分数及差异结构后，直接改写同一份报告，计算
语义均分和三个门禁。只有 `WER <= max`、`LLM >= min`、`专有名词 >= min` 同时成立时
`meets_targets` 才为 `true`。若未通过，编码代理根据逐条差异修改候选 prompt 并再次全量回归，
直到达到目标或判断 prompt 已无法继续改善。单条 STT 失败不会跳过后续样本，但报告会标记为
`incomplete`、命令返回非零，且该报告不能接受复核或作为比较基线。

## 配置说明

CLI 只接受下表中的 canonical dotted key；`hotkey`、`model`、`local_mode` 等旧别名不再支持。

### 输入与音频

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `input.hold_hotkey` | 字符串 | `F8` | 按住录音的具名单键；空字符串可禁用 |
| `input.toggle_hotkey` | 字符串 | `F9` | 切换录音的具名单键；空字符串可禁用 |
| `audio.input_device` | 字符串或 `null` | `null` | 输入设备的精确名称；`null` 使用系统默认设备 |
| `audio.mic_gain` | 数字 | `1.0` | 麦克风增益倍数 |

指定设备在录音时不可用会明确报错，不会静默切换到其他麦克风。cpal 只提供设备显示名称；若系统返回重名
设备，将使用第一个精确匹配项。可重新运行 `viberwhisper setup` 选择其他设备。

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

macOS 的 Chromium/兜底粘贴会在极短的内部注入窗口暂停 ViberWhisper 自己的热键映射，避免合成的 `LEFTMETA`/`V`
触发录音；这不是系统级按键拦截，事件仍会正常送达当前应用。注入结束后热键按下状态会重置。

当前 `rdev 0.5.3` 后端还有以下明确限制，`config check` 会拒绝对应名称：

- macOS：`CAPSLOCK`、`RIGHTCTRL`、`DELETE`、`INSERT`、`HOME`、`END`、`PAGEUP`、
  `PAGEDOWN`、`NUMLOCK`、`SCROLLLOCK`、`PRINTSCREEN`、`PAUSE`、`INTLBACKSLASH` 和全部
  `NUMPAD*` 名称。
- Windows：`RIGHTMETA`、`FUNCTION`、`NUMPADENTER`；数字键盘的报告方式还会受到 Num Lock 影响。

内部可靠性策略不作为用户配置：实时与离线音频统一按 30 秒或 23 MiB 的较小限制分片；每个 STT 请求
最多等待 12 秒，网络错误或 HTTP 5xx 最多重试一次；停止录音后的 session 收敛窗口固定为 30 秒。

### 转写 API

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `transcription.language` | 字符串/null | `zh` | 语言代码；`null` 为自动检测 |
| `transcription.prompt` | 字符串/null | 中文提示词 | 转写提示词 |
| `transcription.temperature` | 数字 | `0` | 转写温度 |
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

### 切换转写服务

修改 API endpoint、model 和环境密钥即可切换兼容接口：

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
```

维护者打包、版本校验、tag 发布、产物验证和失败恢复流程见
[`docs/releasing.md`](docs/releasing.md)。

## 许可证

MIT
