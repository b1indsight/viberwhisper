# 配置指南

[返回使用指南](../README.md) · [命令行参考](cli.md) · [Realtime 与语境记忆](dictation.md)

## 配置文件与首次设置

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

也可将 [`config.example.json`](../config.example.json) 复制到上述路径，或使用 `config set` 修改普通字段。
已有文件必须包含 `schema_version: 3` 和当前结构；`audio.input_device` 缺失时使用系统默认设备，`dictation` 及其可选字段缺失时使用内置默认值。
必需字段缺失、未知字段、损坏 JSON 或 schema 版本错误会进入恢复向导。已退役的 `chunking`、`session`
和 `inference.api.provider` 均视为未知字段。由 v2 升级时，把 `schema_version` 改为 `3`，并删除
`inference.active` 与整个 `inference.local` 对象；原 Local 用户还需要填写可用的 API endpoint 和模型。

API 密钥优先从环境变量读取，其次才读取配置文件中的对应 secret 字段：

```bash
export TRANSCRIPTION_API_KEY=your_api_key_here
export POST_PROCESS_API_KEY=your_key_here
```

Windows PowerShell 对应写法：

```powershell
$env:TRANSCRIPTION_API_KEY = "your_api_key_here"
$env:POST_PROCESS_API_KEY = "your_key_here"
```

环境变量中的密钥只参与运行时解析，绝不会写回磁盘。`config get/list` 只显示密钥来源状态，不显示值；
secret key 也不能通过 `config set` 修改。向导密码框输入的密钥可以保存，但会在本机配置文件中以明文存在，
保存前会再次提示。

## 修改方式

- `viberwhisper setup`：交互设置 API、热键和麦克风，并验证录音。
- `viberwhisper config set <key> <value>`：修改下表中的普通字段。只做类型解析和保存，修改后用 `viberwhisper config check` 检查组合配置。
- 手动编辑 `viberwhisper config path` 指向的文件：包括仅文件支持的整个 `dictation` 对象。`config list/get/set` 不提供 `dictation.*` 键。

修改配置或环境变量后重新启动桌面应用。图形入口需要从它实际继承的环境中取得变量；首次使用可直接在向导密码框中输入密钥。

CLI 只接受下表中的 canonical dotted key；`hotkey`、`model`、`local_mode` 等旧别名不再支持。
可空字段使用字面值 `null` 清空，例如 `viberwhisper config set audio.input_device null`。

## 输入与音频

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `input.hold_hotkey` | 字符串 | `F8` | 按住录音的具名单键；空字符串可禁用 |
| `input.toggle_hotkey` | 字符串 | `F9` | 切换录音的具名单键；空字符串可禁用 |
| `audio.input_device` | 字符串或 `null` | `null` | 输入设备的精确名称；`null` 使用系统默认设备 |
| `audio.mic_gain` | 数字 | `1.0` | 麦克风增益倍数 |

[配置示例](../config.example.json) 将增益设为 `3.0`，可按麦克风情况调整；上表列出内置默认值。

指定设备在录音时不可用会明确报错，不会静默切换到其他麦克风。cpal 只提供设备显示名称；若系统返回重名
设备，将使用第一个精确匹配项。可重新运行 `viberwhisper setup` 选择其他设备。

## 单键热键名称

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

## 音频与请求策略

HTTP 识别路径的内部可靠性策略不作为用户配置：实时与离线音频统一按 30 秒或 23 MiB 的较小限制分片；每个 STT 请求
最多等待 12 秒，网络错误或 HTTP 5xx 最多重试一次；停止录音后的 session 收敛窗口固定为 30 秒。

## 转写 API

默认使用 HTTP STT；也可启用 [Realtime 听写](dictation.md)，替代 HTTP STT 和独立 LLM 后处理。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `transcription.language` | 字符串/null | `zh` | 语言代码；`null` 为自动检测 |
| `transcription.prompt` | 字符串/null | 中文提示词 | 转写提示词 |
| `transcription.temperature` | 数字 | `0` | 转写温度 |
| `inference.api.transcription.api_url` | URL | `https://api.groq.com/openai/v1/audio/transcriptions` | OpenAI-compatible multipart 地址 |
| `inference.api.transcription.model` | 字符串 | `whisper-large-v3-turbo` | 转写模型 |
| `inference.api.transcription.api_key` | secret | 无 | 只读状态；环境变量为 `TRANSCRIPTION_API_KEY` |

