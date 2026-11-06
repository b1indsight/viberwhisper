# 64 - 识别模式二选一配置与 Windows 高 DPI 窗口

**阶段：** 方案待明确批准；尚未修改产品代码或测试。
**PR：** [#165（docs-only draft）](https://github.com/b1indsight/viberwhisper/pull/165)，批准后在同一个 PR 实现。
**Bookmark：** `feat/complete-setup-windows-dpi`。
**基线：** 最新 fetch 后的 `origin/master`，即 [v0.5.0 发布准备提交 e18be032](https://github.com/b1indsight/viberwhisper/commit/e18be032950c03bc7bf40153e028e12193cccacc)。
**工作区：** 独立 JJ workspace `setup-realtime-dpi-20261106`；不修改源主工作区、发布准备工作区、个人配置或个人记忆。

**当前范围：** 以时间线 [001](#001--2026-11-06--向导收缩为-realtime--普通识别二选一) 为准：
向导主要提供 Realtime / 普通识别二选一及所选路径的必要连接配置，撤回提示词、温度、预热、记忆和其他高级编辑页。
Windows DPI 调查与候选修复保持原范围。下方初始方案保留为历史；不再按其高级配置覆盖承诺实施。

## 1. 初始方案

### 背景、目标与边界

现有向导只收集 HTTP STT 连接、可选 LLM 连接、热键和麦克风。当前版本的 Realtime、共享记忆、
提示词组件等已有严格配置模型，但向导没有编辑入口；麦克风增益、语言、温度、LLM 预热等较早的字段也未覆盖。
本次直接核对完整模型，不能只以有意排除 `dictation.*` 的 CLI 字段目录作为依据。
来源：[配置文档类型](../../src/core/config/document.rs#L13-L25)、[原有各配置节](../../src/core/config/document.rs#L127-L262)、
[DictationConfig](../../src/dictation/config.rs#L64-L200)、[字段目录](../../src/core/config/fields.rs#L93-L166)、
[现有向导](../../src/ui/setup.rs#L381-L530)。

另外，用户报告 Windows 窗口在 4K 显示器上模糊。目前没有 Windows runner 或报告机器的截图、
有效 DPI 上下文和启动路径证据。方案区分源码确认的缺口、候选机制和实机验收，不能将源码分析写成根因已证实。

成功标准：

- macOS 和 Windows 的同一向导可配置下表中的全部用户设置，基础流程简短，高级设置主动展开。
- 保留现有值、未启用路径的配置、自定义提示词、长期词表和已有密钥策略；取消、UI 错误或拒绝最终保存不写盘。
- Windows 设置对话框在创建前已有明确 DPI 策略；自建预览气泡按实际窗口 DPI 重建字体并重新布局。
- 明确分别报告 macOS 运行、Windows 交叉检查、Windows CI 原生测试和真实 4K 视觉验收的结果。

不包括：持久化设置中心、第二个 winit 事件循环、配置版本迁移、改写内置提示词或模型默认值、
新增密钥存储、清除个人运行记忆、增加记忆策略参数、扩展 `config list/get/set` 的 Realtime 目录、
安装 Realtime 服务、修改 v0.5.0 发布或手动触发 release/deployment。

### 完整配置覆盖核对

“高级”是向导中的可选分组，默认保留当前配置；并非要求用户继续手改文件。
未选择的识别路径不询问、不重置；需要编辑它时先选择该路径。关闭某项功能保留它的参数。

| 完整字段 | 当前向导 | 拟议入口与语义 |
|---|---|---|
| `schema_version` | 不编辑 | 维持严格 v3，不提供用户修改入口 |
| `input.hold_hotkey`、`input.toggle_hotkey` | 已有 | 保留当前值、顺序捕获和成对确认/校验；不改变单键约束 |
| `audio.input_device` | 已有 | 保留当前设备；断开或枚举失败时警告，由用户明确选择保留或改选可用设备/系统默认 |
| `audio.mic_gain` | 缺失 | 音频高级设置，有限 f32，默认当前值 |
| `transcription.language` | 缺失 | 共享识别高级设置：保留、指定语言或 `null`；HTTP 请求语言与两条路径的分段拼接共用，Realtime 不把它当服务端语言指令 |
| `transcription.prompt`、`transcription.temperature` | 缺失 | HTTP 高级设置；提示词区分 `null` 与空字符串，温度仅接受有限 f32 |
| `inference.api.transcription.api_url`、`model`、`api_key` | 已有 | HTTP 基础设置；密码框始终为空，空输入保留磁盘密钥，环境密钥仍优先 |
| `post_process.enabled` | 已有 | HTTP 基础设置，确认默认值使用当前开关，修正目前写死为 `false` 的默认选择 |
| `inference.api.post_process.api_url`、`model`、`api_key` | 已有 | 启用整理时配置；未启用时原样保留；继续使用现有可空字段和密钥规则 |
| `post_process.preheat_enabled`、`prompt`、`temperature` | 缺失 | LLM 高级设置，保留现有预热含义、`null` 内置规则与自定义空字符串的差别 |
| `dictation.enabled` | 缺失 | 第一阶段选择 HTTP STT 或 Realtime，默认当前路径 |
| `dictation.url`、`model` | 缺失 | Realtime 基础设置；沿用当前默认值和域校验，不把用户提供的局域网测试目标设成默认值 |
| `dictation.api_key_env` | 缺失 | 编辑环境变量名称，明确不是密钥值；空名称禁用鉴权，只显示变量是否存在，不读取到输入框或写入文件 |
| `dictation.prompt`、`prompt_components` | 缺失 | Realtime 高级设置：保持当前、内置规则、全文覆盖、组件对象四种选择；两种覆盖互斥，全文允许空字符串 |
| `dictation.prompt_components.task`、`language`、`numbers`、`cleanup`、`output_format`、`context`、`memory` | 缺失 | 原生多行 JSON 编辑完整七字段对象；预填当前组件，主动新建组件覆盖时才取内置组件作为起点 |
| `dictation.memory.enabled` | 缺失 | 独立的共享记忆分组，HTTP 和 Realtime 都可配置，默认保留当前值 |
| `dictation.memory.long_term[]` 的 `context`、`kind`、`text` | 缺失 | 原生多行 JSON 数组编辑，保留顺序；`kind` 为 `proper_noun` 或 `phrase`，复用域校验 |
| 运行时 `secrets`、环境密钥、`dictation-memory.json` | 不是可持久化配置字段 | 不提供密钥值回显或运行数据编辑器，不因打开/取消向导而修改它们 |

语言、共享记忆和提示词的实际作用分别依据：
[语言与请求参数](../configuration.md#转写-api)、[两条路径的记忆](../configuration.md#llm-整理与确认后学习)、
[提示词组成及空值约定](../../src/dictation/config.rs#L24-L145)、[长期词表类型与边界](../../src/dictation/config.rs#L149-L207)。
不把已退役的 chunking、session 或 Local 配置重新加入向导。

### 拟议用户流程

1. 沿用首次运行、恢复、跳过和显式 `setup` 的入口规则。
2. 从当前 `dictation.enabled` 选择识别路径；编号选择支持取消。
3. 保留或编辑所选路径的连接。Realtime 不要求先填写无关 HTTP STT/LLM 连接，已有 HTTP 配置完整保留。
4. 可选高级分组：共享语言、HTTP 请求参数/LLM 整理参数或 Realtime 提示词来源。普通用户可全部保留。
5. 独立共享记忆分组：解释隐私边界，再选择保留或编辑开关及长期词表；与 Realtime 是否启用无关。
6. 沿用热键配置，选择或保留麦克风；音频高级分组提供增益。
7. 校验候选配置，可选真实热键/录音验证；摘要显示模式、功能状态、密钥来源和词条数量，不显示密钥、提示词全文或词表内容。
8. 最终确认后仅调用一次 `ConfigStore::save`，继续由现有存储层原子写入。

具体行为示例：已有 Realtime 地址、空全文提示词、自定义长期词表和关闭的记忆，重新运行向导并保留所有选项后，
仍为相同配置。切回 HTTP 只改变用户选定的值，Realtime 的地址、提示词和词表不会被恢复成默认值。

### 保留、取消与安全契约

- 候选配置从读入的 `ConfigDocument` 修改，不从默认文档重建；保持该文档拥有的 `SecretSource`。
  不用默认值覆盖已有开关，也不因为设备暂时不可用就把名称改成 `null`。
- 任何高级分组默认“保留”。可空文字提供“保留 / 使用 null / 编辑文字”，输入字面文字 `null` 不会被误解析成空值。
  `Some("")`、`None` 和内置组件来源继续有明确区别。只查看内置规则不会自动把它们写成个人覆盖。
- 提示词/JSON 不经过当前 `safe_dialog_text` 对数据的 2000 字截断、引号替换或换行移除。
  未编辑的输入保留原始值，包括已有 CRLF；确实编辑多行内容后约定返回 LF，不做 Unicode、引号或空白修饰。
- 复用现有数字解析和域校验，不发明温度、增益的新上下限。JSON 只解析为 `PromptComponents` 或 `Vec<LongTerm>`，
  不提供整份配置 JSON 编辑。组件缺字段/未知字段、互斥来源、组合后超过 4096 个 Unicode 字符、
  词表超限/非法种类/保留语境等按现有约束报告，返回编辑时保留尚未合法的草稿。
- 任一文字编辑器取消、Esc 或关闭均取消整个向导；明确的“保留/完成分组”是单独动作。
  校验失败只影响内存草稿，用户可以修正或取消；原生 UI 错误仍为 `Err`，不能伪装成取消。
- 取消、错误、拒绝最后保存时，已有文件逐字节不变；原文件不存在则不创建。
  显式 setup 不在中途补齐默认值。首次启动的既有“成功加载后补齐缺失字段”行为保持独立，不扩大到向导中途。
- HTTP 密钥仍只通过空白初始密码框录入；空输入保留磁盘值，环境变量覆盖磁盘，环境值永不保存。
  Realtime 只配置变量名，不新增磁盘密钥或修改系统环境变量。保留最终明文密钥提醒，不新增“清空密钥”等无关功能。
- 共享记忆启用时，有界个人历史、术语和确认纠错可能发往用户配置的 Realtime 或 HTTP LLM 服务。
  HTTP 未启用整理时不会使用这份记忆；关闭开关不删除配置词表或个人运行文件。
  setup 的录音验证继续禁用个人记忆，不读取、上传或更新它；因此该验证不能证明个人记忆学习效果。

依据：[当前保存边界](../../src/ui/setup.rs#L479-L530)、[原子存储](../../src/core/config/store.rs#L51-L121)、
[凭据解析](../../src/core/config/fields.rs#L175-L188)、[Realtime 凭据解析](../../src/core/config/document.rs#L37-L45)、
[验证器的无记忆路径](../../src/ui/setup.rs#L281-L309)、[记忆隐私说明](../configuration.md#llm-整理与确认后学习)。

### 技术设计与模块边界

`src/ui/setup.rs` 继续拥有候选文档、分组导航、校验和唯一保存时机；配置模型及识别模块继续拥有格式与业务规则。
不为本向导建立第二套 ConfigKey、传输枚举、通用字段框架或 `Edit<T>` 状态层。
已有类型直接表达实际配置，新增界面草稿仅在当前步骤内存活。

计划增加/调整的主要接口：

- `SetupUi::multiline(&mut self, message: &str, default: &str) -> anyhow::Result<Option<String>>`：
  精确多行输入，与现有输入保持接受/取消/原生错误三态。
- `configure_recognition(document: &mut ConfigDocument, ui: &mut dyn SetupUi) -> anyhow::Result<bool>`：
  选择路径并编辑相应连接；返回 `false` 表示取消。
- `configure_advanced(document: &mut ConfigDocument, ui: &mut dyn SetupUi) -> anyhow::Result<bool>`：
  共享语言和所选路径的高级参数；数字通过现有 `set_field` 校验，文字直接保持其类型语义。
- `configure_dictation_prompt(config: &mut DictationConfig, ui: &mut dyn SetupUi) -> anyhow::Result<bool>`：
  处理内置/全文/组件来源和域校验；直接使用现有 serde 类型，不暴露组件字段、不包一层 JSON 解析器。
- `configure_memory(config: &mut MemoryConfig, ui: &mut dyn SetupUi) -> anyhow::Result<bool>`：
  共享记忆说明、开关和词表编辑，独立调用 `MemoryConfig::validate`，不让休眠中的 Realtime URL 阻塞 HTTP 配置。
- `edit_optional_text(ui: &mut dyn SetupUi, label: &str, current: &mut Option<String>) -> anyhow::Result<bool>`：
  统一需要多处使用的保留/null/自定义文字选择；不使用 CLI 字面量 null 协议。

局部名称和拆分可随实现精简；上述接口不改变公共配置 API 或运行时传输行为。

平台输入实现：

- `src/ui/setup/windows.rs`：复用已有模态对话框，增加
  `multiline_input_box(title: &str, message: &str, default: &str) -> anyhow::Result<Option<String>>`。
  内部明确区分普通文字、密码、多行三种控件模式；多行 EDIT 可滚动、允许 Enter 换行，显式按钮接受，Esc 取消，
  不使用小的默认文本长度限制。保留 UTF-16、无外部 owner 和原生失败处理。
- 新增 `src/ui/setup/macos.rs`，负责
  `input_box(title: &str, message: &str, default: &str, multiline: bool) -> anyhow::Result<Option<String>>`。
  用已有 objc2/AppKit 依赖实现 NSAlert 原生文字输入；多行使用 NSScrollView/NSTextView，关闭富文本、
  智能引号/破折号、自动替换和拼写修正。普通文字也从这个入口无损预填，避免在现有脚本输入上叠加“修复值”的特例。
  多行普通 Return 插入换行，Cmd+Return 接受，Esc 取消；主线程和模态生命周期需本机实测。
  密码框仍遵守原来的空白初始/遮罩策略，确认和消息流程不做无关重写；不新建 winit 事件循环。
- `src/ui/setup.rs` 的现有 `ScriptedUi` 扩展多行响应；`src/ui/setup/windows/tests.rs` 继续使用有界子进程驱动真实控件。
  不把 Windows 专用类型泄漏到共享配置。

### Windows DPI 调查结果与拟议修复

源码确认的事实：

1. `run`/`run_desktop` 都先运行 setup，再进入监听器的 winit 初始化；显式 `setup` 不会创建该事件循环。
   现有应用源码和构建/打包脚本未找到应用自有的 DPI 初始化或 DPI manifest 声明。
   [入口顺序](../../src/core.rs#L41-L99)、[winit 创建位置](../../src/ui/listener.rs#L82-L90)。
2. 已核读本机锁定依赖 winit 0.30.13：`src/platform_impl/windows/event_loop.rs:198-202` 在创建事件循环时才调用
   `become_dpi_aware`；`dpi.rs:20-42` 优先 PMv2 并有旧模式回退。这不能保护更早出现的设置对话框，
   也不能据此断言正常监听中的所有气泡都不感知 DPI。
3. 设置输入使用对话框单位、`DS_SETFONT` 和 9pt Segoe UI；气泡是自建 popup，使用 `DEFAULT_GUI_FONT`，
   宽度、边距、圆角和高度为固定像素，窗口过程没有 `WM_DPICHANGED` 分支。
   [设置模板](../../src/ui/setup/windows.rs#L215-L279)、[气泡布局](../../src/platform/windows/preview.rs#L276-L399)、
   [窗口消息和创建](../../src/platform/windows/preview.rs#L508-L710)。
4. 光标锚点已通过目标线程 DPI 上下文转换成物理像素，并在退出时恢复上下文；该正确边界保留。
   [锚点转换](../../src/platform/windows/preview/position.rs#L18-L110)。

候选机制：早期 setup 可能受到 Windows 位图缩放；已经由 winit 设置感知的气泡仍可能因字体、固定尺寸和跨屏复用
表现偏小或比例错误。用户实际遇到哪一种、是否有系统兼容性覆盖，尚未验证。

拟议修复：

- 在现有 platform 门面及 Windows 后端增加
  `initialize_dpi_awareness() -> anyhow::Result<()>`，在 `core::run`、`core::run_desktop`、
  集成 preview demo 的首个原生窗口之前调用，包括热键 helper 分流之前。
  使用 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，已设置时检查实际上下文，不能把所有失败都吞掉。
  原生测试的隔离子进程采用同一初始化。旧系统/兼容性模式不能启用 PMv2 时明确记录降级，不能报告清晰度验收通过；
  不新增更低版本 Windows 支持承诺，也不堆叠无需求的兼容框架。
- 标准设置对话框继续采用 PMv2 的系统自动缩放；不把模板中的对话框单位再乘 DPI，不额外套一层字体/坐标自动缩放。
- 气泡中的尺寸常量以 96 DPI 为基准，布局时按该窗口 DPI 换算；实际文字测量仍使用控件当前字体，
  保留现有正确测量路径。锚点、工作区和原生 API 返回的像素不再二次缩放。
- 气泡使用按 DPI 获取的系统消息字体描述创建 HFONT，替代 stock 字体。
  新的私有原生资源类型 `PreviewFont(HFONT)` 只拥有字体句柄，并在 Drop 释放；WindowState 增加
  `font: RefCell<Option<PreviewFont>>`（当前字体）、`dpi: Cell<u32>`（已应用的窗口 DPI）与
  `pending_dpi: Cell<Option<u32>>`（重入期间待处理的 DPI），不引入配置项。
  先将新字体应用到 text/status，再释放旧字体；销毁 HWND 先于释放最后的字体。
- 在气泡窗口处理 `WM_DPICHANGED`，采用系统建议矩形并结合现有锚点/工作区约束重新布局；
  创建及每次跨屏复用后以 `GetDpiForWindow` 的实际值更新，不能永久缓存首次主屏 DPI。
  若 `SetWindowPos` 在布局中同步产生 DPI 通知，记录待处理 DPI，退出现有重入保护后补做重排，避免通知被丢弃或反复缩放。
  不通过重写文本实现缩放，保留选区、只读、滚动、撤销、IME、Enter/Esc 和流式更新语义。

Win32 平台约束依据：
[进程感知必须先于 HWND](https://learn.microsoft.com/en-us/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process)、
[PMv2 与自建窗口缩放](https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows)、
[普通对话框默认自动缩放](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setdialogdpichangebehavior)。
这些文档和源码都不能替代报告机器上的复现。

### 替代方案、成本与风险

- 仅增加 Realtime 开关：改动少，但遗漏连接、提示词、共享记忆和其他模型字段，不能满足此次需求。
- 所有字段强制逐项询问：无需高级分组，但每次 setup 都很长，也容易误改休眠配置；选渐进式分组。
- 组件逐项输入、词表增删改表单：更适合不熟悉 JSON 的用户，但新增导航/列表逻辑较多。
  当前建议只对这两个结构化高级值使用校验 JSON 编辑，复用严格类型；代价是仍需理解 JSON 转义。
  普通全文提示词无需 JSON 转义。若用户要求逐项表单，在批准前调整此方案。
- 继续用 tinyfiledialogs 的净化默认值编辑提示词：会损伤引号、换行和长度；选局部原生无损输入，不移除展示用的净化边界。
- DPI manifest 是微软推荐的进程默认声明方式，但需新增资源构建及各可执行文件嵌入检查。
  当前选最早入口 API 初始化，利用已有 windows-sys 和平台边界，避免扩展打包链；按平台约束在任何 HWND 之前执行。
- 只设置进程感知：不能补齐自建气泡的字体和跨屏布局；只放大固定尺寸又不能保护早于 winit 的设置窗口。
  两部分需分别验证，普通对话框不重复缩放。
- DPI 字体/布局更新存在原生消息重入、GDI 资源泄漏、混合 DPI 坐标混用风险；以生命周期约束、实际窗口 DPI、
  原生编辑/布局 smoke 和独立期望值检查控制。没有 Windows 4K 实机时仍保留视觉验收缺口。

### 批准后的实施顺序

1. 扩展现有向导行为回归：完整自定义配置保留、选路与共享记忆、新步骤取消/错误不写盘、null/空字符串/密钥边界。
2. 增加两平台无损文字/多行编辑，接入基础/高级分组，复用已有模型校验；修正开关默认值和设备保留行为。
3. 增加最早 DPI 初始化及原生窗口感知检查，再处理气泡字体、逻辑尺寸和 DPI 变化；每层单独验证。
4. 更新 `scripts/test-windows-setup.ps1` 适配新首步，保持既有 CLI/GUI、direct/ShellExecute、SSH 环境与取消检查；
   在现有 Windows CI 运行适用的原生入口检查，不为此触发 release workflow。
5. 同步 `docs/configuration.md`、`docs/dictation.md`、`docs/cli.md`、相关 UI 架构说明与 changelog，明确新的向导入口和未覆盖验证范围。
6. 在同一 bookmark/PR 完成消融、必要简化和仓库 code-bearing push 审阅门禁，再按实际 CI 证据推进；不自动发布。

不需 schema 迁移。功能保留现有识别/学习协议；若需要回退，回退本 PR 的向导/DPI 改动，不删除个人配置、记忆或词表。

### 验证与消融

确定性验证围绕用户能观察到的配置和窗口行为，优先扩展现有测试，避免为每个私有 helper 增加测试：

- 用非默认且不对称的 HTTP 与 Realtime 配置检查“全部保留”的持久化结果：包含开启的 LLM、关闭的记忆、
  自定义组件、空全文、超过 2000 字且含引号/反斜杠/换行/CJK/emoji 的文本、磁盘备用密钥和不可用设备名。
  未选择路径参数不变，环境密钥不出现在保存内容，变量名空字符串仍可保存。
- 每类新输入的取消、UI 错误及最后拒绝保存都检查磁盘字节不变；没有文件时不得创建。
  无效 JSON/域输入修正后可继续，拒绝非法组件和词表，同时保留失败的输入草稿供修改。
  域内已有边界测试不机械复制；新增用例重点证明向导确实调用相同规则。
- 三种提示词来源、`null` 与空字符串用不同输入检查；数值非法/非有限输入不能保存，不新增模型没有的上下限。
  共享语言在 Realtime 下仍可设置，HTTP 的记忆编辑不被休眠 Realtime endpoint 阻塞。
- 录音验证使用候选路径、热键、设备、增益；保持不加载/发送/更新个人记忆。
  网络服务验证仅在用户允许的测试目标和无个人数据夹具上进行，不是保存配置的前提。
- Windows 原生子进程测试检查 Unicode、多行 Enter、遮罩、接受/取消、长输入、有效窗口 DPI 上下文与退出后无保存。
  气泡扩展现有原生 smoke：实际字体/行高、短长文本及警告、只读到编辑转换、选区与滚动、边缘位置，
  将写死 420/220/48/24 的期望改为独立的逻辑尺寸期望。合成 DPI 通知只证明消息处理，不冒充真实跨屏。

验证分层与当前承诺：

| 环境 | 批准后拟运行的检查 | 能力边界 |
|---|---|---|
| 当前 macOS | `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo clippy --locked -- -D warnings`；隔离配置下运行原生文字/多行/取消场景，截图后实际检查 | 证明共享流程和 macOS UI，不能证明 Windows 运行效果 |
| 当前 macOS 的 Windows 交叉目标 | `cargo check --locked --target x86_64-pc-windows-gnu --all-targets --features windows-app,integration` 与对应 Clippy；MSVC 检查在已有工具链可行范围补充 | 仅编译/静态检查，不代表执行过 Windows 控件 |
| 托管 Windows CI | 现有原生 build/test/Clippy、输入子进程及实际 EXE setup smoke；记录 runner 有效 DPI，集成预览检查在可执行环境运行 | 证明该 runner 下的 Windows 契约，不能把普通虚拟显示或合成通知等同真实 4K |
| 用户/后续可用 Windows 4K 机器 | 相同屏幕/缩放/版本下对照设置窗口和气泡：100% 控制、150%/200%，不同 DPI 副屏、同一气泡跨目标屏复用、改变显示缩放；检查中文/英文/emoji、长提示词、警告和文本编辑 | 才能关闭“真实 4K 模糊已修复”的视觉验收项；目前没有此证据 |

界面结果提供检查过的实际截图，不展示密钥或个人数据；macOS 截图不当作 Windows 图像。
运行界面前核实隔离配置路径，禁止用真实用户配置完成保存回归。

消融在实现后、自动代码审阅前进行：同一可复现 Windows 窗口场景比较 v0.5.0 基线、仅早期 DPI 初始化、
完整字体/布局/DPI 更新。分别记录窗口上下文、字体像素高度、窗口/控件物理尺寸、文本和选区，
识别“设置窗口位图拉伸”与“气泡尺寸/字体不随 DPI”是否分别改善。
无真实高 DPI 环境时只记录能执行的分层比较，把视觉与跨屏部分保留待验，不给模拟结果贴实机标签。
另外比较保留全部高级分组与进入后不修改两个路径，要求保存的配置语义一致，确保新增编辑机制没有隐式改值。

### 审批事项、实施中问题与延期范围

需要用户明确批准本方案后才写代码或测试。最值得判断的设计选择是：

1. 是否接受提示词组件与长期词表采用“带校验的原生多行 JSON 高级编辑”，普通全文仍为普通多行输入。
2. 是否接受当前先交付候选 DPI 修复和可执行的自动验证，真实 Windows 4K 清晰度保留待实机验收，报告中不标为已证实解决。

实现中自行核对、无需先阻塞批准的问题：AppKit 模态输入的 Return/自动替换行为、实际 HWND 感知与旧模式降级诊断、
DPI 通知重入的最小处理、CI runner 的 DPI 和可执行的预览场景。
用户若能提供模糊窗口种类、Windows 版本、缩放比例、显示器布局、启动方式和截图，将有助于针对性复现，
但这些缺失不阻塞向导实现和已定位 DPI 缺口的候选修复。

逐项词表管理、专用凭据存储、全局提示词调优、个人运行记忆管理及发布操作留在本次范围之外。

本轮已执行的工作仅为：最新远端基线核对、独立 workspace 创建、完整源码/文档与 Win32 约定核读，
以及本地只读方案咨询（实际模型 `claude-opus-5-5`，工具和 MCP 列表均为空，成功返回；建议由主代理核对）。
未运行新功能构建、产品测试、录音服务测试或 Windows 窗口，以上验证均为批准后的计划。

## 2. PR 变更时间线

### [001] — 2026-11-06 — 向导收缩为 Realtime / 普通识别二选一

- **背景与需求更新：**
  - [用户在本线程明确要求](https://ampcode.com/threads/T-01a1afc5-c7d0-7125-a67a-a98e48fad7aa)：下列高级字段不加入向导，
    主要提供 Realtime 和普通识别二选一配置。
  - 已完成的完整模型核对继续作为配置保留的依据；不再以“向导覆盖所有用户字段”作为本次验收目标。
- **决定与状态：** 用户已要求收缩范围；下述修订方案待明确批准，实现仍未开始。
  此条取代初始方案中的高级分组、多行/JSON 编辑、相关新接口和相关编辑测试，Windows DPI 方案及验收边界不变。

#### 修订后的向导范围

| 入口 | 提供的配置 |
|---|---|
| 识别方式 | Realtime / 普通识别（HTTP STT）二选一，默认当前 `dictation.enabled` 对应路径；支持取消 |
| Realtime | `dictation.url`、`dictation.model`、`dictation.api_key_env`；沿用现有默认值与 ws/wss/模型/变量名校验，环境变量名可为空表示无鉴权 |
| 普通识别 | 沿用 STT 地址、模型、API Key，以及可选 LLM 整理开关、地址、模型、API Key；不增加新的高级项 |
| 共用步骤 | 沿用热键、麦克风选择、可选录音验证及最终确认保存；修正当前开关默认值和设备不可用时的原值保留 |

选 Realtime 时不询问或重置 HTTP STT/LLM 参数，包括已存的 `post_process.enabled`；运行时是否禁用独立整理仍由现有识别路径决定。
选普通识别时不询问或重置 Realtime 连接和全部高级配置。保留连接与保留单个字段时，直接沿用读入值，不能通过净化过的默认输入回写改变它。
普通识别的 LLM 开关默认当前值；暂时断开的麦克风或枚举失败时，默认保留已有设备名称，只在明确选择后替换。
相关现有行为见 [向导](../../src/ui/setup.rs#L381-L485) 与 [所选路径解析](../../src/core/listener.rs#L52-L64)。

下列字段**全部不新增向导入口**，保留读入值，也不通过向导恢复为内置默认值：

- `transcription.prompt`、`transcription.temperature`。
- `post_process.preheat_enabled`、`post_process.prompt`、`post_process.temperature`。
- `dictation.prompt`、`dictation.prompt_components`，以及组件的 `task`、`language`、`numbers`、`cleanup`、`output_format`、`context`、`memory`。
- `dictation.memory.enabled`、`dictation.memory.long_term[]`，包括条目的 `context`、`kind`、`text`。

为落实“主要提供二选一”的目标，`transcription.language` 和 `audio.mic_gain` 本次也不增加入口；这两项已有值同样保留。
以上均是 UI 范围收缩，不删除配置字段，不改变运行时对提示词、温度、预热、语言、增益或记忆的使用，不改模型或产品默认地址。
若所选路径的既有高级值不合法，沿用领域校验并明确报告，不通过清空这些值自动修复或保存。

#### 对技术设计、复杂度和验证的影响

- 保留初始方案的 `configure_recognition(document: &mut ConfigDocument, ui: &mut dyn SetupUi) -> anyhow::Result<bool>`：
  在现有 `SetupUi` 输入/确认能力上选择两种模式，编辑所选路径连接；`false` 表示取消。继续由 `run_wizard` 拥有候选文档和唯一保存时机。
- 撤回 `SetupUi::multiline`、`configure_advanced`、`configure_dictation_prompt`、`configure_memory`、`edit_optional_text`，
  不新增 macOS AppKit 输入模块，不扩展 Windows 多行 EDIT，不引入 JSON 编辑器或其专用状态/校验流程。
- 复用现有单行/密码框；连接保留选择不重新赋值。若活动连接的单行输入存在无损保留问题，只做满足该契约的局部修正，
  不借此恢复全文编辑器或重写所有平台对话框。HTTP 密钥仍为空白初始、空输入保留磁盘、环境值优先且永不写盘；Realtime 只配置环境变量名。
- 用户未修改的所有高级字段和非活动路径配置，分别用含自定义提示词、空字符串、非默认数字、组件和词表的文档验证保存前后结构相等。
  这些是“未编辑值不被改写”的回归，不测试已撤回的高级编辑交互。
- 保留模式选择、Realtime 连接校验、两方向切换、现有 LLM/设备值保留、取消/UI 错误/拒绝保存不写盘、密码处理和验证器隔离个人记忆的检查。
  对模式选择及 Realtime 新增各类输入分别验证取消行为；休眠路径的无关参数不应阻塞当前路径配置。
- 不再为多行/JSON/高级参数编辑新增测试或截图；修订 `scripts/test-windows-setup.ps1` 的首步预期，macOS 和 Windows 沿用各自原生输入，
  运行/截图检查二选一及所选连接、取消等受影响界面。Windows DPI 的字体、布局、跨屏通知、原生 smoke 与真实 4K 待验边界全部保留。
- 文档只宣称向导新增识别方式选择和对应连接配置；提示词/记忆等仍保留现有配置文件入口，不再承诺向导可编辑全部 `dictation.*`。

#### 实施次序与待批准事项

1. 扩展现有向导回归：两模式、活动连接、未编辑配置保留、密钥与取消边界。
2. 接入二选一和 Realtime 必要连接，复用普通识别既有流程，修正默认选择和设备保留。
3. 按初始方案处理 Windows 早期 DPI 初始化、气泡字体/布局/跨屏更新，分别运行可执行的验证与消融。
4. 更新既有 Windows setup smoke 和相关使用文档，沿用同一 bookmark/PR 与代码审阅门禁，不触发发布。

原来的“JSON 高级编辑还是逐项表单”审批问题已撤回。当前只需批准这个简化后的整体方案；
真实 Windows 4K 清晰度仍需实机验收，当前 macOS 与普通托管 CI 不能替代。
本次仅更新方案文档和审批摘要，未新增实现或测试。
