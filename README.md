# ViberWhisper

一个基于 Rust 实现的语音转文字输入工具，使用热键录音，停止后预览识别结果，再按回车确认输入。

灵感来源于 [Typeless](https://typeless.ai/)，默认使用 [Groq Whisper API](https://console.groq.com) 进行语音识别，也可通过配置切换到任何兼容 OpenAI multipart 格式的转写接口。

## 功能特性

- **全局热键录音**：默认按住 F8 开始、松开停止（Hold 模式），按一下 F9 开始、再按一下停止（Toggle 模式）；两者均可改为具名单键，包括单独的右 Alt/Option
- **AI 语音识别**：通过可配置的 HTTP 转写接口将语音转为文字（默认 Groq Whisper）
- **静音幻听抑制**：在 STT 上传前本地过滤没有可听窗口的近静音分片，整段静音不会触发后处理、历史记录或文字输入
- **长录音自动分片**：超过时长/大小限制的录音自动切分，后台并行转写，结果智能合并
- **LLM 文本后处理**：可选的 LLM 后处理层，自动补标点、去语气词、清理中断与重复
- **气泡编辑与确认输入**：识别结果显示在无标题栏、无按钮的原生气泡中，可直接修改，回车输入、Esc 取消；确认后恢复目标窗口，支持中文和 emoji
- **Realtime 流式预览**：收到正文 delta 后在同一气泡中追加；最终结果有纠正时，从首个差异词替换尾部。生成期间只读、可复制、Esc 取消，完成校验后开放编辑和回车确认
- **本地识别历史**：确认的文本以 JSONL 追加到应用数据目录，右键菜单直接显示最近 5 条，点击即可复制完整原文；取消的结果不写入历史
- **状态栏录音控制**：左键点击五柱 V 形声波图标即可开始或停止录音，右键打开识别历史和退出菜单
- **灵活配置**：支持自定义热键、模型、语言、API 地址、麦克风增益等
- **自动清理**：自动保留最新 10 条录音，旧文件自动删除

## 使用指南

### 系统要求

- **操作系统**：macOS 或 Windows
  - macOS：原生控件优先通过辅助功能 API 写入当前选择；Chromium 浏览器和不支持该能力的控件使用原生剪贴板与 Cmd+V。需在「系统设置 → 隐私与安全性 → 辅助功能」中授权正在运行的终端或 ViberWhisper
  - Windows：使用 SendInput API，无需额外权限
- 安装发布包不需要 Rust；如需从源码构建，请参阅[贡献指南](CONTRIBUTING.md)

### 下载安装包

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

### 快速开始

#### 1. 获取 API 密钥

默认使用 Groq：前往 [Groq Console](https://console.groq.com) 注册并获取 API 密钥。

#### 2. 配置

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

监听模式会在确认输入时，把文本框编辑后的非空最终文本追加到配置文件同目录下的 `history.jsonl`。
历史与文字注入使用同一份确认文本，保留大小写、换行和 Unicode 内容。
确认前和取消时不写入；确认后投递失败的重试复用已有记录，不重复追加：

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/history.jsonl`
- Windows：`%APPDATA%\ViberWhisper\history.jsonl`

每行是一个按时间先后排列的独立 JSON 记录，例如
`{"text":"识别结果","metadata":{"created_at_unix_ms":1786612345678}}`。启动和追加前只校验
最后一行；如果 JSON 或元数据无效，就删除该行，再继续使用此前记录。文件限制为 5 MiB，
接近上限时淘汰最旧的完整记录，不截断正文。保存失败不会阻止确认后的输入。该文件是本机明文，
不包含音频、API 密钥或目标应用信息。

右键菜单直接显示最新 5 条；长文本在 40 个 Unicode 字符后缩略，但点击时复制的始终是完整原文。
复制动作不会自动粘贴，也不需要 macOS 辅助功能权限。

#### 3. 启动并使用

1. 启动程序，系统托盘/状态栏会出现五柱声波图标；Windows 安装版从开始
   菜单启动，便携版双击 `viberwhisper-app.exe`，均不会打开命令行窗口
2. 将光标定位到任意文本输入框（浏览器、编辑器、聊天框等）
3. 左键点击状态图标开始录音（五柱声波变红），再次点击停止
4. 也可以**按住 F8**开始录音，松开后停止并转录（Hold 模式）
5. 或按一下 **F9** 开始录音，再按一下停止（Toggle 模式）
6. 识别完成后可在气泡中直接修改文字，按 **Enter** 输入修改后的内容，按 **Esc** 取消；**Shift+Enter** 换行，中文输入法选词时的回车先确认候选词
7. 复制历史：右键点击托盘图标，选择「最近识别」下的任意一条；菜单摘要不会改变复制的完整原文
8. 退出：右键点击托盘图标选择「退出」，或按 **Ctrl+C**

预览显示时会尝试取得键盘焦点；系统拒绝切换时请点击预览后按回车。确认键释放后才恢复
预览出现前的目标并写入文字，第一次回车不会作为换行或发送键交给目标应用。
气泡获得焦点后需要重新按下回车；之前已按住的回车不会确认。macOS 支持
**Cmd+A/C/X/V** 全选、复制、剪切和粘贴。
短句气泡按文字实际宽度收窄，达到最大宽度后换行并随内容增减高度；文字左对齐，输入区垂直居中。
达到最大高度后在框内滚动。气泡优先出现在原输入光标下方；下方空间不足时移到上方，
靠近屏幕边缘时自动避让。应用未提供光标坐标时尝试输入框附近，再回退到屏幕中央。
macOS 可拖动气泡空白区域，拖动后沿用手动位置。
待确认时再次请求录音会回到当前预览，需要先输入或取消。目标关闭、焦点无法恢复或输入
失败时保留预览，可重试或复制后手动粘贴；如果平台报告文字可能已部分输入，应先核对目标内容。

Realtime 模式会逐步显示服务返回的正文，隐藏 JSON 和记忆元数据；生成期间不可编辑或确认，
Esc 会取消整次听写（仍在录音时也会停录）。最终结果通过校验和本地纠错后，保留相同前缀，
从最早出现差异的词开始替换旧尾部，然后才允许编辑和确认。确认前不会向目标应用输入。
录音期间的预览不抢焦点，停录后才尝试聚焦。请求时机仍是每个 30 秒分段结束或停录，
因此短录音在停录后开始显示识别流；服务不发送 delta 时一次显示完整结果。

> macOS 需要辅助功能授权才能完成文字输入。未授权时输入会明确失败且不会改动剪贴板。原生应用没有焦点输入框或焦点位于可识别的密码框时也会拒绝输入。Chromium 浏览器不依赖网页辅助功能树，而是使用原生剪贴板与 Cmd+V；转写文本会保留在剪贴板中，自动粘贴未生效时可手动粘贴。浏览器隐藏网页辅助功能树时，系统无法区分普通网页输入框和密码框；此时行为等同用户手动执行 Cmd+V。

macOS 启动监听时若缺少辅助功能权限，会显示授权说明和「打开设置」按钮；启动后首次因权限失效而输入失败时也会提示。同一次启动最多显示一次，选择「稍后」仍可录音，识别结果可从待确认预览复制后手动粘贴。若更新后已有授权失效，请在「系统设置 → 隐私与安全性 → 辅助功能」中移除旧条目，重新添加当前应用，再退出并重启。离线转写和配置命令不会显示此提示。

### CLI 命令

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
```

### 配置说明

CLI 只接受下表中的 canonical dotted key；`hotkey`、`model`、`local_mode` 等旧别名不再支持。

#### 输入与音频

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

原 HTTP 识别路径的内部可靠性策略不作为用户配置：实时与离线音频统一按 30 秒或 23 MiB 的较小限制分片；每个 STT 请求
最多等待 12 秒，网络错误或 HTTP 5xx 最多重试一次；停止录音后的 session 收敛窗口固定为 30 秒。

#### 转写 API

也可启用下述 Realtime 听写路径，替代 HTTP STT 和独立 LLM 后处理。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `transcription.language` | 字符串/null | `zh` | 语言代码；`null` 为自动检测 |
| `transcription.prompt` | 字符串/null | 中文提示词 | 转写提示词 |
| `transcription.temperature` | 数字 | `0` | 转写温度 |
| `inference.api.transcription.api_url` | URL | Groq Whisper URL | OpenAI-compatible multipart 地址 |
| `inference.api.transcription.model` | 字符串 | `whisper-large-v3-turbo` | 转写模型 |
| `inference.api.transcription.api_key` | secret | 无 | 只读状态；环境变量为 `TRANSCRIPTION_API_KEY` |

#### Realtime 听写与语境记忆

仅通过配置文件设置，修改后重启生效；不新增设置窗口、托盘配置或专用管理命令。
升级后，桌面应用成功验证现有配置时会自动补齐并保存缺失的 Realtime 字段，包括 `memory` 的子字段；已有开关、地址、模型和自定义提示词保持原样。缺少整个 `dictation` 时补入默认配置，`enabled` 仍为 `false`。完整配置不会重复写入，`config get/list/check` 等检查命令不会触发补齐。
缺失的 `prompt` 和 `prompt_components` 保存为 `null`，继续使用内置默认提示词；不会将默认提示词正文写入配置。`prompt: ""` 仍表示不使用提示词。
在 `viberwhisper config path` 显示的文件中加入以下片段（完整示例见 `config.example.json`）：

```json
"dictation": {
  "enabled": true,
  "url": "ws://127.0.0.1:8080/v1/realtime",
  "model": "mlx-community/gemma-4-e4b-it-4bit",
  "api_key_env": "REALTIME_API_KEY",
  "prompt": null,
  "prompt_components": null,
  "memory": {
    "enabled": true,
    "long_term": [
      {"context": "Rust 项目", "kind": "proper_noun", "text": "ViberWhisper"},
      {"context": "Rust 项目", "kind": "phrase", "text": "提交一个草稿 PR"}
    ]
  }
}
```

`enabled` 缺省为 `false`，旧配置继续使用原路径。启用后忽略旧 STT 和后处理的连接设置，
不再发起第二次 LLM 清理请求。鉴权读取 `api_key_env` 指定的环境变量；变量未设置则不发
Authorization。地址使用 `ws://` 或 `wss://`，不要在 URL 中放密钥、查询参数或片段。
默认提示词正文维护在 [`assets/dictation-prompt.default.json`](assets/dictation-prompt.default.json)，
Rust 只负责读取和组合。要独立修改规则，将 `config.example.json` 中完整的
`dictation.prompt_components` 对象复制到本机配置，保持 `prompt` 为 `null`，然后编辑对应项：

| 组件 | 用途 |
|------|------|
| `task` | 当前音频的任务与忠实转写原则 |
| `language` | 中国大陆简体中文（zh-CN）和外语规则 |
| `numbers` | 数量、金额、日期、时间的数字格式 |
| `cleanup` | 填充词、自我修正、原措辞和语气 |
| `output_format` | JSON 字段、转义和静音输出 |
| `context` | 话题标签与未知语境 |
| `memory` | 按正文顺序提取专名及长度限制 |

组件按表中顺序组合；首尾空白移除，空白项跳过，其余用换行连接。对象必须包含七个
字符串字段，不能只提供其中一项。修改本机组件后重启即可生效，无需编译。
未提供组件时使用应用内嵌的 JSON 默认资源；发布新的内置默认值仍需更新应用。

`prompt` 仍支持完整任务提示词覆盖：字符串完整替换所有组件，空字符串清空指令。
它与 `prompt_components` 不能同时设置；两者为 `null` 时使用默认组件。
候选提示仍须让模型返回约定的 JSON 字段，组合全文最多 4096 个字符。
`memory.enabled=false` 同时关闭长短期记忆和纠错表的读取、学习与应用。

模型在同一次音频响应中输出整理后的文字、话题标签和专名字符串数组，例如：

```json
{
  "context": "编程工具",
  "memory_candidates": ["Codex", "Redis", "Codex"],
  "text": "用 Codex 连接 Redis，再让 Codex 检查。"
}
```

`context` 始终是字符串，沿用已有话题时原样返回其标签；无法判断时填 `"未知语境"`，
候选必须为 `[]`。未知时不应用按话题纠错，也不学习话题、术语或纠错规则；有效正文和
全局转写历史不受影响。`"未知语境"` 是保留值，人工词表中的同名话题必须先改名。
客户端只交付经过校验的最终文字，JSON 和记忆不会输入到活动应用。候选定位只验证文字
支持，不能证明识别准确或模型的话题判断正确。

短期记忆在配置文件同目录的 `dictation-memory.json` 保存，按最近独立观察滚动 24 小时过期，
持续运行时也会过期，重启不会延长期限。长期条目直接在 `memory.long_term` 中编辑，不自动过期，也不被模型
改写。`kind` 为 `proper_noun` 或 `phrase`，每条最多 80 字，语境标签最多 64 字，长期最多
512 条。模型再次返回的已有词也会更新短期观察和支持次数。
单轮最多提供 4 个语境、每个语境最多 16 个长期词条、总计 6000 个序列化字符；
先按配置顺序选择长期语境与词条，再填充近期语境和历史。超出单轮预算的配置仍保留。
桌面 Realtime 与启用 LLM 后处理的普通 HTTP 路径共用这份记忆，在用户确认且目标接受文字后
才提交本次学习。取消、清空预览、投递失败、识别或后处理失败不会学习；失败后修改再重试时，
以最后成功提交的文字为依据更新候选和语境历史。

手动纠正专有词时，客户端结合模型原文、预览文字、最终编辑文本的差异和模型候选，
记录同一语境内的“误词 → 正确词”。例如把“维伯耳语”改成“ViberWhisper”，以后在同一
语境再次识别出该误词，且本次模型仍将完整误词提取为专有词候选，预览才自动修正。
模型按正文出现顺序逐次列出完整专名，同名重复出现也重复列出，最多前 8 项，每项最多
80 字，不再返回 `kind/text/quote` 对象或自动提取固定短语。客户端使用 NFKC、大小写归一化
和 Unicode 完整词边界定位。用于纠错或学习时，每个名称提供的次数必须等于正文中可定位
的次数；例如正文有两次“张山”而数组只列一次时，该名称整组跳过，不猜选中了哪次，也不
全文替换。正文同名出现 9 次、数组按上限列出前 8 次仍符合输出协议，但这个不完整的名称组
不会用于纠错或学习。
无法定位或格式错误的候选不连带丢弃其他有效名称；超过 8 项时放弃整组候选，正文保留。
旧自定义 `dictation.prompt` / `prompt_components` 若仍要求语境对象或候选对象，需要手动
同步新协议。旧格式元数据会降级，合法正文保留；应用不自动改写个人提示词或存量记忆。
修改后的词语由本地对齐得到，没有再次调用模型提取。首版只学习显示候选范围内的
替换、删除和严格位于候选内部的插入；跨过候选或分段边界的修改不学习。词语对齐能
确定文字差异，无法单独证明编辑的语义意图；含糊范围跳过学习，最终输入和全局转写历史照常保存。
未提取为专有词的正文不做本地替换；存量短语记忆仍可读取，人工短语词表继续保护原文。
用户将自动修正改回原词，
仅在修改仍属于显示候选内部时撤销规则；补回候选首尾的字词或跨分段还原暂不学习。
改成第三个词则更新原误词的规则，包括增字或删字，例如 `Codex → Code`。
单字修改按候选整词学习，例如“张山 → 张珊”；
模型同时提取“张山”和“山”时，只处理完整的“张山”。交叉重叠且没有唯一完整词条的
候选跳过纠错。无可靠候选、语境不明、含糊改写、纯格式或数字变化不生成永久替换。
删除已纠正的词只删除本次内容，保留已有拼写规则；仅添加标点也不更新人工纠错证据。
改词时新加的句末标点不写入词语替换表，模型已提取的名称标点和名称中的符号保留。
同次录音、同一语境中，同一个误词出现不同的最终拼写时停止应用对应规则，
未修改的出现位置也参与这项判断。配置词保护范围内的出现位置不生成片段规则，也不参与
独立词条的拼写冲突判断。只在完整名字旁边加词，例如 `Codex → Call Codex`，不生成
改名规则；同时改名字和添加词语、范围含糊时跳过学习。候选内部增字可学习，例如
“张珊 → 张小珊”；首尾补字暂不学习，例如“张珊 → 张珊珊”和 `Codex → XCodex`。
即使相邻候选因删除等修改无法对齐，也不将边界新增内容交给另一侧。
普通 HTTP STT 由同一次 LLM 后处理补充语境与候选，再使用相同的纠错逻辑；关闭后处理或
缺少有效语境/候选时，只保存编辑后的历史，不推测纠错规则。

人工纠错存于同一个 `dictation-memory.json` 的 `corrections`，长期保留，最多 512 条，
超额淘汰最早人工纠正的规则。匹配按话题隔离并检查词边界，替换只执行一遍；冲突和循环
规则不自动应用，手工配置的长期词完整范围受保护，例如 `York → Yolk` 不会改动配置中的
`New York`，仍可修正同一段中的独立 `York`。自动替换本身不会再次计为人工纠正。
运行数据保持 schema 2，兼容读取 schema 1；旧模型观察仍按 24 小时到期。

清理前先退出客户端：将 `observations` 改为空数组可清空短期记忆，删除 `corrections`
中的对应条目可移除某条纠错；删除整个文件会同时清空两者。`memory.long_term` 配置
不受影响。回退到只支持 schema 1 的旧程序前，请备份并移开该运行数据文件。

长录音按 30 秒预算分段，同连接可持续多轮且累计超过 60 秒。完整响应校验后交付，明确重复
口述会保留。网络、录音或清理失败时保留已验证分段，不更新本次记忆；主动取消不交付文字。
断线不自动重放已提交音频，也不切换云端。

服务需要支持[通用 Realtime 协议及生命周期要求](docs/architecture/dictation.md#audio-and-lifecycle)。
当前局域网 Gemma 实现尚有兼容性缺口，真实质量和延迟待服务增强后验证；
实测阻塞及命令见[实现记录](docs/plan/52-contextual-streaming-dictation.md#待完成)。

`convert` 使用所选 Realtime 路径，但默认禁用个人记忆。`prompt-lab evaluate` 关闭个人记忆输入，
用于迭代完整任务提示词：不加参数使用配置/内置默认值，`--prompt-file` 完整替换提示词，
`--no-prompt` 清空指令；这两种临时全文覆盖都会忽略配置组件，且不会写回配置。
候选采用后可编辑 `dictation.prompt_components`，或清空组件并写入整段 `dictation.prompt`，
重启客户端生效。
报告记录实际使用的提示词；缺少约定 JSON 输出的结果会按识别失败记录。
Realtime 多音频块评测、实际交付与离线转换统一使用 `transcription.language` 的拼接规则：
中文不添加分隔符，其他语言或未指定时添加空格，保留重复口述。
评测沿用原 JSON 报告、WER、专有词指标和语义审阅流程。
需要验证语境和记忆时，添加 `--realtime-cases cases.json`，用显式种子和模拟时钟顺序
播放 WAV。每个 ready 样本须恰好出现一次；场景之间独立，步骤之间学习记忆。
报告记录实际发送的快照、原始结构化输出、记忆变化和本地断言，并把断言纳入最终 gate。
场景模式同样不接触个人记忆文件；场景历史会保存在评估报告中。
格式和用法见[场景示例](docs/architecture/prompt-lab.md#realtime-context-and-memory-scenarios)。
原始录音采集 `prompt-lab record` 仍使用原 HTTP STT，避免把整理结果伪装成 raw-STT 样本。

#### LLM 后处理

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

#### 切换转写服务

修改 API endpoint、model 和环境密钥即可切换兼容接口：

```bash
./viberwhisper config set inference.api.transcription.api_url https://api.openai.com/v1/audio/transcriptions
./viberwhisper config set inference.api.transcription.model whisper-1
```

### LLM 后处理

启用后，转写结果会在输出前经过 LLM 整理，补标点、去掉明确无意义的语气词及卡顿，并按
最终说法合并明确改口。例如“订七张票，哦不，订八张票，都选靠窗的”整理为“订8张票，
都选靠窗的”；独立否定、重复强调、原有措辞和未完成的句子保留，不凭常识猜测识别错误。
默认语言、数字、改口和保真规则复用 [Realtime 提示词](assets/dictation-prompt.default.json)，
普通后处理只依据 STT 文本，不能重新听音频。

普通桌面路径在同一次推理中提取话题字符串及专名字符串数组，复用上文的专有词纠错与确认学习，
不再增加一次推理。`dictation.enabled=false` 选择普通识别；`dictation.memory` 控制两条
桌面路径的记忆，使用同一个 `dictation-memory.json`。关闭后处理不会创建记忆写入者。

**隐私边界：** 记忆启用时，有界历史短句、术语和已确认纠错会发给配置的后处理服务，
该服务可能在云端。设 `dictation.memory.enabled=false` 可禁用个人快照、学习和本地纠错。
`convert` 与 setup 仅使用整理规则，不读取、上传或更新个人记忆；原始采集保持原样。

接口继续使用 OpenAI-compatible chat completions，`stream=false`。模型现在必须返回
`context`、`memory_candidates`、`text` 三个字段的 JSON；程序只显示/交付 `text`。
`post_process.prompt` 替换默认整理规则，程序仍附加任务边界、JSON 格式及候选定位说明。
旧自定义提示词中的“只输出纯文本”等要求应移除，以免与协议冲突。整个 JSON 无效、正文为空
或请求失败时回退原始 STT；个别候选无效时只跳过该候选，不丢弃有效正文。

#### 启用方法

```bash
# 启用后处理
viberwhisper config set post_process.enabled true

# 配置 LLM API
viberwhisper config set inference.api.post_process.api_url https://api.openai.com/v1/chat/completions
viberwhisper config set inference.api.post_process.model gpt-4o-mini

# 设置 API 密钥（通过环境变量）
export POST_PROCESS_API_KEY=your_key_here
```

#### 两种模式

- **预热模式**（默认，`post_process.preheat_enabled = true`）：会话在收到文本后启动后台请求，只使用最后一代结果；失败后以完整文本重试一次
- **保守模式**（`post_process.preheat_enabled = false`）：录音全部结束后一次性发送，零 token 浪费

当前桌面在 STT 汇总完成后传入一次完整文本，两种模式都在此时启动后处理；预热模式目前
不代表录音期间已提前完成推理。两种模式使用相同的结构化校验和确认学习边界。

后处理失败时自动降级为输出原始转写文本，不会导致整次录音失败。

## 开发与贡献

直接查看原生预览交互，可运行 `cargo run --features integration --bin viberwhisper-preview`。
启动后有 3 秒选择目标输入框，再演示流式正文、最终纠正和可编辑气泡；回车会实际写入该输入框，Esc
取消后退出，不调用 API、不录音、不保存演示历史。加上 `-- --smoke` 会向当前目标实际
投递一段编辑后的测试文字，并验证按住回车期间不确认、释放后只确认一次；请先聚焦
专用测试输入框。正常录音听写继续使用默认程序入口。

macOS 可用 `-- --artifacts .amp/in/artifacts/streaming-preview` 运行不捕获目标、不输入文字的
原生流式检查，并保存正文追加和最终纠正的 PNG；该模式不需要录音、API、个人配置或历史。

README 面向安装、配置和日常使用。源码构建、技术栈、测试要求、跨平台注意事项及 PR 流程见
[贡献指南](CONTRIBUTING.md)；架构、功能计划和维护者文档索引见
[项目文档](docs/README.md)。

## 许可证

MIT