上述请求参数用于 HTTP STT。Realtime 使用独立提示词，`transcription.language` 只决定分段拼接，
不作为服务端语言约束；Realtime 采样参数由服务端控制，详见[Realtime 指南](dictation.md)。

## 切换转写服务

修改 API endpoint、model 和环境密钥即可切换兼容接口：

```bash
viberwhisper config set inference.api.transcription.api_url https://api.openai.com/v1/audio/transcriptions
viberwhisper config set inference.api.transcription.model whisper-1
```

同时把 `TRANSCRIPTION_API_KEY` 设为所选服务的密钥；模型名称必须受该 endpoint 支持。

## LLM 后处理配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `post_process.enabled` | 布尔 | `false` | 是否启用 LLM 后处理 |
| `post_process.preheat_enabled` | 布尔 | `true` | 是否使用后台预热请求模式 |
| `post_process.prompt` | 字符串/null | 内置默认 | 自定义整理规则；结构化输出协议始终保留 |
| `post_process.temperature` | 数字 | `0.0` | 后处理温度 |
| `inference.api.post_process.api_url` | URL/null | 无 | OpenAI-compatible chat completions 地址 |
| `inference.api.post_process.model` | 字符串/null | 无 | LLM 模型名 |
| `inference.api.post_process.api_key` | secret | 无 | 只读状态；环境变量为 `POST_PROCESS_API_KEY` |

> **注意**：`config.json` 已在 `.gitignore` 中排除，避免误提交真实密钥。程序不会把环境变量中的密钥写入磁盘；手工写在 `config.json` 中的密钥会在更新其他配置项时原样保留。
> 后处理当前固定使用 OpenAI-compatible chat completions 请求格式。

## LLM 整理与确认后学习

启用后，转写结果会在输出前经过 LLM 整理，补标点、去掉明确无意义的语气词及卡顿，并按
最终说法合并明确改口。例如“订七张票，哦不，订八张票，都选靠窗的”整理为“订8张票，
都选靠窗的”；独立否定、重复强调、原有措辞和未完成的句子保留，不凭常识猜测识别错误。
默认语言、数字、改口和保真规则复用 [Realtime 提示词](../assets/dictation-prompt.default.json)，
普通后处理只依据 STT 文本，不能重新听音频。

普通桌面路径在同一次推理中提取语境及带 `quote` 的候选，复用[语境记忆与确认后学习](dictation.md#确认后学习)，
不再增加一次推理。`dictation.enabled=false` 选择普通识别；`dictation.memory` 控制两条
桌面路径的记忆，使用同一个 `dictation-memory.json`。关闭后处理不会创建记忆写入者。

**隐私边界：** 记忆启用时，有界历史短句、术语和已确认纠错会发给配置的后处理服务，
该服务可能在云端。设 `dictation.memory.enabled=false` 可禁用个人快照、学习和本地纠错。
`convert` 与 setup 仅使用整理规则，不读取、上传或更新个人记忆；原始采集保持原样。

接口继续使用 OpenAI-compatible chat completions，`stream=false`。模型现在必须返回
`memory_candidates`、`context`、`text` 三个字段的 JSON；程序只显示/交付 `text`。
`post_process.prompt` 替换默认整理规则，程序仍附加任务边界、JSON 格式及候选定位说明。
旧自定义提示词中的“只输出纯文本”等要求应移除，以免与协议冲突。整个 JSON 无效、正文为空
或请求失败时回退原始 STT；个别候选无效时只跳过该候选，不丢弃有效正文。

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

- **预热模式**（默认，`post_process.preheat_enabled = true`）：会话在收到文本后启动后台请求，只使用最后一代结果；失败后以完整文本重试一次
- **保守模式**（`post_process.preheat_enabled = false`）：录音全部结束后一次性发送完整文本

当前桌面在 STT 汇总完成后传入一次完整文本，两种模式都在此时启动后处理；预热模式目前
不代表录音期间已提前完成推理。两种模式使用相同的结构化校验和确认学习边界。

后处理失败时自动降级为输出原始转写文本，不会导致整次录音失败。
