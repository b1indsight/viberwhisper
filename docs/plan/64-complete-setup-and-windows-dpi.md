# 64 - 识别模式二选一配置与 Windows 高 DPI 窗口

**阶段：** 已按 [028] 的采用决定在本地接入正式 Windows 预览及演示，见 [029]；保留已接受的按下态色差。本轮格式、macOS 回归、Windows 交叉检查及纯像素对照已通过，待独立审查与推送后原生复验。[027] 的历史失败不改写为通过；真实 4K 仍待实机。
**PR：** [#165](https://github.com/b1indsight/viberwhisper/pull/165)，沿用同一 draft PR 交付实现。
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

### [002] — 2026-11-06 — 二选一向导与 Windows DPI 候选修复

- **背景与范围：** 按 [001] 的简化范围实现；完整模型审计用于保护未编辑值，不恢复高级编辑入口。
- **已实现的向导：**
  - `ui/setup.rs::configure_recognition` 在现有 `SetupUi` 边界选择普通识别或 Realtime，默认当前模式；
    Realtime 只问地址、模型和鉴权变量名，普通识别保留原 STT/可选 LLM 流程。两条路径互不改写连接参数。
  - LLM 开关默认当前值；断开的设备作为可保留选项，枚举失败保留原设备。未编辑连接保留原始空白和字符；
    tinyfiledialogs 的安全展示值原样接受时映射回原始值，不把截断或引号替换后的展示文本写盘。
  - 继续只在最终确认后原子保存一次；密码框为空、空输入保留磁盘密钥、环境值优先且不落盘。
    Realtime 只编辑变量名称；不更改运行时记忆、提示词或产品默认服务。
- **DPI 决策与实现：**
  - 在 CLI、GUI 和预览 demo 的首个 HWND 前设置并检查进程/线程 PMv2；已有 PMv2 可重复进入，
    不兼容的外部 DPI 上下文明确报错。源码确认 winit 只在创建事件循环时设置 DPI，无法覆盖更早的设置对话框。
  - 设置窗口保留标准 DLU、9pt Segoe UI 和 PMv2 自动缩放，避免同时手工放大模板。
    气泡改用对应窗口 DPI 的系统消息字体，由窗口拥有并在两个控件换字体后释放旧资源。
  - 只缩放 96-DPI 布局常量；原生测得的字形、边距、光标和工作区不二次放大。移除主屏宽度缓存，
    按当前工作区计算。`WM_DPICHANGED` 应用建议矩形并重新排版，保留文字、选区、滚动和 undo。
  - 同步原生调用可能在布局中递送 DPI 通知，故记录待处理 DPI 并在当前排版后处理；不递归借用状态、
    不任意限制重试次数。临时零尺寸子控件由唯一布局函数确定几何，创建失败时销毁窗口并释放字体。
- **测试与使用说明：** 扩展现有向导回归和 Windows 有界原生子进程，覆盖两方向切换、休眠连接无效、
  高级值/密钥/不可用设备保留、无效 Realtime 参数和取消/错误/最终拒绝保存。原生子进程检查 setup/preview
  的 PMv2 与字体，向真实 HWND 注入 96/144/192/96 DPI 和布局重入通知，检查布局和编辑状态。
  PowerShell smoke 更新为两种路径并加入普通 Windows CI；没有手动触发 release。
  README、配置/Realtime/CLI 指南、UI/平台架构与 changelog 同步更新。
- **证据边界：** 本机 macOS 的 build/test/clippy 和 Windows 目标类型/lint 检查通过。
  交叉检查设置 `DOCS_RS=1`，仅跳过 `ort-sys` 不提供的 GNU ONNX 原生产物准备，不代表 Windows 链接、运行或真实 4K 验收。
  新增 Windows 原生检查等待该实现提交的 Windows CI；此前 docs-only 绿色检查不证明新增功能。
  正常监听气泡可能已由 winit 设为 DPI-aware，用户报告机器的模糊根因仍未复现，不能据此宣称该机器清晰度已修复。

### [003] — 2026-11-06 — 保留契约消融与分层验证

- **消融设置：** 在同一独立工作区对 `switching_recognition_keeps_unedited_configuration_and_disk_keys`
  使用相同代表输入：双向切换、非默认且带空白/引号的模型、长 Unicode/空/组件提示词、长期词条、
  磁盘密钥和不可用设备。每次只撤去一项机制，执行
  `cargo test --locked --lib ui::setup::tests::switching_recognition_keeps_unedited_configuration_and_disk_keys -- --exact`，
  观察失败后恢复，再测试下一项；临时变体不保留。

| 对比 | 实际结果 | 决定 |
|---|---|---|
| 改动前基线运行新增保留回归 | 失败，识别路径、LLM 开关、设备和模型空白被改写 | 接入模式选择，并保护读入文档 |
| LLM 确认默认值恢复为写死 `false` | 失败，保留操作将已启用的整理关闭 | 沿用当前开关，不引入额外状态 |
| 必填输入统一 `trim()`，不区分原样接受 | 失败，未编辑值不再与原始文档相等 | 仅清理确实编辑的输入 |
| 不提供暂时断开设备的保留选项 | 失败，原设备被系统默认取代 | 使用现有编号选择追加原设备，不新增设备管理层 |
| 恢复完整实现后的相同回归与全量测试 | 通过 | 保留上述最小机制 |

- **实际验证：** macOS 的 `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、
  `cargo clippy --locked -- -D warnings` 通过；原有私有数据集/外部实时服务测试保持忽略，没有将其算作实机验证。
  Windows GNU 目标的 `cargo check` 和两套 `cargo clippy --all-targets`（`windows-app` 与
  `windows-app,integration`）通过，均使用 `DOCS_RS=1` 跳过依赖原生产物准备，不包含链接和执行。
- **macOS 原生输入的执行与限制：** 完整 `setup` 在显示向导前阻塞于 CoreAudio 设备枚举，采样栈定位到
  `cpal::supported_input_configs → AudioUnitSetProperty → mach_msg`；本次不修改该既有音频路径。
  临时测试通过既有 `run_wizard` 边界注入设备列表和临时配置路径，用真实 `NativeSetupUi` 打开识别方式、
  普通 STT 地址、Realtime 地址/模型/鉴权名五种对话框；前序步骤由测试适配器响应，每个目标原生窗口
  五秒自动结束后返回取消，全部退出且未创建配置，没有访问个人记忆或远端服务。
  仅针对该测试进程树的窗口截图返回 `could not create image from window`；当前无桌面控制工具，
  因此没有可审阅截图，不能宣称已完成 macOS 视觉检查。临时源码夹具和辅助脚本在验证后删除。
- **Windows 消融与待验：** 本机没有 Windows 执行环境，无法运行移除 DPI 初始化、改回 stock 字体、
  移除缩放或丢弃重入通知的原生对比。最小候选修复及对应真实 HWND 检查已落入代码，
  原生结果等待实现提交的 Windows CI；这些比较和真实跨屏/4K 视觉验收均延后，未以数学模拟或类型检查替代。
  后续实机应覆盖设置窗口、生成中只读气泡、可编辑气泡、长文本与警告、100/150/200% 缩放及混合 DPI 跨屏，
  并检查字体清晰度、输入法、选区、滚动和撤销。发布流程、个人设置和共享工作区保持不变。

### [004] — 2026-11-06 — Windows 系统弹框 DPI 诊断

- **触发与证据：** [实现提交的 Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42194060211/job/127812796156)
  编译成功，原生测试在首个 `MessageBoxW` 确认框的窗口 PMv2 断言失败；此前进程和 UI 线程的 PMv2
  初始化检查已连续通过两次。驱动线程在发送完成命令前 panic，留下模态弹框，随后父进程的 30 秒时限终止子进程。
  macOS CI 通过。原日志没有窗口实际上下文，不能据此称为初始化失败或 4K 模糊根因已确认。
- **诊断决策：** 保留严格的窗口 PMv2 断言，不先放宽测试或改写系统弹框。扩展既有原生测试，
  记录 HWND、类名、标题、所属进程/线程、实际 DPI 上下文及 PMv1/PMv2/system 比较；在调用弹框前记录 UI 线程上下文。
  先检查标题/正文/所有权，再检查 DPI，避免把未确认身份的窗口当成产品弹框。
  只读 Claude Opus 5.5 咨询也建议先区分错误 HWND、PMv1 与 system/unaware；咨询不算 Windows 运行证据。
- **失败处理：** 消息与输入驱动捕获检查 panic，仍按原命令完成自己拥有的模态弹框，再恢复原 panic；
  不忽略失败，不更改最终断言，不让真实断言被父进程超时掩盖。
- **边界与待验：** 本轮仅补足测试诊断和失败退出，不宣称系统弹框 DPI 问题已修复。
  本机只有 macOS，原生上下文及提前退出效果仍须该诊断提交的 Windows CI 确认；不把交叉编译当作运行或 4K 验收。
  不新增产品分支或缩放机制，故不引入产品消融变体。此前 macOS 长地址审查项仍待单独返修授权，未混入本次改动。
- **本机验证：** `cargo fmt --check`、`cargo test --locked --lib ui::setup::tests` 与
  `DOCS_RS=1 cargo clippy --locked --target x86_64-pc-windows-gnu --all-targets --features windows-app,integration -- -D warnings`
  通过；后者仅证明 Windows 目标的类型/lint，不包含原生链接或执行。

### [005] — 2026-11-06 — 设置确认与提示复用原生模板

- **诊断结果与触发：** [诊断提交的 Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42195829987/job/127818743053)
  确认调用线程为 PMv2，而同一进程/线程、标题和正文匹配的 `#32770` 确认框为 system-aware；
  PMv1/PMv2 比较均为假，system 比较为真，有效 DPI 为 96。检查失败后仍成功回答弹框，子进程正常报告断言，
  不再由父进程超时掩盖。排除了错误 HWND、无效句柄和早期初始化失败；Windows 内部选择该模式的原因仍未证实。
- **决定与状态：** 在已批准的 DPI 范围内，用 `DialogBoxIndirectParamW` 替换设置确认/提示的 `MessageBoxW` 路径，
  复用现有输入框的对齐模板、9pt Segoe UI、DLU 和 PMv2 自动缩放；严格窗口断言保持不变，原生结果仍待新 head 的 CI。
  这条决定细化 [002] 的设置窗口实现，不改变识别、保存、个人配置或记忆。
  [微软的 PMv2 文档](https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows)
  支持模板对话框自动缩放；只读、无工具/MCP 的 Claude Opus 5.5 咨询指出，隐藏 owner 或 v6 manifest
  是否改变本例 `MessageBoxW` 行为尚无证据。因此不增加隐藏 HWND、跨应用 owner 或资源构建依赖，也不把咨询当作运行证明。
- **交互与取舍：** 确认保留默认 Yes/No，不增加 Esc/关闭的隐式答案；提示可由确定、Esc 或关闭结束。
  创建/初始化错误仍传播，不变成 No。确认和提示使用与输入框一致的中文按钮、可滚动正文，不再显示系统问号/信息图标，
  按钮也不再跟随 Windows 的界面语言。输入/密码的模板和编辑协议不变，没有重写向导流程。
- **回归与可观测性：** 扩展既有有界原生测试，保留标题、正文、所有权与严格 PMv2 检查，
  同时检查默认按钮、确认不接受 Esc/关闭、提示接受 Esc/关闭；成功路径也输出实际 HWND/DPI 和气泡检查记录。
  EXE smoke 增加空必填项的提示与恢复，继续覆盖两条识别路径、两种 EXE、direct/ShellExecute、SSH 环境及取消不保存。
  CI 请求隔离账号的非密码窗口截图并上传；截图只来自测试进程，临时切换截图线程至 PMv2 后恢复，避免虚拟化尺寸。
  截图须取回并实际检查后才算视觉检查，不能把上传步骤或 96-DPI 图像当成真实 4K 清晰度证明。
- **本机验证与比较：** 格式、macOS 向导回归、Windows GNU 目标的 all-targets/integration lint 通过，
  后者仍仅为 `DOCS_RS=1` 的类型/lint 检查。临时程序在 macOS 编译并执行修改前后的纯 Rust 模板构建函数，
  比较同样的普通/密码输入，两个 `DLGTEMPLATE` 均与父提交逐字节一致；确认/提示的控件数、DLU 矩形和 9pt 声明符合预期。
  比较源码与结果保留于本线程工作区 `.amp/in/artifacts/dialog-template-data-comparison.log`，临时程序随后删除；
  此项只保护模板数据，不验证 Win32 渲染。原生替换前后、字体/布局消融、截图和气泡结果等待 Windows CI，
  真实 4K/混合 DPI 清晰度继续待实机。此前长地址审查项未获新的返修要求，未混入此修复。
- **CI 返回后的验证结果：** [修复提交 9c8997d7 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42199751516)
  在 macOS 和 Windows 均通过。Windows 的构建、全量测试、原生/DPI smoke 和 Clippy 完成；
  [原生日志](https://github.com/b1indsight/viberwhisper/actions/runs/42199751516/job/127831820751)
  确认实际确认、提示和输入窗口均为 PMv2、system=false、DPI=96，原来的系统确认框断言失败已关闭。
  气泡的生成中只读、可编辑警告两种状态通过合成 96/144/192/96 DPI 及同步重入检查，核对字体、布局、
  文本、Unicode 选区、滚动、撤销及只读状态。CLI/GUI、direct/ShellExecute、SSH 环境、两种识别模式、
  必填提示/恢复及取消不保存 smoke 全部通过。
  下载本次 `windows-setup-captures` 并实际检查模式选择、普通地址、Realtime 地址/模型/鉴权名、
  首次确认和必填提示：中文与按钮完整，未见裁切、缺字、重叠或空白捕获。
  截图只来自 96-DPI 托管显示上的非密码夹具窗口；气泡没有截图，合成通知也不是实际跨屏。
  设置窗口的感知模式已有前后原生证据，字体/布局机制的消融、气泡视觉效果及真实 4K/混合 DPI 清晰度仍待实机；
  未改动或关闭此前 macOS 长地址审查项，PR 继续保持 draft，未合并或发布。

### [006] — 2026-11-06 — Windows 识别结果气泡视觉验证

- **请求与起点：** 用户要求测试 Windows 识别结果窗口的效果。[005] 只检查了设置窗口截图，
  气泡虽通过原生控件断言但没有图像。本轮补足现有 `viberwhisper-preview --artifacts` 的 Windows 原生捕获，
  复用已有流式/最终结果与失败夹具，不制作替代 UI；无目标捕获、文本注入、配置或持久化历史写入。
- **执行前确定的案例：** 默认短句可编辑、生成中前缀/修订/撤回、中文/英文/emoji 编辑结果、
  长文本自动换行与有界滚动、长文本加警告、空结果失败和收敛超时；检查正文、提示区域和控件没有重叠或裁切。
  空失败 Enter 不得确认，Esc 各取消一次；正常结果与生成中只读状态作为相邻对照。
  在已有 DPI 原生断言的 96/144/192/96 及重入路径捕获真实控件，核对字体、边距、尺寸及文本/选区/滚动/撤销保留。
- **验证边界：** 预设文字代替麦克风与识别服务，Windows 执行仍依赖托管 CI。
  日志和文件名须区分实际窗口 DPI 与合成布局 DPI；合成 150/200% 图像不代表真实显示缩放、混合屏、输入法或 4K 清晰度验收。
  不改生产窗口外观和交互，不混入既有 macOS 长地址审查项；截图下载并逐项检查前不报告视觉通过。
- **本机检查：** 格式、macOS 预览协议回归、带 integration 的 all-targets Clippy 和 CI YAML 解析通过。
  Windows GNU 目标带 `windows-app,integration` 的 all-targets Clippy 通过，仍使用 `DOCS_RS=1`，不含原生链接或执行。
  macOS 实际运行同一 `--artifacts` 命令正常退出并生成各状态图，作为共享演示路径的相邻对照；不能据此推断 Windows 效果。
  新增的捕获代码只在测试/integration 功能中编译；不捕获其他应用，也不修改既有原生断言。
- **首轮 Windows 执行：** [捕获提交的 Windows job](https://github.com/b1indsight/viberwhisper/actions/runs/42205815584/job/127852037150)
  的构建、全量测试、原生/DPI smoke、两模式 EXE smoke 与气泡演示均通过，生成 19 个 PNG；macOS job 也通过。
  最后的 Windows Clippy 1.99 因 `chunks_exact_to_as_chunks` 新 lint 失败；本机为 1.94，未触发该 lint。
  将捕获工具的四通道遍历改为 `as_chunks_mut::<4>().0`，不压制 lint 或改变像素转换；本机两种 Windows feature 组合检查通过，CI 回归待新 head。
- **已检查的图像：** 默认短句、流式前缀/修订/撤回、中英混合/emoji、长文本、警告、空失败与超时均已查看。
  正文和提示可读、没有相互覆盖；两种空错误的提示完整，Enter 不确认及 Esc 各取消一次由实际演示断言通过。
  合成布局的生成中/警告气泡已查看，实际窗口 DPI 均为 96；警告气泡分别为 420×268、630×402、840×536。
  19 张 PNG 的 CRC/解压长度有效，回到 96 DPI 和重入 144 DPI 的四对 PNG 均逐字节相同。
  图像是自有 HWND/子控件的 `WM_PRINT` 原生渲染，不包含桌面合成器阴影等效果，不代表物理高 DPI 显示的清晰度。
- **效果观察与范围：** 短句“我们明天上午去上海。”也会拆为两行，右侧始终显示竖向滚动条；
  基线已有同一自动宽度 padding 和 `WS_VSCROLL` 组合，本轮如实展示，不因效果测试顺手改生产布局。
  长文本处于有界滚动视口，不把不可见的上方内容称为数据丢失；真实跨屏、输入法、麦克风/服务与 4K 仍未验证。

### [007] — 2026-11-06 — 修复 Windows 气泡短句换行与常驻滚动条

- **用户补充与基线：** 用户在看到 [006] 图像后明确要求“先 fix 布局问题”，因此本轮扩展 [006] 的仅测试范围，
  修复已观察到的短句换行和多余竖向滚动条；不扩展设置向导或修改此前 macOS 长地址审查项。
  [基线 8ccc9d8a 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42207604879) 两平台均通过，
  重新取回的 19 张 PNG 与首轮逐字节相同，默认短句仍为 157×54 的两行气泡，中英混合结果仍为 261×54。
- **执行前确定的验收案例：** 在原生 `EDIT` 上验证单行短句没有不必要换行且无竖向滚动条，
  覆盖报告中的中文短句及混合/emoji；长内容超过既有高度上限时保留可用的滚动条和滚动能力，
  从短句增长到长文本再删除回短句后重新收起滚动条。检查恰好容纳与多一行的边界，不只覆盖极短/极长内容。
  保留既有生成中只读、可编辑、空错误、末尾换行、警告页脚、Unicode 选区、滚动、撤销、屏幕边缘和 DPI 重入检查。
- **范围与验证：** 修改只归属 Windows 气泡布局和既有原生回归夹具，沿用逻辑宽高上限、字体、定位和确认协议。
  不通过整体加宽或永久移除滚动能力掩盖问题。以同一原生捕获命令比较修复前后各状态；
  本机 Windows 目标类型/lint 和 macOS 回归是相邻检查，窗口效果须取回 Windows CI 图像后实际检查。
  合成 96/144/192 DPI 与真实 4K、显示缩放和跨屏验收继续分开报告。
- **实现与取舍：** `src/platform/windows/preview.rs` 先按不含滚动条的客户区宽度重新排版，
  通过当前原生窗口/客户区几何补偿非客户区，而不假定合成布局 DPI 与系统滚动条 DPI 相同。
  只有内容超过原有 196 逻辑像素视口上限才显示滚动条；有余量时为其保留实际宽度，仍不突破原有宽度上限。
  在每次布局前后保留首个可见行，防止临时测量重排夹断滚动位置；没有增加持久状态、辅助层或滚动条收敛循环。
  未采用所有气泡统一加宽、永久去掉滚动能力或每次键入都隐藏再显示滚动条的替代方案。
- **原生回归扩展：** 沿用 `src/platform/windows/preview/smoke.rs`，用实际行数和窗口样式检查中文短句、混合 emoji、
  只读/编辑状态、增长再缩短，以及刚好容纳/多一行/重新容纳的边界。现有 DPI 通知与重入夹具增加短句，
  验证无滚动条时的单行、最小高度和选区；长文本继续验证滚动、撤销、只读状态与系统字体。
  捕获日志新增原生行数和滚动条状态，避免只凭图片判断。
- **已完成的本机检查：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked --features integration`、
  `cargo clippy --locked --all-targets --features integration -- -D warnings` 通过。
  两组 `DOCS_RS=1 cargo clippy --locked --target x86_64-pc-windows-gnu --all-targets --features windows-app[,integration] -- -D warnings`
  通过；该环境变量仅避开本机不支持的 GNU ONNX 准备步骤，不代表 Windows 链接或运行。
  原始输出保留在工作区 `.amp/in/artifacts/preview-layout-local-validation.log`。
- **比较与尚未完成的验证：** 修改前的原生捕获已证实短句换行和常驻滚动条；修复后的同命令原生捕获须由 Windows CI 执行。
  尚未在 Windows 运行逐项撤去宽度补偿/显隐/滚动恢复的消融变体，不能把源码推导或本机交叉检查当成这些对比的结果。
  本轮预审时保留这项环境限制，后续实际窗口、边界断言及截图结果在
  [验证记录](https://ampcode.com/artifacts/windows-dpi-b8d76e0c-091e-4333-88d5-ba67d9cb623e) 更新。

### [008] — 2026-11-06 — 按最终视口限制滚动恢复

- **触发与范围：** [007] 的独立审查指出，布局和流式更新均无条件恢复旧首行；已滚动的长结果收缩成可完全容纳的多行时，
  可能留下正的首行且已没有滚动条。用户明确要求返修此项；该场景仍未在修改前的 Windows 程序中复现，不能把审查推断写成实测根因。
  本轮仅处理 Windows 气泡滚动恢复，不修改 macOS、设置向导或既有长地址审查项。
- **上一轮结果：** [布局候选 21b0c97f 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42212958061) 在 macOS 和 Windows 均通过。
  已取回并检查原生短句、混合 emoji、长文本、警告及合成 DPI 图像；短句和混合结果均为单行、没有滚动条，
  长文本仍有有界滚动视口，警告与正文分离。此结果不覆盖新增的已滚动多行收缩场景。
- **执行前确定的案例：** 从四十行且离开末尾的首行位置开始，分别收缩到八行、超过视口三行、超过视口八行，
  依次要求首行归零、限制到最后完整视口、保留仍有效的首行。另从末尾及末尾前一行各追加一行，
  分别要求跟随末尾和保留用户位置。生成中只读与最终可编辑状态均覆盖，检查完整文字、原生行数、选区、撤销和滚动条状态。
  继续运行原有字体、DPI 重入、屏幕边缘、警告和空错误检查；图像仍须取回并检查。
- **实现决策：** 复用实际字体度量和最终 `EM_GETRECT` 排版区域计算最大首行，由布局恢复、流式恢复和末尾判断共用。
  完全容纳时上限为零，仍溢出时只截断已失效的首行；不增加持久状态、额外缩放或滚动条收敛循环。
  只读、无工具/MCP 的 Claude Opus 5.5 咨询也建议用最终排版区域，避免将刚隐藏滚动条的未验证 range/page 状态当成依据；
  咨询不是 Windows 执行证据。保留原有字体和文本测量路径，不另建字体缓存或页面测量层。
- **已完成的本机验证：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked --features integration`、
  `cargo clippy --locked --all-targets --features integration -- -D warnings` 及 Windows GNU 目标两种 feature 组合的 all-targets Clippy 均通过。
  后者继续使用 `DOCS_RS=1`，没有 Windows 链接或执行；原始输出在工作区 `.amp/in/artifacts/preview-scroll-local-validation.log`。
  新原生回归先于候选实现写入，但尚未执行其修改前的 Windows 版本，不将此顺序称为原生先红后绿。
- **比较及复杂度：** 两条恢复路径合并使用同一上限和滚动动作，未增加持久状态或控制循环；上限计算复用原有字体测量，
  恢复时会重新查询，未做原生性能测量。尚未运行移除上限的原生消融，不报告消融通过。
  上一轮图像的 PNG 结构检查通过，三种夹具的 96-DPI 回程和 144-DPI 重入图像各自逐字节相同；
  这只验证既有捕获结果的稳定性，不替代新增滚动场景的原生对照。
- **验证边界：** 本机仍只有 macOS。新增断言尚未在 Windows 运行，同一新增场景的修改前后原生对照及消融尚未完成；
  本机交叉检查只证明类型/lint。原生 CI 将验证候选的结果，不能单凭修改后的通过声称旧程序一定出现该问题。
  真实 4K、物理显示缩放、跨屏及输入法仍待实机验证。

### [009] — 2026-11-06 — 原生滚动回归通过，修复演示中的重复执行

- **实际结果与触发：** [008] 候选的 [Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42216619935/job/127888139564)
  通过新增的五组只读/可编辑滚动案例及原有字体、DPI 重入和编辑状态断言；原生测试正常返回。
  后续演示每次流式修订又调用同一布局夹具，新增整组案例重复三遍；演示源码保留十秒总时限，
  日志显示进程约二十秒后以 `Native preview delivery timed out` 退出。最终 Clippy 未执行，不能称整轮 Windows CI 通过；macOS job 通过。
  完整输出在工作区 `.amp/in/artifacts/ci-windows-preview-scroll-395d02b1.log`。
- **决策及实现：** 将新增的收缩、保留和追加案例从逐次执行的 `smoke_placement` 移到测试专用 `smoke_scrolling`，
  由既有 `smoke_dpi` 在恢复实际窗口 DPI 并完成流式检查后调用一次。保留同一批输入、期望值、状态断言及可选 PNG；
  原有逐次流式、定位、换行和滚动条边界检查继续执行。不延长演示期限，不放宽断言，不修改生产布局、向导或 macOS 路径。
  本轮只调整验证职责，没有新增运行时机制；不另加测试计数器、计时宽限或重复运行开关。
- **已检查的原生图像：** 十张新增滚动图按预期首末行逐项核对：归零、末页限制、有效位置保留、末尾跟随及末尾前位置保留均符合断言。
  尾随图证明最后一行可见，不将滚动条外观当成原生 range/page 数据。全部 PNG 的 CRC 和解压长度有效，
  既有图像与 [007] 候选逐字节相同；比较输出在 `.amp/in/artifacts/windows-preview-scroll-image-validation.log`。
- **本机检查及原生边界：** 格式、构建、macOS integration 测试与 all-targets Clippy，以及 Windows GNU 两组 feature 的 all-targets Clippy 均通过，
  输出在 `.amp/in/artifacts/preview-scroll-fixture-validation.log`。GNU 检查继续用 `DOCS_RS=1`，没有 Windows 链接或执行。
  调整后的同命令 Windows 原生测试及演示仍待新 CI，不能由本机检查推定超时已消除。
  旧程序同一收缩场景与移除上限的原生消融仍未运行；实际 DPI=96 的捕获及合成通知仍不替代真实 4K、混合屏或输入法验收。

### [010] — 2026-11-06 — 右侧滚动条外观提案（待批准）

- **请求与状态：** 用户基于最新长文本警告截图要求美化右侧滚动条。此项在 [007]–[009] 的布局与滚动修复上增加外观，
  沿用同一功能记录、bookmark 和 PR；本条为待批准方案，尚未编写产品实现或原生测试。
  不修改正文、警告区、字体、气泡尺寸上限、确认协议、macOS 或配置向导。
- **建议外观：** 去掉整列浅灰轨道，只显示约 6 DIP 的中性灰圆角滑块；普通、悬停、按下时依次加深，
  不添加箭头、渐隐动画或配置选项。保留原生槽位对应的鼠标命中范围，不能把可拖动区域一起缩成 6 DIP。
  短内容继续隐藏滚动条，系统高对比度模式保留原生外观。
- **方案与取舍（待验证）：** 仅切换原生主题能否满足指定外观没有运行证据；不改系统全局滚动条度量。
  优先验证一个位于原生槽位上的同级绘制覆盖层，其 `WM_NCHITTEST` 返回 `HTTRANSPARENT`，让同线程 EDIT 继续接收输入。
  保留 `WS_VSCROLL`、原生槽宽、原生拖动/键盘/滚轮以及可访问滚动对象，滑块位置和长度只读 `GetScrollBarInfo`，
  不建立第二套行数、页高或拖动模型。不采用移除原生滚动条再重写手势/无障碍的完整自绘控件，
  也不依赖只处理部分消息的非客户区重绘补丁。点击穿透与无障碍命中仍须实测，不能由消息返回值直接推定兼容。
- **批准后的实施顺序：** 先在现有 Windows 原生夹具验证覆盖层的 z 序/裁剪、输入穿透、模态拖动期间重绘、
  UIA 命中与 `WM_PRINT` 合成；有一项不成立则保留原生样式并回报，不自动扩大为自定义滚动系统。
  可行后在 Windows 预览模块创建/定位/绘制覆盖层，观察原生滚动、布局和 DPI 变化刷新外观；
  不接管编辑焦点或手势，不改变 [008] 的最终视口上限与位置恢复。原生几何保持物理坐标，只将滑块宽度等外观常量按 DPI 缩放，
  且不得超出实际槽位。处理高对比度/主题变化及创建失败、销毁时的句柄/GDI 生命周期。
- **归属与改动范围：** 生产入口为 `src/platform/windows/preview.rs`，绘制/生命周期确需独立职责时可用其私有子模块；
  扩展既有 `src/platform/windows/preview/smoke.rs`，不新建通用 UI 框架或滚动组件层。
  如需 `HIGHCONTRASTW`，只启用现有 `windows-sys` 依赖的 `Win32_UI_Accessibility` feature，不添加新 crate。
- **执行前固定的原生验收：** 检查槽位边缘而非仅滑块中心的鼠标命中；滚轮、键盘、轨道点击、拖动到首尾时，
  原生首行与滑块同步且没有重复滚动、跳动或丢失焦点。覆盖只读/编辑、短长切换、警告、文本收缩、流式保留与末尾跟随，
  保留现有 DPI 回程/重入、选区和撤销检查。捕获普通、悬停及按下状态，确认只有槽位像素改变、无灰色整列和重叠，
  高对比度回退保留系统条；用实际原生坐标核对滑块，不用截图外观推导滚动范围。
  CI 若不能执行真实鼠标拖动或 UIA 检查，应明确保留未验证项，不能以消息模拟、类型检查或样式截图代替。
- **本轮仅完成的准备：** 已查看当前原生截图与控件/布局代码，并完成只读、无工具/MCP 的 Claude Opus 5.5 方案咨询。
  HTML 效果稿直接嵌入既有原生截图，只叠加右侧外观；浏览器执行了当前/建议、交互配色及窄屏显示检查并查看渲染图。
  此稿用于批准外观，不是修改后的 Windows 运行结果。既有成功 CI 不证明本提案；真实 4K、物理显示缩放、混合屏和输入法仍未验证。

### [011] — 2026-11-06 — 整体对齐 macOS 深色气泡（待批准）

- **请求与替代范围：** 用户随后提供 macOS 短句气泡，要求“效果对齐 macos 的外观”。本条替代 [010] 只改滚动条、
  不动正文/提示配色和字体的范围；[010] 保留为历史，其中保留原生滚动语义和条件可行性验证的约束继续适用。
  当前仅修订方案和 HTML 效果稿，产品代码及原生测试均未改动；不修改 macOS、配置向导或此前长地址审查项。
- **参考与主题取舍：** 已阅读 macOS 预览源码及引入该外观的
  [effadc792f2d](https://github.com/b1indsight/viberwhisper/commit/effadc792f2d62262acdf58f0e63189c530b7154) 历史，复用既有原生演示/捕获方法。
  macOS 使用动态 `textBackgroundColor`、`textColor`、`separatorColor` 和 `secondaryLabelColor`，不是写死深色主题。
  本方案以用户提供的深色态为目标，正文底色采样约为 `#1e1e1e`，描边/次要色近似匹配；不称为跨平台 AppKit 常量。
  建议本批固定该深色态，不新增自动明暗切换或主题配置；这一点须随本条明确批准。高对比度开启时使用系统色与原生滚动条，
  在初次创建、`WM_SETTINGCHANGE` / `WM_SYSCOLORCHANGE` 后重新判定，不修改系统全局设置。
- **建议外观与保留项：** 沿用既有 14 DIP 圆角，补细描边，正文与警告共用深色底；正文浅色，提示较小且使用次级灰。
  横向 16、纵向 12 DIP 正文留白，以及 28–196 DIP 视口、96–420 DIP 宽度约束和现有短句垂直居中保持不变。
  默认文字设置下以正文/提示 15/12 DIP 为比例目标：继续读取窗口 DPI 对应的 `lfMessageFont`，保留系统字体族，
  正文相对现有基准按 15/12 放大、提示沿用基准，避免把 macOS 15 点误当 Windows 15pt 而变成 20 DIP。
  该做法保留已反映在字体度量中的放大；Windows“放大文本”是否进入此度量仍待实机，不提前声称已验证。
  光标、选区、输入法、编辑/只读、键盘与撤销仍归原生 EDIT；不复制 Apple 字体或模拟蓝色光标，不引入分层窗追求逐像素阴影/抗锯齿。
- **归属与最小实现：** 在 `src/platform/windows/preview.rs` 处理窗口底色/描边及 EDIT、STATIC 配色，
  按 HWND 区分只读 EDIT 与状态提示，覆盖 `WM_CTLCOLOREDIT` 和 `WM_CTLCOLORSTATIC`。优先复用现有 GDI 机制，
  不增加通用主题/字体框架；两种字号沿用已有字体所有权和同步重入约束，原生控件全部切换后才能释放旧字体。
  提示横向留白拟对齐 macOS 的 20 DIP；页脚 48→44 DIP 仅在真实字体测量证明全部警告/错误文案均不裁切时采用，
  否则保留足够高度，不以缩字、截断或盖住正文换取一致尺寸。
- **滚动条与可行性：** 深色底上的约 6 DIP 滑块在普通/悬停/按下时逐级变亮，替代 [010] 白底逐级加深的配色。
  保留实际原生槽宽、原生 `WS_VSCROLL` 和 [008] 最终视口上限/位置恢复，不移除滚动对象或另建行数/页高/拖动模型。
  沿用 [010] 的只绘制覆盖层与 `GetScrollBarInfo` 几何来源，先验证 z 序/裁剪、鼠标/UIA 命中、模态拖动重绘及 `WM_PRINT` 合成；
  不依赖未公开深色主题类名，有更简单且已实测符合目标的公开原生机制时优先使用。若可行性不成立，保留原生条并报告剩余差异，
  不将部分实现称为外观已对齐，也不自动扩展成完整自绘滚动系统。
- **批准后的原生验收：** 扩展既有 `preview/smoke.rs`，覆盖短句、中英/emoji、长文、警告、空错误与生成中只读；
  运行 96/144/192/96 合成布局和 DPI 同步重入，检查两种字体、文字/选区/撤销保留、描边、只读和提示配色。
  由实际行高及最终 `EM_GETRECT` 生成恰好容纳/多一行与收缩/尾随案例，不沿用旧字号的固定可见行数。
  同时执行 [010] 的槽边缘命中、滚轮/键盘/轨道/首尾拖动和可访问性检查；模拟高对比度判定只证明回退分支，不替代真实高对比度。
  继续使用已有无注入目标的 `viberwhisper-preview --artifacts` 及 Windows CI 捕获，下载并查看实际各状态图后才报告原生效果。
- **本轮证据与边界：** 已完成固定 Opus 5.5、无工具/MCP 的只读方案咨询，建议经本地源码核对后采用；咨询不是执行证据。
  更新同一视觉 artifact，分别演示短句、长文、警告及三种滑块配色；HTML 长文/警告复用现有原生演示文案，
  滑块为明确标注的可视示意层，浏览器字体和滚动不能证明 Windows 字形、拖动、UIA 或合成捕获可行。
  本轮没有 Windows 原生运行；真实 4K、物理 150/200% 显示、混合屏、输入法和系统文本放大仍待实机。

### [012] — 2026-11-06 — 深色气泡本地实现与隔离的滚动条可行性夹具

- **请求、决策和状态：** 用户确认 [011] 后，按截图的固定深色态实施，不新增明暗主题或配置向导设置。
  当前仅为独立 JJ workspace 内的未提交实现；没有推送或新的 Windows CI 结果，不能称为原生外观已验收。
- **实际实现：** `windows/preview.rs` 以现有 GDI 绘制 `#1e1e1e` 底与 `#343434` 细边，保留 14 DIP 圆角、
  既有正文留白、短句居中及尺寸上限。EDIT 与 STATIC 按 HWND 区分文字颜色，同时处理只读 EDIT 发出的 `WM_CTLCOLORSTATIC`，
  防止生成中文字误用提示颜色。系统高对比度开启或判定查询失败时使用系统底/文字色；收到设置、系统色或主题消息后重新判定、重绘。
  画刷和画笔使用 Windows stock DC 对象，不增加每窗 GDI 资源管理层或全局主题修改。
- **字体与页脚：** `PreviewFont` 持有正文、提示两个 HFONT；提示沿用当前 DPI 的 `lfMessageFont`，正文高度按 15/12 四舍五入，
  保留字体族。沿用 DPI 重入保护与两控件全部切换后释放旧字体的顺序。提示 x 留白改为 20 DIP，使用提示自身字体和最终宽度测量，
  最小提示高 32 DIP、底部 12 DIP，常规页脚为 44 DIP，较长警告/错误允许增高而不遮住正文。
  已核对 `finish_transcription`：服务故障采用固定用户文案，不把任意长提供商响应直接作为气泡警告。
- **原生夹具准备：** 在既有 `preview/smoke.rs` 更新两字号期望，按实际行高与最终 `EM_GETRECT` 推导滚动边界，
  不再假定八行总能容纳；保留收缩、范围夹取、人工位置、末尾跟随、选区、撤销和 DPI 重入场景。
  增加短句/只读/合并警告/空错误在合成 96/144/192 DPI 下的颜色、提示留白与高度检查，捕获后检查背景/描边像素。
  局部覆盖系统配色只验证回退绘制路径，不修改 OS 高对比度设置，也不证明真实系统切换。
- **滚动条仍未启用：** 将一次性实验隔离到仅 `cfg(test)` 编译的 `preview/scrollbar_probe.rs`；正式预览及演示程序不创建覆盖层。
  实验使用 `GetScrollBarInfo` 的原生槽/滑块几何、同级命中透明窗口、EDIT 的 sibling clipping 和原生滚动消息观察，
  不另造滚动范围/拖动模型。初步夹具准备检查槽位几何、实际 `WindowFromPoint` 边缘命中、短文/模拟系统色回退、
  原生与覆盖层 `WM_PRINT` 对照、约 6 DIP 滑块，以及槽位以外像素不变；断言失败会正常报错，不降级成成功。
  这些新增断言在本机没有执行。屏幕实际 z 序/剪裁、模态真实拖动与 UIA 仍是独立门禁，不能用合成捕获或点查询替代。
  Windows 原生执行必须更新原 PR 后由现有 CI 完成；本轮尚无新增推送授权，不手动触发工作流。
- **本机结果：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked --features integration`、
  `cargo clippy --locked --all-targets --features integration -- -D warnings` 通过。
  Windows GNU 目标分别以 `windows-app` 与 `windows-app,integration` 执行 all-targets Clippy 通过；`DOCS_RS=1` 继续仅跳过 ONNX 原生准备，
  没有 Windows 链接/运行。依赖的既有 rdev 警告仍输出，未静音或改动依赖。
  日志位于本 workspace `.amp/in/artifacts/macos-aligned-macos-validation.log` 和 `macos-aligned-windows-cross.log`。
- **复杂度与待验收：** 两字号直接扩展原字体所有者，配色直接在原窗口过程处理，不增加通用主题框架。
  尚不能进行 Windows 原生的单字体/双字体、原生条/覆盖层同条件消融；复杂的覆盖层明确留在测试夹具，未因类型检查通过就进入产品。
  仅保留准备好的原生对照输入，待实际 Windows 结果决定是否保留/简化该机制，移除一次性实验后才能完成此批功能。
  新外观 PNG 尚未产生或查看；已有 HTML 图仍只是目标稿。真实 4K、物理缩放、混合屏、输入法、系统文本放大及真实高对比度均未验证。

### [013] — 2026-11-06 — 原生提示区失败诊断与首轮测试截图保留

- **触发与已知事实：** 用户授权审查后更新同一 PR 并继续 Windows 原生验证。
  [67467d12](https://github.com/b1indsight/viberwhisper/commit/67467d1235cfc4f1e4c74253c398037a1c907fd7)
  的两个独立只读审查均完成、无新发现；[该轮 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42244262500)
  的 macOS 通过，Windows 构建通过，既有 DPI/编辑保留及十个滚动案例通过，随后新增提示区断言在 96 DPI 失败。
  原日志未记录具体场景、控件尺寸或哪一项条件失败，尚不能把它称为真实文字裁切或错误测试预期。
- **实现与取舍：** `preview/smoke.rs` 保持相同输入和全部条件，补记场景、系统色模式、测量高度、控件高度、
  正文/底部间隔、左右留白和最小高度；在几何断言前保存原生帧，避免失败现场丢失。
  `.github/workflows/ci.yml` 将既有捕获路径从单一 smoke step 提到 Windows job，让首轮 `Test` 失败时也能由既有 `always()` 上传步骤保留 PNG。
  不跳过测试、不放宽断言、不延长超时；不更改产品布局、颜色、滚动条或其他平台。没有增加诊断框架，故无新增机制消融。
- **边界与证据：** 此批只补足缺失证据，尚未验证根因或关闭原失败；需要新的 Windows 原生结果确认。
  第一轮完整日志在本 workspace `.amp/in/artifacts/ci-windows-macos-aligned-67467d12.log`。
  本机格式、共享预览协议测试、CI YAML 解析与 Windows GNU integration 类型/lint 检查的命令输出记录在
  `.amp/in/artifacts/footer-diagnostic-local-validation.log`；交叉检查仍没有 Windows 链接/运行。
  真实 4K、物理缩放、混合屏、输入法、系统文本放大和真实高对比度限制不变，覆盖层仍限测试。

### [014] — 2026-11-06 — 捕获路径保留在 step 作用域并验证 Actions 上下文

- **触发与决策：** [013] 本地诊断提交的双独立审查均完成，功能/安全报告一项 high：
  `jobs.<job_id>.env` 不支持 `runner` 上下文，移动后的 `${{ runner.temp }}` 将使工作流无效；复杂度无发现。
  此提交未推送。核对 [GitHub 官方可用性表](https://docs.github.com/en/actions/reference/workflows-and-actions/contexts#context-availability) 后，
  用户明确要求修复该项，采用两个现有测试 step 各自声明环境变量的最小方案，不新增全局初始化 step、脚本或 YAML 锚点。
- **实际实现：** `.github/workflows/ci.yml` 不再声明 job 级捕获变量；保留原 smoke step 的 `env`，在首轮 `Test` 同样设置路径并标注同步关系。
  两处都写入 Windows CI 的 `${{ runner.temp }}/windows-preview-captures/`，由既有 `windows-preview-captures` artifact 上传步骤收集。
  [013] 的原生尺寸日志、失败帧保存、全部输入及断言不变；没有修改产品窗口或正式启用细滚动条。
- **本机验证与复杂度比较：** 临时下载 actionlint 1.7.12 并核对发行版 SHA-256。
  同一 `actionlint -shellcheck= -pyflakes= -oneline <workflow>` 检查中，[013] 原稿在 job env 明确报 `context "runner" is not allowed here`、退出 1，
  修正后的完整工作流退出 0；此项验证 Actions 表达式作用域，超出先前 Ruby YAML 仅解析语法的范围。
  仅关闭未使用的外部 shellcheck/pyflakes 集成，不关闭 Actions 语法或表达式检查。
  格式检查通过；未改动的 Rust 诊断代码沿用 [013] 的共享预览协议与 Windows GNU integration 类型/lint 通过结果。
  日志在本 workspace `.amp/in/artifacts/footer-workflow-actionlint.log`；临时工具及输入在检查后清理，不安装到用户全局环境。
- **边界：** 本机确认了工作流作用域缺陷的前后差异；96-DPI 提示区原失败仍需 Windows 原生尺寸和截图确定原因。
  待按返修授权完成本次审查，再更新同一 PR 和精确 head 的 CI 订阅；不手动触发工作流，不将此检查称为 Windows 界面通过。

### [015] — 2026-11-06 — 原生证据确认提示区测试的增高前提错误

- **实际失败与证据：** [09486c68 的 Windows job](https://github.com/b1indsight/viberwhisper/actions/runs/42248010081/job/127995248827)
  在同一两行 warning 场景失败；新增日志确认实际 DPI=96，文字测量高 30 px、提示控件高 32 px，
  正文间隔和底部留白均为 12 px、左右留白均为 20 px。全部防裁切和间距条件成立，唯一不成立的是要求 warning 测量高度大于 32 px。
  取回并直接查看失败帧，正文与两行警告完整、互不覆盖，深色底和细圆角描边均存在；不能将此失败称为窗口文字裁切。
  原始输出在本 workspace `.amp/in/artifacts/ci-step-captures-09486c68.log`，原生 PNG 在同名目录。
- **最小修正与对照：** 保留全部四类输入、三档合成 DPI、局部系统配色覆盖，以及文字高度、间距和左右留白断言。
  只把“必须超过最小提示高度”的前提从两行 warning 改到已有三行 error 场景，并说明各自作用；不删除增长验证，
  不更改测量方式、固定期望像素、超时或产品布局。将三行文字错误地固定在 32 DIP 高度的实现仍会被现有防裁切断言拒绝。
  没有新增状态、辅助层或产品机制，故不另做机制消融；本次是修正已被原生证据推翻的测试前提，不把修改预期描述为产品修复。
- **本机检查：** 格式、共享预览协议回归及 Windows GNU integration 的 all-targets 类型/lint 检查通过；原始命令输出在
  `.amp/in/artifacts/footer-fixture-correction-validation.log`。GNU 检查仍用 `DOCS_RS=1`，没有 Windows 链接或执行。
  同一场景修正后的 Windows 原生执行、后续三行错误场景和测试专用滚动条实验尚待新 CI，不以本机通过代替。
- **验证边界：** 失败 head 的 macOS job 通过，Windows 已执行的 DPI/编辑保留及滚动恢复检查通过，后续原生阶段被该断言阻断。
  当前仅有自有 HWND 的 `WM_PRINT` 捕获，实际显示 DPI=96；合成 DPI 不是真实高 DPI 显示。
  细滚动条仍未在产品中启用，屏幕剪裁、真实拖动、UIA、真实 4K、混合屏、输入法及系统文本放大尚未验证。

### [016] — 2026-11-06 — 提示区与初步细滚动条原生验证通过，修正实验像素遍历 lint

- **原生结果：** [f4e08942 的 Windows job](https://github.com/b1indsight/viberwhisper/actions/runs/42249800890/job/128001562295)
  通过全量测试、原生设置/DPI、两模式 EXE smoke 和结果演示；macOS job 通过。
  warning 在合成 96/144/192 DPI 的实测高为 30/50/64 px，控件分别完整容纳；三行 error 的文字及控件高均为 45/75/96 px，
  保留间距与增长检查全部通过，关闭 [015] 的原生断言失败。局部系统配色回退也通过，不等于真实 OS 高对比度切换。
- **细条初步门禁及图像：** 测试专用探针在原生首行 0/7/25 检查槽/滑块几何、约 6 DIP 宽度、槽外像素不变、
  `WM_PRINT` 合成及实际 `WindowFromPoint` 槽边缘命中；短文隐藏和模拟系统色回退通过。
  已直接查看探针首尾位置与原生条基线图；实验图不再有整列浅色轨道，滑块随原生位置变化。
  正式窗口和演示仍使用原生条，屏幕真实剪裁、模态拖动重绘与 UIA 未验证前不启用覆盖层。
- **实际 CI 失败与最小修正：** 最后 Clippy 1.99 的 `chunks_exact_to_as_chunks` 拒绝新增实验中两处固定四字节分块，
  因此不能称整轮 Windows CI 通过。将像素对照循环改为 `as_chunks::<4>().0`，保留同一逐像素枚举、索引与槽外相等断言；
  不添加 lint 例外、不改 CI 工具链、不改产品窗口或测试输入。没有新增机制，故无额外消融或永久测试。
- **本机检查与证据：** 格式及 Windows GNU 的 `windows-app`、`windows-app,integration` 两组 all-targets 类型/lint 检查通过，
  输出在 `.amp/in/artifacts/scrollbar-probe-chunks-validation.log`。本机 Clippy 为 1.94，没有该新增 lint，最终仍需 CI 1.99 复验；
  GNU 检查使用 `DOCS_RS=1`，没有 Windows 链接/执行。原生日志在 `.amp/in/artifacts/ci-footer-fixture-f4e08942.log`，
  同名目录保存本轮 PNG；全部图片结构检查通过，前一轮共有图逐字节一致，输出在 `scrollbar-probe-native-capture-validation.log`。
  已核对默认短句、长文本警告、三行错误与局部系统色代表图；演示警告本来只有一行，未把错误的两行复核期望当成裁切缺陷。
- **交付与边界：** 此批仅修正测试工具的 lint，待同一门禁审查后推送原 PR 并订阅精确 head。
  全部图仍是自有 HWND 的原生 `WM_PRINT`，实际显示 DPI=96；真实 4K、物理缩放、混合屏、输入法与系统文本放大仍未验收。

### [017] — 2026-11-06 — 扩展独立原生阶段，验证屏幕、真实输入与 UIA

- **前序结果与本轮范围：** [07b14263 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42251899876)
  在 macOS 与 Windows 均通过，包括最终 Clippy；取回的全部 PNG 与 [016] 逐字节相同，代表细条图已再次查看。
  用户已要求继续原生验证并允许审查后推送同一 PR。本轮只扩展 [010] / [011] 已批准的剩余可行性检查，
  不将实验覆盖层接入产品，不修改向导、macOS、个人配置或此前长地址审查项。
- **职责与超时决策：** 原生测试父进程依次启动 setup/DPI 与 scrollbar 两个独立子阶段，各保持三十秒期限，避免同时争抢桌面输入。
  前者保留全部原输入与断言；后者使用新的自有气泡，不能继承合成 DPI 布局。Rust 在拥有窗口的线程泵消息，
  Windows PowerShell 5.1 私有客户端发送实际 OS 输入并查询 UIA；无须新增 crate 或 COM 接口实现。
  每次客户端有十秒期限；独立 watchdog 在超时/失败时释放左键与 Ctrl、取消原生跟踪并恢复指针，随后收集/终止客户端。
  外部客户端自行驱动拖动与捕获，模态跟踪中不依赖 Rust 主循环协调，也不在按住鼠标时发起 UIA 请求。
- **执行案例与独立对照：** 可编辑/只读分别先运行无覆盖层的系统条，再运行覆盖层，使用相同 HWND、文案与输入序列。
  验证客户区和槽边缘滚轮、PageDown、轨道点击，以及实际按住滑块经过中部、拖到首尾、释放、移出的过程；
  不发送伪造的 `WM_VSCROLL` 或鼠标按下消息。精确比较原生首行、选区与焦点/文字，防止只检查非零位移而漏掉重复滚动。
  终点期望独立读取 Win32 range/page，不从产品的视口辅助函数生成。
- **真实屏幕与可访问性：** 使用 `CopyFromScreen` 仅捕获自有气泡矩形，调用 DWM flush 后要求连续两帧满足槽位颜色、宽度和原生滑块纵向范围；
  不手工重绘覆盖层来制造成功。对照同阶段系统条 PNG 的槽外像素，限定在实际窗口 region，排除两次原生 caret 范围的闪烁差异。
  UIA 比较槽边缘/滑块与交互后的 EDIT 归属、原生滚动对象、支持模式及 ControlView 子对象，不能把额外装饰 Pane 当作兼容。
  原生条必须不能通过细条像素门禁，窗口边框点必须不能被判作 EDIT。高对比度环境或无法访问桌面会明确失败为未验证，不记为成功。
- **本机证据与复杂度：** 格式、macOS integration 测试和 Clippy、Windows GNU integration all-targets 类型/lint 检查已通过，
  原始输出在 `.amp/in/artifacts/scrollbar-desktop-macos-tests.log` 与 `scrollbar-desktop-windows-cross.log`；交叉检查不含 Windows 执行。
  临时便携 PowerShell 的完整语法解析及内嵌 C# 编译通过；仅在 macOS 检查结构大小，纯像素门禁接受正确输入并拒绝缺失、过宽、
  过期位置和浅色轨道四个故意错误。输出在 `scrollbar-desktop-powershell-validation.log`；这些纯检查不证明 Win32 ABI、屏幕、输入或 UIA。
  保留的产品实现没有增加机制；实验继续用原生/覆盖层对照确定是否可用，尚不能报告新 Windows 门禁或消融已通过。
- **剩余边界：** 新增客户端尚未在 Windows 运行，必须经本次审查、授权推送后取回新 CI 日志和屏幕帧再判断。
  同级覆盖层若未通过原生门禁，按 [010] / [011] 保留系统条并报告，不自动扩展为自定义滚动系统。
  本阶段限定实际 96 DPI 的托管桌面；合成 DPI、管理型 UIA 客户端和注入的 OS 输入不证明真实 4K、混合屏、Narrator、输入法或物理鼠标手感。

### [018] — 2026-11-06 — 系统条 UIA 基线失败与客户端代理初始化候选

- **触发与已观察结果：** [a815cd23 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42259974145)
  macOS 通过，Windows 在首轮 Test 失败；后续设置/EXE smoke、演示与最终 Clippy 未执行。
  setup/DPI、编辑状态及既有滚动/提示区检查完成。新的 `native-ready` 系统条阶段产生全部八个真实屏幕帧，
  走完滚轮、PageDown、轨道点击、按住拖经中部和首尾、释放与移出，并检查文字、焦点和原生首行；
  最后 UIA 要求失败：槽边缘/滑块虽归 EDIT 的 HWND，但类型为 Pane（50033），`Patterns=[]`，`AccessibleScrollbar=false`。
  此阶段 9.59 秒正常报告失败，不是外层三十秒超时；不能归因于此前审查 medium，亦不能称覆盖层输入/UIA 已通过。
  覆盖层桌面阶段和只读基线都尚未运行。
- **图像和原始证据：** 日志为 `.amp/in/artifacts/ci-desktop-input-a815cd23.log`，同名目录取回 PNG。
  全部图经 CRC/解压检查；共有图与前一 head 逐字节一致，新增八张 `probe-screen-native-ready-*` 为 `CopyFromScreen`。
  已查看初始、中部、底部、回顶帧，系统条位置与预期相符，文字无重叠/裁切；重复文案不能单靠截图识别逻辑首行。
  捕获检查输出在 `desktop-input-failure-capture-validation.log`，实际显示仍为 96 DPI，不是 4K。
- **诊断与决策：** 核对 native 控件消息路径，未吞 `WM_GETOBJECT`。参考当前 dotnet/wpf 的
  [ProxyManager](https://github.com/dotnet/wpf/blob/main/src/Microsoft.DotNet.Wpf/src/UIAutomation/UIAutomationClient/MS/Internal/Automation/ProxyManager.cs)
  及 UiaCoreApi：默认代理加载先清除重试标志，发现调用程序集时未保护动态帧的 `ReflectedType`，提供者回调吞掉异常。
  这可以产生所见的通用 HWND/Pane 回退；但未取得失败线程栈或本机 .NET Framework 4.8 对应实现，代理缺失/版本问题也尚未排除，
  因而只记为代理初始化假说。固定 Opus 5.5、无工具/MCP 的只读咨询支持最小候选，不是运行证据。
  不改 Rust COM 初始化、客户端 apartment 或产品可访问性实现，不假定多行 EDIT 必须报告 Edit 类型。
- **候选改动与复杂度：** 只改现有 `scrollbar_probe.ps1`，在其 C# 适配器中用具名托管调用者显式注册匹配 UIA 客户端强名的
  `UIAutomationClientsideProviders`，在任何 UIA 查询前执行，注册/加载异常直接失败；不在 PowerShell 动态帧内依赖隐式发现。
  使用实际客户端 DLL 作为编译引用，输出实际加载的客户端/代理名称、PS/CLR、位数和 apartment，并为比较补充控件类、类型及 framework。
  保留全部 UIA、屏幕、输入、负对照断言和原超时，不新增产品机制、库或配置。
  原始无显式注册版本的失败已捕获；候选在真实 Windows 的效果和消融仍待后续 CI，不能预报根因已确认或门禁已清除。
- **本机检查：** 格式、macOS integration 测试和 all-targets Clippy、Windows GNU integration all-targets 类型/lint 检查通过。
  PowerShell 7 完整解析及提取 C# 编译通过；临时具名 stub 程序集检查了客户端/代理名称映射、具名调用帧，以及注册异常不会被吞掉。
  初版 scratch 的 Add-Type 输出随机程序集名，导致假的加载失败，该次结果已作废；改为 Roslyn 显式命名 stub 后正负对照通过。
  输出在 `uia-proxy-local-validation.log`，原无效夹具输出单独保留；此检查未使用真实 UIA DLL，未执行 Win32、屏幕或输入。
- **边界与交付：** 本批是 CI 基线失败的最小客户端候选，仍需双独立审查、推送同一 PR 后读取 Windows 的实际程序集和 provider 结果。
  此前外层超时清理 medium 与 macOS 长地址项均未返修；产品和演示保留系统条，真实 4K、混合屏、IME、Narrator 和物理鼠标仍未验收。

### [019] — 2026-11-06 — 首个客户端超时的阶段诊断

- **新证据：** [e6c7b0ba 的 CI](https://github.com/b1indsight/viberwhisper/actions/runs/42263748418)
  macOS 通过、Windows 构建通过，首轮 Test 在首个 `native-ready` 桌面客户端的十秒期限失败；scrollbar 子阶段共 12.66 秒，
  未触发外层三十秒期限，后续 smoke、演示及最终 Clippy 未执行。Rust 已收集并打印子进程两个输出流，但没有初始化结果、UIA 指标或异常。
  取回的 53 张 PNG 均经 CRC/解压检查，与前轮共有图逐字节一致；没有真实屏幕帧，覆盖层桌面和只读案例未运行。
  原生日志为 `ci-uia-proxy-e6c7b0ba.log`，图片在同名目录，检查记录为 `uia-proxy-failure-capture-validation.log`。
- **决策与范围：** 尚不能区分 PowerShell 启动、程序集加载、C# 编译、代理注册或 UIA 查询的耗时/阻塞，不能确认 [018] 的假说或宣布修复。
  固定 Opus 5.5、无工具/MCP 的只读诊断也指出现有证据没有具体的注册挂起机制；其建议仅作为诊断参考，不是 Windows 执行。
  保留当前候选、全部断言、输入顺序、十秒客户端和三十秒阶段期限，不凭猜测延长期限、改 apartment、COM 或产品可访问性。
- **诊断改动：** 在原 `scrollbar_probe.ps1` 加直接写入并刷新 stderr 的阶段标记，绕过 PowerShell host 输出路径；
  包括首行到进程启动的耗时、脚本单调计时与 UTC，以及程序集加载、适配器编译、代理注册前后、第一次 UIA 查询、滚动条搜索和屏幕捕获前的边界。
  C# 注册前后使用同一进程 UTC，可与脚本时间关联；注册后的程序集加载以方法返回标记界定。不新增产品状态、依赖、配置、输入或等待。
- **本机验证：** 格式、macOS integration 全量测试及 all-targets Clippy 通过。PowerShell 7 完整解析、C# 5 语法编译通过；
  具名 stub 程序集的注册成功、故意异常与故意阻塞/终止对照通过：阻塞前的阶段行在重定向 stderr 中已可读，终止后没有伪造完成行。
  首两次临时夹具分别因根路径和引用数组类型错误作废，修正夹具后正负对照通过，产品候选未因此修改；输出分别保留在本机检查日志旁。
  记录为 `uia-stage-local-validation.log` 和 `uia-stage-macos-validation.log`；临时运行时、stub 与脚本随后清理。
  此检查未执行 Windows PowerShell 5.1、真实 UIA DLL、Win32、输入或屏幕，不证明下一轮一定能捕获标记或清除失败。
- **交付边界：** 这是定位 CI 失败所需的诊断改动，须双独立审查后沿用原 PR 推送，并读取新原生阶段证据再修正已验证的问题。
  正式预览与演示继续使用系统条；已有外层超时清理 medium 与 macOS 长地址项未返修，真实 4K、混合屏、IME、Narrator 和物理鼠标仍未验收。

### [020] — 2026-11-07 — 统一原生阶段期限并由父进程清理输入和进程树

- **触发与修订：** [23b7a22b 的 Windows 运行](https://github.com/b1indsight/viberwhisper/actions/runs/42266275473)
  记录代理注册返回，首次 UIA 查询前的进程年龄约 9.279 秒，首个客户端仍触发十秒期限。
  尚不能证明查询挂起，也没有在旧版本故意留下全局按键来复现外层清理风险。用户随后明确要求修复已报告的期限/输入清理项。
  本项替代 [017] 的四个独立十秒 watchdog 与三十秒外层预算，不修改 UIA 候选、原生输入/屏幕断言或产品可访问性。
- **单一监督与取舍：** `ui/setup/windows/tests/process.rs` 在不拥有窗口的测试父进程监督完整阶段；setup 保留三十秒，
  scrollbar 的四个顺序客户端共用九十秒，包含进程冷启动、编译、注册和查询；另有五秒清理区间。
  九十秒是根据现有单客户端初始化证据选择的保守验证预算，不是已经测得的完整四案上界或性能承诺。
  删除客户端内层期限、watchdog、先释放再终止和 UI 线程负责 kill 的路径，避免窗口阻塞同时阻断终止。
- **进程与输入所有权：** 父进程创建关闭即终止、禁止 breakaway 的 Job Object。子进程首先阻塞读取 stdin 的准入字节，
  父进程分配并用 `IsProcessInJob` 确认归属后才允许 DPI/UI/驱动启动；父进程在分配前退出则 stdin EOF 直接失败。
  未采用具名事件，减少命名、额外句柄和父进程死亡等待协议；仅为 Windows 测试启用已有 windows-sys 的 Security/JobObjects 功能。
  正常退出、失败、超时和 unwind 都清理所属进程；清理检查 job 的 active count 为零且 Rust 子进程已退出，
  再发送左键/Ctrl release，恢复物理指针位置并检查实际键态和位置。终止请求、输入或退出无法确认均不能算通过；
  异常出口仍尝试释放，但明确保留失败。整个清理不依赖目标消息循环。
- **输出与故障对照：** PowerShell 的两个输出流直接继承 Rust 子进程句柄，父进程并发读取，退出后在同一清理期限收集，
  不再等 UI 线程返回才取得已刷新的阶段行。新增串行准入拒绝对照及清理故障阶段：先延迟准入两秒检查没有自有窗口、
  job 成员或按键；再让驱动在原生滑块上按住 Ctrl/左键并持续发送，父进程必须观察真实 capture、两个键态和至少两个 job 成员，
  然后走同一终止/清理路径。未看到故障前提不能伪报故障验证成功；正常四案和所有 UIA/像素/负对照保持原样。
- **本机检查与机制对照：** 格式、macOS 构建、integration 测试和 all-targets Clippy，以及 Windows GNU 两种 feature 组合的
  all-targets 类型/lint 检查通过；GNU 仍用 `DOCS_RS=1`，不包含 Windows 链接或执行。
  PowerShell 7.5.4 完整解析、全量适配器 C# 5 语法编译及最小 UIA compile stub 检查通过，未调用 Win32。
  临时程序原样提取准入语句和排空函数，检查延迟准入、EOF、错误字节，以及 1,048,593 / 2,097,183 字节的非对称双流输出；
  正确实现收集完整输出，移除两个读取线程的单机制对照确实阻塞，随后已安全终止/收集。
  原始记录为 `native-supervision-{macos-validation,windows-cross,client-validation,portable-io}.log`，
  执行前的固定案例在 `native-supervision-proof-cases.md`。这些只证明本机可移植 I/O 与静态/编译检查，
  不能作为 Win32 job、按住输入、UIA 或屏幕的运行证据；资源监督与原生故障控制仍须后续 Windows CI。
- **咨询及交付边界：** 只读咨询实际模型为 Opus 5.5，tools/MCP 均为空；采纳更小的 stdin 准入建议，未将咨询当运行证明。
  沿用原草稿 PR，待双独立审查后按已授权流程推送并读取原生结果。正式窗口和演示仍使用系统条；
  只有可编辑/只读系统条与覆盖层的原生屏幕、实际输入和 UIA 门禁全部通过，才考虑已批准的产品接入。
  macOS 长地址项未修改，真实 4K、混合屏、IME、Narrator、系统文本缩放和物理鼠标手感仍未验收。

### [021] — 2026-11-07 — 原生清理门禁通过；无窗口合成层候选（待批准）

- **实际结果：** [020] 的 [Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42361741658/job/128371391892)
  通过构建、准入拒绝、设置窗口以及按住原生滑块和 Ctrl 后的故障清理。父进程实际观察自有 capture 与存活驱动，
  终止后确认所属进程为零、按键释放和指针恢复。系统条可编辑案例完整通过真实输入和 UIA 对照；
  细条案例通过空闲、悬停、按下和拖至中段，拖到底时实际屏幕出现原生浅色轨道并触发像素断言，未触发阶段期限。
  初始 UIA ControlView 还多出一个 Pane，最终完整指标比较尚未执行；只读两案及后续 smoke、演示和最终 Clippy 均未运行。
  macOS CI 通过，不能据此称整轮 CI 或细条方案通过。
- **证据与结论边界：** 已解码并查看本轮中段、末端和系统条实际屏幕；末端槽位的 `#f0f0f0`、`#858585` 与系统条一致，
  预期 `#c7c7c7` 按下滑块已被覆盖。此前 `WM_PRINT` 合成正常不证明真实桌面正常。
  尚未定位具体原生绘制调用或证明 `WS_EX_TRANSPARENT` 是唯一原因；不以冷启动变快推导性能改善。
  原始记录为 `ci-native-supervision-f1322977.log`、同名截图目录和 `native-supervision-f1322977-capture-inspection.log`；
  后续执行前固定案例在 `scrollbar-drag-composition-cases.md`，均位于工作区 `.amp/in/artifacts/`。
- **当前决策：** 按 [010] 的可行性门禁保留产品及演示的原生系统条，不绕过屏幕或 UIA 断言。
  本项未修改滚动条实现、期限、UIA 指标或输入逻辑；[020] 的清理修复已有本次原生证据，细条失败仍未关闭。
  给现有 HWND 叠加样式不能直接消除已观察到的额外节点；不采用挪动节点、过滤指标、定时补画、未公开主题或自定义滚动模型来制造通过。
- **范围修订建议（待批准，尚未实施）：** 用不创建额外 HWND 的 DirectComposition 视觉层替代测试专用同级窗口覆盖层。
  在预览自身的顶层 HWND 上合成槽位外观，仍以 `GetScrollBarInfo` 为唯一几何来源，保留原生 `WS_VSCROLL`、命中范围、
  拖动、滚轮、键盘、焦点和 UIA 对象。该候选需要 DirectComposition / DXGI / D3D11 及 Windows 专用 COM 绑定，
  超出现有纯 USER32/GDI 覆盖层的实现范围，因此先请求明确批准，不直接引入依赖或图形设备。
  无额外 HWND 有望避开新增节点，但具体合成、GDI surface 兼容性和真实桌面结果仍是待验证假设。
- **批准后的有界实验与验收：** 先只替换现有测试探针的绘制职责，不启用产品路径或新建通用渲染框架。
  原生 EDIT 继续绘制，其上只合成物理槽位，处理裁剪、布局/DPI、短文本隐藏、系统色回退及释放；初始化、提交或设备失败均不能伪报成功。
  保留 `WM_PRINT` 合成与全部既有真实屏幕、首行、选区、文本、焦点、支持模式和完整 ControlView 对照，不过滤多余节点。
  新增创建前后子窗口及线程顶层窗口列表不变的检查，区分无 HWND 合成与另开弹窗。
  在取得原生窗口 DC 并故意改写槽位、不给探针补画消息的对照中，先证明底层写入实际发生，再检查合成层仍可见；
  若 Win32 DC/合成读回不稳定则保留不确定性，不将该对照标为通过。
  四个系统条/细条、可编辑/只读案例必须完整运行，检查按住拖至首尾和释放/移开后的实际屏幕，继续使用相同输入与期望值。
  全部通过后才按原门禁考虑接入产品；任何关键能力失败则继续保留原生，不自动扩展为新的滚动或无障碍实现。
- **咨询与交付：** 本轮只读咨询的初始化和响应均为 Opus 5.5，tools/MCP 为空；建议不是 Windows 执行或 API 兼容性保证。
  咨询结果在 `scrollbar-drag-design-advice.md`。本项仅记录运行结果与待批准修订，不新增代码提交或推送，沿用原 PR 和 bookmark。
  真实 4K、混合屏、IME、Narrator、系统文本缩放、物理鼠标以及此前 macOS 长地址项的边界不变。

### [022] — 2026-11-07 — 同一 EDIT HWND 内自绘（方向已选，实施方案待批准）

- **请求与替代范围：** 用户在了解同窗口自绘与覆盖窗的差别后要求“用同一个窗口自绘”。本项改用现有 EDIT HWND 内的
  USER32/GDI 绘制，替代 [010]/[011] 的额外装饰窗口方向和 [021] 未批准的 DirectComposition 候选；历史失败证据保留。
  不删除已有预览、EDIT 或 STATIC 控件，不新增 child、sibling、popup 或 owned HWND，不引入新 crate 或图形设备。
  本条是具体实施方案，仍按 plan-first 在明确批准后才新增测试或实现；当前仅修订文档。
- **保留契约：** 正文编辑和滚动继续由原生 EDIT 负责，保留 `WS_VSCROLL`、实际槽宽、鼠标命中/capture/拖动、滚轮、
  键盘、轨道与箭头区域点击、选区、撤销、焦点、IME/确认键协议和 UIA。只改变槽位像素，不另建行数、页高、拖动或无障碍模型。
  沿用 [011] 的深色背景与约 6 DIP 圆角滑块，普通/悬停/按下颜色为 `#929292` / `#b0b0b0` / `#c7c7c7`；
  箭头仅视觉隐藏，原生命中区不缩小。高对比度走系统色与原生条，短文本继续收起滚动条；原生几何保持物理像素，只缩放外观常量。
- **已知依据与首要未知：** EDIT 创建时没有额外边框，当前非客户区滚动条来自 `WS_VSCROLL`。
  [WM_NCPAINT](https://learn.microsoft.com/en-us/windows/win32/gdi/wm-ncpaint) 支持自绘窗口框架，
  [GetDCEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getdcex) 支持取得窗口 DC 和显式裁剪；
  这些文档没有保证模态 thumb tracking 的所有原生绘制都经由可接管消息。[021] 也未定位具体覆盖调用。
  因而先验证公开、非轮询的消息路径是否足够；不能直接将只处理 `WM_NCPAINT` 或默认处理后补画称为完整修复。
- **批准后的最小实施顺序：**
  1. 先在 `preview/scrollbar_probe.rs` 替换测试专用覆盖窗，去掉类注册、额外 HWND、z 序定位、`WS_CLIPSIBLINGS` 修改及
     顶层窗口为覆盖窗补做的 `WM_PRINT` 合成。保留现有自有窗口捕获、原生几何和输入/UIA 驱动。
     在同一 EDIT 的局部 subclass 中试验非客户区绘制；不在 Win32 调用期间持有可变借用，不接管原生鼠标/滚动消息。
  2. 用一小段测试专用轨迹记录 EDIT 与既有父窗口相关消息的进入/退出、原生 capture、槽位/滑块几何和本方绘制序号，
     对照按住拖至中段、首尾和释放后的真实屏幕；包含客户区鼠标消息和 capture 变化，不能只观察非客户区鼠标移动。
     使用既有输出通道，在案例结束或失败时输出，不引入追踪框架、逐消息 BitBlt、定时器、轮询补画或第二套期限。
     消息/截图相关性只能缩小调查范围，不能据此声称定位了具体内部绘制调用或排除了所有瞬时帧。
  3. 只有公开消息路径在实际首尾拖动中足以保持目标外观，才完善同窗口绘制的裁剪、状态切换、DPI、短文本隐藏和释放。
     绘制只覆盖 `GetScrollBarInfo` 返回的槽位；窗口 DC 使用明确坐标转换和 `SaveDC`/`RestoreDC`，按原线程成对释放。
     `WM_PRINT` 继续验证自有控件绘制，但不再代替真实屏幕验收。不得靠冻结 EDIT、隐藏原生条、未公开消息/API detour 或反复补画绕过问题。
  4. 测试探针的全部原生门禁通过后，才将同一已验证的绘制职责接入 `preview.rs` 的现有控件消息处理和生命周期，
     避免复制第二套绘制/滚动逻辑；复跑产品/演示与所有原生门禁后再报告接入完成。
     不修改 setup、macOS、字体/布局策略、此前长地址项、配置或发布流程，不新建通用渲染框架。
- **执行前固定的验收与对照：** 保留 [021] 所引固定案例中的四个系统条/候选、可编辑/只读客户端和全部外观目标。
  同一真实输入序列检查 idle、hover、pressed、按住拖至中段/底/顶、释放及移开；原失败的按住末端必须通过连续屏幕断言，
  不放宽颜色、宽度、槽外像素、文本、选区、焦点、原生首行、支持模式或完整 ControlView 相等要求，不过滤额外节点。
  增加原生箭头区域按住/释放对照，与系统条执行相同输入，检查单步方向、capture、连续滚动、释放后停止及末端夹取；
  连续重复次数只记录，不用计时假设硬编码或要求跨案相等。旧 `HTTRANSPARENT` 断言改为 EDIT 原生 `HTVSCROLL`/`WindowFromPoint` 命中，
  同时检查安装前后子窗口和线程顶层窗口列表不变；这是窗口归属变化所需的断言替换，不删除输入或 UIA 门禁。
  保留 DPI 回程/重入、收缩/尾随、选区/撤销、系统色分支、短文本隐藏及父进程准入/按住输入故障清理。
  原生系统条继续作为同输入负对照；必要时撤去候选绘制验证屏幕门禁确能失败，不保留无实际作用的诊断机制。
  不沿用 [021] 专用于独立合成表面的“外部 DC 写入免疫”要求：本候选与原生共享 GDI 表面，没有该隔离性质。
- **停止条件与后果：** 若原生 tracking 绕过可用的公开绘制路径、仍露出系统轨道、仅能事后补画或任一输入/UIA 门禁失败，
  保留产品和演示的系统条并报告实际限制；不自动切换 DirectComposition、扩展自定义拖动/无障碍、放宽断言或延长期限。
  不能由一次稳态截图或消息轨迹推断绝无闪烁。失败时若需放宽上述行为约束，须另行说明并获得明确批准。
- **验证与交付：** 本机继续执行格式、macOS 构建/integration 测试/Clippy 及 Windows GNU 两组 feature 的类型/lint 检查，
  GNU 的 `DOCS_RS=1` 仍不包含 Windows 链接或运行。审查后沿用原 bookmark/PR 推送，让现有 Windows CI 执行原生案例、
  smoke、演示及最终 Clippy，取回并逐项查看实际截图；不手动 dispatch 工作流或发布。
  首轮原生证据仍只覆盖 hosted 96 DPI；真实 4K、物理 150/200% 缩放、混合屏、IME、Narrator、系统文字缩放和物理鼠标手感保留未验收标记。
  方案咨询的初始化与回复已核对为 Opus 5.5、tools/MCP 均为空；只采纳可行性前置与共享表面边界，不采纳逐消息像素追踪、
  用采样推断“全部写入已抑制”或任意固定重跑次数。咨询不是 Win32 运行证据，草稿位于工作区 `.amp/in/artifacts/scrollbar-same-window-plan-advice.md`。

### [023] — 2026-11-07 — 同一 EDIT 非客户区绘制探针与箭头输入对照

- **触发与范围：** 用户明确要求“实现”，按 [022] 执行。同窗口候选先落在现有测试专用 `scrollbar_probe.rs`，
  不提前修改 `preview.rs`、产品或演示；原覆盖窗的失败证据和 [022] 的停止条件继续保留。
- **实际实现：** 删除覆盖窗类、额外 HWND、z 序定位、`WS_CLIPSIBLINGS` 修改和父窗口 `WM_PRINT` 手动合成。
  现有 EDIT 的局部 subclass 仅在 `WM_NCPAINT` 内用 `GetDCEx` 绘制原生槽位，使用窗口坐标转换、显式裁剪及成对的
  `SaveDC`/`RestoreDC`、`ReleaseDC`；`WM_PRINT` 把原生客户区和本方非客户区分开绘制。
  保留原生 `WS_VSCROLL`、命中、capture/tracking、所有滚动/编辑消息和完整 UIA；不处理第二套滚动或选区模型。
  安装、悬停边界和按下/释放的视觉状态变化才请求重绘，不在滚动/绘制消息默认处理后追加补画，不设置重绘定时器。
  EDIT/父窗口的相关消息进入/退出、原生 capture、物理槽位/滑块几何及本方绘制计数缓存在测试局部，案例结束或失败时经原通道输出。
  轨迹不包含逐消息像素读回，也不能证明不存在未采样的瞬时帧或定位具体原生绘制调用。
- **先固定案例再实现：** 工作区 `scrollbar-drag-composition-cases.md` 在代码修改前追加 [022] 的案例。
  保留四个系统条/候选、可编辑/只读客户端及既有真实屏幕、槽外像素、首行、文本、选区、焦点和完整 ControlView 比较。
  旧覆盖窗定位/`HTTRANSPARENT` 断言由无新增 child/线程顶层 HWND、EDIT style 不变及 EDIT 原生 `HTVSCROLL`/`WindowFromPoint` 替代。
  增加上下箭头区域的真实单击、按住自动重复、内部位置释放后停止和近末端按住夹取；重复次数仅记录，单击必须精确一行。
  末端按住/释放的实际屏幕继续按相同颜色、宽度和槽外像素验收，不比较不同时刻的重复次数或中途正文位置。
  原有 `WM_PRINT`、系统色分支、短文本隐藏与父进程准入/故障清理仍在；不延长九十秒或五秒期限。
- **本机检查：** `cargo fmt --check`、macOS integration 构建/测试/全目标 Clippy、Windows GNU 的 `windows-app` 和
  `windows-app,integration` 两组全目标 Clippy 通过。GNU 使用 `DOCS_RS=1`，不含 Windows 链接或运行。
  从官方归档校验 SHA-256 后临时运行 PowerShell 7.6.9，完整驱动脚本解析通过；这不是 Windows PowerShell 5.1、Win32 ABI、输入或 UIA 执行。
  原始输出在工作区 `.amp/in/artifacts/same-edit-macos-checks.log`、`same-edit-windows-cross.log` 和 `same-edit-powershell-parse.log`。
- **消融与未验收：** 去掉候选绘制的原生系统条继续作为同输入负对照，保留像素门禁必须拒绝原生外观的要求。
  本机不能执行该 Windows 对照，尚无同窗口运行证据；因此候选仅作为测试探针提交，不把未经比较的机制接入产品。
  仍需审查后推送同一 PR，由 Windows CI 执行实际屏幕和输入门禁并取回截图；任一关键门禁失败就保留产品系统条并报告。
  真实 4K、物理缩放、混合屏、IME、Narrator、系统文字缩放及物理鼠标边界不变。

### [024] — 2026-11-07 — 同窗口候选悬停屏幕失败，停止产品接入

- **实际执行：** 已审查的 [同窗口探针提交](https://github.com/b1indsight/viberwhisper/commit/29293477c3275889fb933b3006a2b1c809e1537d)
  在 [Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42370708522/job/128398049945) 构建通过、Test 失败；
  macOS CI 通过。失败来自候选 `ready` 的真实屏幕 hover 断言，不是原生对照或阶段期限。
  原生可编辑对照完整通过滚轮、PageDown、轨道点击、按住拖至中段/首尾、释放、箭头单步/重复/夹取和最终输入/UIA 检查。
  准入拒绝、设置窗口和按住 Ctrl/左键后的故障终止检查仍通过，日志确认所属进程为零、输入释放和指针恢复。
- **候选证据：** 同窗口安装/移除的 HWND、style、原生命中、WM_PRINT 槽宽/槽外像素、系统色分支和短文本隐藏检查通过。
  桌面候选初始 ControlView 只有原 EDIT，滚动条可访问、支持模式及边缘/滑块 UIA 归属与原生初始值一致；
  这没有完成最终全文本、选区、焦点和完整 UIA 对照。候选空闲帧为深色槽位与六像素 `#929292` 滑块，
  随后的滚轮、键盘和轨道点击后，hover 帧恢复为 `#f0f0f0` 轨道和系统箭头/滑块，未保持预期 `#b0b0b0` 细条。
  取回的 PNG 已逐一校验 CRC 并解码；hover 槽位颜色分布与本轮系统条负对照一致，不能把失败解释为仅箭头或细条色差。
- **消息边界：** 轨迹记录到滚动后的原生几何变化和 WM_PAINT，未伴随新的 WM_NCPAINT，绘制计数保持不变；
  对应候选实际屏幕随后未通过。当前仅接管 WM_NCPAINT 并在普通视觉状态边界失效的路径不足以维持本次交互的目标外观。
  尚未定位具体内部绘制调用，也没有执行到本轮候选按住拖至末端；不把本次失败推广为所有同 HWND 自绘都不可能，
  不用初始截图、WM_PRINT 或消息采样推断没有瞬时系统帧。
- **决定与后果：** 按 [022] 的停止条件保留产品及演示的系统条，不接入本轮候选、不放宽断言、不延长期限，
  不增加事后反复补画、隐藏原生条、自定义输入/UIA 或 DirectComposition。候选最终对照和只读两案未执行，
  后续原生设置/DPI smoke、演示及最终 Clippy 被 Test 失败跳过；草稿 PR 尚未通过本轮验证，不能报告完整实现已完成。
  如需改变保留原生滚动行为等约束，须先提出修订并获得明确批准；本项只记录结果，不新增产品或测试逻辑。
- **原始证据与限制：** 工作区 `.amp/in/artifacts/ci-same-edit-29293477-failed.log` 保存失败及完整消息轨迹，
  `ci-same-edit-29293477/` 保存本轮 PNG，`same-edit-29293477-capture-inspection.log` 保存解码及槽位像素检查。
  实际 hosted 显示为 96 DPI；真实 4K、物理缩放、混合屏、IME、Narrator、系统文字缩放和物理鼠标手感仍未验收。

### [025] — 2026-11-08 — DirectComposition 有界实验方案修订（实施待批准）

- **背景与替代范围：** [024] 已触发 [022] 的停止条件。用户接受按检索结果修订 DirectComposition 方向，
  本项重新提出 [021] 的独立合成候选，不倒改其历史审批状态，也不将检索或咨询当作运行证明。
  仅在测试实验中替代 [022] 的共享 GDI 表面和禁止图形设备/绑定依赖的限制，并恢复独立合成表面的底层 DC 覆写对照；
  其余原生行为和门禁继续保留。详细实施须获得明确批准，不在本次方案修订中修改测试、依赖或产品实现。
- **职责边界：** 在现有气泡父 HWND 上挂接视觉层，不新增 child、sibling、popup 或 owned 装饰窗口。
  EDIT 继续拥有 `WS_VSCROLL`、物理槽宽、命中/capture/模态拖动、滚轮、键盘、轨道与箭头重复点击、选区、撤销、只读和 UIA。
  `GetScrollBarInfo` 仍是槽位及滑块几何来源；只把槽位映射到父窗口客户区并裁剪合成内容，不另建滚动或无障碍模型。
  六 DIP 圆角滑块和三态颜色沿用 [022]；箭头仅视觉隐藏，实际点击范围不缩小，不宣称具有 macOS 的全部交互语义。
- **文档依据与未知：** [CreateTargetForHwnd](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-createtargetforhwnd)
  支持绑定已有同进程 HWND，并将视觉层置于直接 HDC 内容和子窗口之上；[根坐标与提交](https://learn.microsoft.com/en-us/windows/win32/directcomp/basic-concepts)
  以目标客户区为准。因此候选目标是已有父窗口，不假定 EDIT 客户区的合成能覆盖其非客户区。
  层叠契约不能证明原生 tracking 内的几何同步、实际捕获、UIA 或设备生命周期正确；这些均为前置未知。
- **批准后的最小顺序与文件范围：**
  1. 先核对公开 API 的具体设备、surface/content 和绘制路径及官方 COM 绑定，不预设任意 DComp surface 都支持 `GetDC`，
     不手写 vtable。依赖只放 `Cargo.toml` 的 Windows target dev-dependencies，并同步必要的 `Cargo.lock` 变化；
     优先核对锁文件已有的 `windows` 绑定是否足够，仅开启实际需要的 DComp、D3D11、DXGI/绘图接口，不顺带升级依赖。
     保持实验代码只由 `smoke.rs` 的 `cfg(test)` 路径加载，核对普通构建不引入实验图形代码。
  2. 在 `preview/scrollbar_probe.rs` 替换失败候选的绘制职责，复用现有 guard、EDIT/父窗口观察、输入驱动和生命周期监督。
     **先验证合成层叠与原生几何同步两项：** 最小可辨认槽位/滑块必须在真实滚轮、PageDown、轨道/箭头和按住拖动中，
     根据原生几何正确更新；不能只盖住系统条却保留静止滑块，或等模态调用返回后才更新。
     更新入口仅用有文档依据的非轮询通知，并以实际轨迹核对；不预设每次拖动都经过 `WM_NCPAINT` 或外层 subclass 返回。
     如需公开原生事件观察，必须限定自有进程/线程，不引入跨应用监听。没有可靠入口或公开绘图路径即停止。
  3. 两项前置能力通过后，才完善相同外观、布局/DPI 映射、短文本隐藏、系统色回退和失败清理。
     视觉层只覆盖原生槽位，不能遮住正文、警告或抢焦点；几何保持物理像素，只缩放外观常量。
     提交/设备错误必须报告失败，guard 在 HWND 销毁前解绑并释放资源，清理后恢复系统条；不引入设备恢复框架。
  4. 保留 `WM_PRINT` 的自有窗口离屏几何/像素检查；若需显式绘入同一视觉内容，明确它不是实际合成器读回。
     真实屏幕仍由现有 `scrollbar_probe.ps1` 捕获，不以离屏图代替；只在确有需要时补充该驱动的证据输出。
     不另建渲染框架、第二套输入驱动、监督进程或期限，不修改 `preview.rs`、setup、macOS、字体/布局策略、配置或发布流程。
- **验收与负对照：**
  - 原生/候选 × 可编辑/只读四案必须完整执行。保留 [022] 的 idle、hover、pressed、按住拖至中/底/顶、释放和移出，
    箭头单步/重复/停止/夹取、文本、选区、焦点、首行、支持模式、完整 ControlView 和槽外像素断言，不过滤额外节点。
    安装与移除前后子窗口及线程顶层窗口列表、EDIT style、`HTVSCROLL`/`WindowFromPoint` 归属保持不变。
  - 恢复 [021] 的底层 DC 覆写对照：先独立证明自有槽位下层的写入确实发生，再不给候选补画通知而检查屏幕合成仍保持。
    写入无法证实、读回有歧义或图像不稳定均不能称为免疫；帧采样只能证明被观察帧，不能保证没有任何瞬时闪烁。
  - 移除合成层后必须恢复原生外观；原生同输入负对照必须继续被细条像素门禁拒绝，避免把捕获不到合成内容误报为通过。
    保留既有 DPI 回程/重入、收缩/尾随、系统色/隐藏和父进程准入/按住输入故障清理；不改变九十秒阶段与五秒清理预算。
- **停止、验证与交付：** 任一层叠、同步、输入、完整 UIA、实际屏幕或释放门禁失败就停止，产品和演示继续使用系统条。
  不通过隐藏原生条、API detour、私有主题/消息、定时器补画、自定义拖动/provider、放宽断言或延长期限兜底。
  本机格式、macOS 构建/integration 测试/Clippy、Windows GNU 两组 feature 的类型/lint 检查不能替代 Windows 执行；
  审查后沿用原 bookmark/PR，由既有 Windows CI 运行原生门禁并取回、查看实际截图，不手动 dispatch 或发布。
  本项即使成功也只提供测试证据，不自动授权接入产品；须先报告完整结果，再单独决定产品接入。
  首轮 hosted 96 DPI 与合成 DPI 通知分别标注，真实 4K、物理缩放、混合屏、IME、Narrator、文字缩放和物理鼠标仍待实机。
- **咨询边界：** 方案草稿经固定 Opus 5.5、无工具/MCP 的只读咨询，初始化、响应和 usage 模型均已核对，结果成功。
  仅采纳有界实验和双前置门禁，不把模型建议当作 API 或运行保证；实际顺序编号按现有记录续为 [025]。
  脱敏草稿与边界检查位于工作区 `.amp/in/artifacts/scrollbar-dcomp-revised-plan-advice.md` 和同前缀的 `validation.json`，
  原始事件流已清理。当前只有方案/结果记录变化，未新增实现或原生运行；沿用同一草稿 PR，不新增 PR 或合并。

### [026] — 2026-11-08 — 父 HWND 合成槽位实验实现，原生可行性待验证

- **触发与范围：** 用户要求“先实现”后，按 [025] 仅替换 `preview/scrollbar_probe.rs` 的测试候选绘制，
  并扩展现有 `scrollbar_probe.ps1` 的证据；普通预览、演示、设置、macOS、工作流与进程监督未改动。
  保留 [024] 和方案提交两次重试的 GDI 按下态失败，不把本机编译或 API 文档当作修复证明。
- **绘制取舍：** 使用锁文件已有的官方 `windows = 0.54.0` 绑定，仅增加 Windows target dev-dependency 的实际图形 feature，
  不升级依赖或手写 COM。WARP D3D11 经 DXGI / D2D 设备创建 DComp，视觉层绑定已有气泡父 HWND、置于子控件上方，
  surface 与裁剪只覆盖映射到父客户区的原生槽位。`BeginDraw<ID2D1DeviceContext>` 尊重返回的绘制偏移，
  仅调用 surface 的 `EndDraw`，不假定 surface 支持 GDI `GetDC`。
  几何使用物理像素，只缩放六 DIP 外观宽度；沿用三态颜色，不增加输入或 UIA 模型。
- **同步与生命周期：** 原生 EDIT 的所有屏幕绘制和输入照常执行；以包括嵌套 `WM_VSCROLL/SB_THUMBTRACK` 在内的
  公开滚动、布局和状态通知读取 `GetScrollBarInfo`，不从消息的十六位位置推算。缓存几何/颜色，相同帧不提交，
  不增加轮询或事后反复补画。回调中的设备/提交错误记录为实验失败；显式关闭在 HWND 销毁前移除观察器、
  解绑 root、提交并等待完成，Drop 保留失败清理。guard 保持装箱地址稳定，避免移除 subclass 前移动其回调引用。
  `WM_PRINT` 仍手动 GDI 绘入相同几何，明确只是离屏检查，不是合成器读回。
- **固定门禁与消融控制：** 原生/候选 × 可编辑/只读四案及既有输入、完整 UIA、命中、HWND/style、槽外像素、
  隐藏/系统色和清理断言保留，期限不变；新增滚轮、PageDown 和轨道后的真实屏幕帧。
  候选空闲后，客户端先通过自有 EDIT 的 DC 写入品红并逐像素读回，证明下层写入发生，再不给补画请求检查合成屏幕。
  读回有歧义直接失败。每次移除合成层后，额外捕获恢复帧，要求细条门禁拒绝并与原生基线逐像素相同，
  不能把品红残留或未捕获到合成内容当作恢复成功。只忽略原有光标闪烁区域。
- **本机执行：** `cargo fmt --check`、macOS integration 构建/测试/all-targets Clippy，以及
  `DOCS_RS=1` 的 Windows GNU `windows-app` / `windows-app,integration` all-targets Clippy 通过。
  normal 依赖树中 `windows` 仍仅来自已有 `cpal` 路径；实验模块仍仅由 `smoke.rs` 的 `cfg(test)` 加载。
  PowerShell 7.6.9 完整解析驱动、编译不含 Windows UIA 初始化的 C# 适配器，并验证三态纯像素正负对照；
  未执行任何 Win32、DComp、UIA、真实屏幕或鼠标调用，也未验证 Windows PowerShell 5.1。
- **未决证据与交付：** 本机没有 Windows 图形环境，无法在审查前完成候选/无合成/移除合成的真实比较；
  这些对照仍是同一既有 CI 的前置验收，设备路径、层叠保护和模态同步尚未证实。
  因而实现只保留为测试实验，未接入产品；任一关键门禁失败仍按 [025] 停止，不更换架构、放宽断言或延长期限。
  本机输出位于 `.amp/in/artifacts/dcomp-{macos,windows}-*.log` 与 `dcomp-powershell-validation.log`，
  固定验收记录为 `scrollbar-drag-composition-cases.md`。真实 4K、物理缩放、混合屏、IME、Narrator、文字缩放和物理鼠标仍待实机。

### [027] — 2026-11-08 — 合成候选按下态失败，原生截图及捕获轨迹复核

- **本轮运行：** [实验提交 8c34b024](https://github.com/b1indsight/viberwhisper/commit/8c34b0240447b30e41e2b3a56889d4d7477ae53a)
  已经两项独立只读审查并推送原 PR；[CI](https://github.com/b1indsight/viberwhisper/actions/runs/42463410264)
  的 macOS 通过，Windows 构建通过、Test 失败。实际 checkout 为该 head 与原基线的 PR 测试快照；
  运行环境为 Windows Server 2025、PowerShell 5.1、CLR 4、x64/STA，实际显示为 96 DPI。
- **已执行对照：** 准入拒绝、设置窗口、按住 Ctrl/左键后故障终止的清理均符合预期；
  后者确认所属进程为零、输入释放、指针恢复。系统条可编辑案例完整通过滚轮、PageDown、轨道、
  按住拖至中段/底部/顶部、箭头单步/重复/停止/夹取及最终输入/UIA。候选初始 UIA 的 ControlView
  只有原 EDIT，支持模式和槽位/滑块归属与原生初始值相同，不等于最终完整对照已通过。
- **合成屏幕与部分滚动：** 候选 idle、underlay、wheel-client、wheel-slot、page-down、track、hover
  实际屏幕门禁均通过；两处滚轮首行均为 3，PageDown/轨道均为 8，与系统条观测一致。
  下层 DC 的 3332 个品红像素先被独立逐像素读回，随后屏幕槽位与 idle 无像素差异，轨迹提交计数仍为 1，
  因而本次观测帧证明了无候选补画请求的下层覆写隔离；不保证没有任何未采样的瞬时闪烁。
- **确切失败与已定位状态错误：** pressed 帧仍只有深色背景和悬停色 `#b0b0b0`，与 hover 的 PNG
  及解码 RGB 完全相同；预期按下色 `#c7c7c7` 像素为零，没有露出浅色系统条或品红。
  轨迹中 `WM_NCLBUTTONDOWN` 已设置按下态并提交；随后 `WM_CAPTURECHANGED` 进入/退出时，
  `GetCapture()` 都仍为 EDIT 的 `0x60046`，但 `pressed` 从 true 变 false，提交从 12 变 13。
  当前观察器无条件在该消息后清除按下态，解释了这次实际颜色失败；没有定位 Windows 内部产生该消息的调用。
  客户端约 6.62 秒以真实断言非零退出，未触发九十秒阶段期限，不将其解释为基础设施超时。
- **停止与未覆盖：** 按 [025] 停止本轮实验接入，产品及演示仍用系统条；不自动返修、重跑、
  放宽断言、延长期限或更换架构。本轮候选拖至中段/首尾、释放/移出、箭头重复、最终完整 UIA、
  移除后的原生像素对照，以及原生只读/候选只读两案均未执行。后续设置/DPI smoke、演示和最终 Clippy
  被 Test 失败跳过；此前其他 head 的通过不作为本轮成功证据。本次仅追加本地结果记录，未新增提交或推送。
- **原始证据与边界：** `.amp/in/artifacts/ci-dcomp-8c34b024-windows.log` 保存实际运行及消息轨迹，
  `ci-dcomp-8c34b024/` 保存本轮 PNG，`dcomp-8c34b024-capture-inspection.log` 保存 CRC、解码和槽位颜色比较。
  已查看上述候选帧及系统条中段/首尾对照；真实 4K、物理缩放、混合屏、IME、Narrator、系统文字缩放和物理鼠标仍待实机。

### [028] — 2026-11-08 — 采用本轮合成实现，记录已接受的外观差异

- **决定与替代范围：** 用户明确表示“这一轮的实现可以接受，就用这个实现，记录一下差异就行”，
  因而选用 [026] 的父 HWND DirectComposition 实现，本轮不再为 [027] 的按下态色差返修或更换方案。
  此项修订 [022]/[025] 的三态颜色要求及 [027] 因该色差暂停采用的决定；原始失败和诊断记录保留。
- **已接受的颜色差异：** 默认 `#929292`、悬停 `#b0b0b0` 和六 DIP 圆角细条沿用现有实现。
  本次真实屏幕的滑块按下态仍为悬停色 `#b0b0b0`，与原方案预期的更亮 `#c7c7c7` 不同；
  接受该可见差异，不再将它作为要求返修的缺陷。不把单次观测扩大为所有按住状态都必然采用同一种颜色。
- **保留的原生语义：** 六 DIP 是视觉宽度，命中、capture、滚动和隐藏箭头的点击/重复范围仍由原生 EDIT 管理。
  目标是 macOS 风格的深色细条外观，不宣称与 macOS 覆盖式滚动条的全部交互语义相同。
- **验证及交付边界：** 本次只记录采用决定与差异，不修改实现、测试断言或历史 CI 结果，不伪报 CI 转绿。
  当前实现仍由 `smoke.rs` 的 `cfg(test)` 路径加载，正式窗口和演示尚未接入；方案选择与实际接入状态分别记录。
  [027] 未执行的完整拖动、最终 UIA、只读和移除恢复对照，以及真实 4K/混合屏等实机范围继续标注为未验证，
  用户接受外观差异不作为这些检查已经通过的证据。本次未新增提交、推送、合并或发布。

### [029] — 2026-11-08 — 将选定的合成细条接入正式 Windows 预览

- **范围与状态：** 用户要求“正式实现”，据 [028] 将 [026] 的父 HWND 合成绘制从测试边界接入普通
  Windows 预览与演示；只替代此前“产品和演示继续使用系统条”的交付边界，不重做设置向导、macOS
  或按下态颜色。代码和本机检查已完成，独立审查、提交推送及本 head 的 Windows 原生结果待完成。
- **单一实现与所有权：** 图形代码和消息观察器移入 `preview/scrollbar.rs`，生产和测试共用。
  `NativePreview` 在原有控件创建及布局成功后安装唯一的盒装 guard；关闭会话保留 HWND/guard，
  销毁预览时先移除观察器并释放合成目标，再销毁 HWND 和状态。Windows 图形绑定只从 dev-dependencies
  移到正常 target dependencies，沿用锁定版本和既有 features，不升级依赖。
- **失效与重入：** 初始化失败记录警告并保留原生条；运行错误先保留首次原因，再分离、释放合成资源，
  请求原生重绘，本 HWND 后续不重试。原生查询在借用图形状态之前执行；嵌套绘制通知跳过活动更新，
  避免在 FFI 回调中因重复借用而 panic。缓存包含父客户区原点，不保留跨会话可能陈旧的悬停标记。
  不新增轮询、装饰 HWND、私有 API、滚动模型、UIA provider 或通用恢复框架。
- **外观和语义：** 沿用六 DIP 圆角视觉宽度及原生几何。按住态依 [028] 接受一整条悬停灰或亮灰，
  默认、悬停、释放、移出仍检查原定精确颜色；测试拒绝两色混合、浅色穿透、错误宽度/位置及槽外变化。
  连续帧标识包含实际匹配颜色，不能用交替颜色伪造稳定。编辑、capture、命中、滚动、隐藏箭头重复
  和完整 UIA 仍全部交给原生 EDIT；不承诺 macOS 覆盖式滚动条的全部交互语义。
- **验证设计与消融依据：** 原 [026]/[027] 已在同一 hosted 96-DPI 桌面比较系统条与合成候选，
  并证明下层品红写入可读回而观测到的合成槽不变；旧同 EDIT GDI 失败保持历史，不重复换方案。
  本轮继续使用原生/合成、可编辑/只读及移除恢复对照，新增自动安装、同 HWND/guard 复用、长短文本、
  只读转编辑、嵌套重绘及运行错误分离后的实际屏幕负对照。移除检查走正式 Drop，不预先代为分离。
  故障注入只验证错误处理，不冒充真实 GPU/device loss；本轮尚未执行的原生案例不记作通过。
  手动 WM_PRINT 仅用于 test/integration 离屏捕获，轨迹与计数只编入测试，普通产品不携带这些探针。
- **验收与证据边界：** 先检查格式、macOS build/test/Clippy、Windows 正常依赖及两组 feature 的交叉
  检查，再按既有独立审查门禁交付同一 PR。之后检查本 head 的 CopyFromScreen、演示和完整滚动/UIA
  结果；保留九十秒阶段期限与五秒清理预算。真实 4K、实际显示缩放/混合屏、IME、Narrator、系统文字
  缩放和物理鼠标手感仍待适当 Windows 实机，不能由 macOS、GNU 交叉检查或合成 DPI 消息代替。
- **本机结果：** `cargo fmt --check`、macOS integration 构建/全量测试/all-targets Clippy、Windows GNU
  非测试 `--lib --bins --features windows-app` 与两组 feature 的 all-targets Clippy 均通过；锁文件与
  父提交逐字节相同，图形绑定出现在正常依赖图。测试中的裸指针地址比较先触发 Clippy，已采用其自动
  `std::ptr::eq` 修正并重跑通过，没有压制 lint。既有 vendored rdev 警告未修改。
  校验官方归档 SHA-256 后，用 PowerShell 7.6.9 完整解析驱动，并执行原 Test-Slot 及生产颜色选择循环：
  两种完整按住色分别通过，混色、缺条、宽条、错位和浅色槽均拒绝，交替颜色不能匹配同一连续帧标识。
  这些仅是 macOS 上的解析/内存像素检查，未执行 Windows PowerShell 5.1、DComp、屏幕输入或 UIA。
  本机日志和后续原生证据维护在[持续验证记录](https://ampcode.com/artifacts/windows-dpi-b8d76e0c-091e-4333-88d5-ba67d9cb623e)。

### [030] — 2026-11-08 — 修正正式接入测试新增的 Comctl32 导入要求

- **失败证据与诊断边界：** [正式接入提交](https://github.com/b1indsight/viberwhisper/commit/0cbdbd6f3d958b071c9e3b97e4a678f04a082466)
  的 [Windows 作业](https://github.com/b1indsight/viberwhisper/actions/runs/42467077728/job/128688953553)
  构建及测试编译成功，测试 EXE 在任何测试执行前以 `0xc0000139 / STATUS_ENTRYPOINT_NOT_FOUND` 退出。
  后续原生 smoke、演示及 Clippy 跳过，没有生成 PNG，Artifacts API 返回零项；不能沿用 [027] 的截图
  作为本轮验证。macOS 作业通过。失败日志未输出具体 DLL/入口名，未取到该 EXE 的实际导入表。
- **已核对的不兼容导入：** 本轮仅在 guard 清理断言新增 `GetWindowSubclass`，绑定按名称静态导入
  Comctl32；[官方要求](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-getwindowsubclass)
  使用 v6 manifest，而当前测试程序没有该声明。它是与加载失败高度吻合的诊断，仍须以修正后的同一
  Windows 工作流确认，不把文档或 GNU 交叉检查写成失败进程已报告该入口。
- **最小修正及取舍：** 保留正式 Drop 后对父 HWND/EDIT 的回调清理断言，改用生产路径已有的
  [RemoveWindowSubclass](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-removewindowsubclass)
  再次移除同一回调/ID，并要求返回 false；若仍能移除即断言失败。该 API 支持 Comctl32 5.8，
  不为测试引入 v6 主题、序号/私有 API、动态加载适配器或新的状态。没有预先替生产 Drop 移除回调，
  不删除清理检查，不改生产渲染器、依赖、manifest、工作流、接受的颜色或既有输入/UIA/像素断言。
- **本机复验及剩余范围：** 格式、macOS integration 构建/测试/all-targets Clippy 和 Windows GNU
  非测试 lib/bins、两组 feature 的 all-targets Clippy 通过；既有 rdev 警告保持不动。
  修正前固定了加载、清理及原生滚动验收案例，日志为 `.amp/in/artifacts/loader-repair-*-validation.log`。
  下一步仍按同一 PR 的审查门禁交付，再检查 Windows 测试实际启动和完整原生截图/滚动/UIA/回退；
  尚未执行的案例不记通过。九十秒阶段与五秒清理预算不变，真实 4K、混合屏等实机边界继续保留。

### [031] — 2026-11-08 — 关闭会话时清空复用窗口的合成滚动条帧

- **触发与已验证的失败：** [兼容性修正 030](https://github.com/b1indsight/viberwhisper/commit/5d51976d37041bb2aadc0ce2594ac4de0a539892)
  的 [Windows 作业](https://github.com/b1indsight/viberwhisper/actions/runs/42468013105/job/128691932413)
  已正常启动测试、通过设置/DPI 和输入故障清理阶段，支持加载修正；随后既有原生回归在
  `Closing a session retained its scrollbar frame` 断言失败，未触发九十秒期限。
  关闭后的轨迹仍有原生滚动条几何；`close()` 已清空会话并隐藏/清空 EDIT，但绘制器只检查
  原生 style/几何，不检查活动会话，也没有由关闭所有者保证一次同步。
- **实现及取舍：** `preview/scrollbar.rs` 读取已有 `session_id`，无活动会话即无可绘制帧；
  `NativePreview::close()` 调用同一 `sync()` 清空合成内容，不依赖已隐藏窗口再产生绘制通知。
  保留 HWND、guard 和图形资源供下次会话复用；随后原生通知也不能从旧几何恢复已关闭帧。
  不增加可见性/会话副本、窗口、轮询、重建机制或自定义滚动状态，不修改原生输入、UIA、
  style、颜色接受范围、依赖、manifest、工作流或阶段/清理期限。
- **测试与复杂度：** 保留已经在修正前实际失败的关闭/复用回归及全部断言，不增加重复测试。
  既有后续案例要求 streaming/可编辑重开沿用同一 HWND/guard、恢复帧，以及短文隐藏和系统色回退。
  本次只补既有状态约束和所有者同步，没有新机制需要消融；Windows 后续原生结果仍须实测。
- **本机检查及原生边界：** 格式、macOS integration 构建/测试/all-targets Clippy，以及 Windows GNU
  非测试 lib/bins 和两组 feature 的 all-targets Clippy 通过；既有 rdev 警告未改。
  执行前固定案例和输出分别为 `.amp/in/artifacts/scrollbar-drag-composition-cases.md`、
  `scrollbar-close-macos-validation.log`、`scrollbar-close-windows-cross-validation.log`。
  已检查本轮 CI 的默认/滚动位置 WM_PRINT 及 warning PNG：细条无可见箭头，警告未截断；
  这些是修正前的手动离屏证据，不证明实际合成屏幕、拖动或本修正成功。
  完整桌面可编辑/只读、拖动/UIA、移除和运行错误回退及演示尚未执行到；修正后的原生验收
  待同一 Windows 工作流，交叉检查不链接或运行 Windows。真实 4K/混合屏等实机限制不变。

### [032] — 2026-11-08 — Windows 光标定位与缩放残影修复提案（待批准）

#### 请求、基线与证据

- 用户报告结果窗口出现在屏幕中间，大小变化时出现多层圆角残影，要求参考 macOS 的效果修改。
  [本次讨论与截图](https://ampcode.com/threads/T-01a1bbd1-24d8-74c0-bbb4-dab887a9561c)。
- 本次已 fetch，基于 `origin/master` 的 [v0.5.1 发布准备提交](https://github.com/b1indsight/viberwhisper/commit/240b1c20)。
  原 [PR #165](https://github.com/b1indsight/viberwhisper/pull/165) 已合并；本提案作为新的修复跟进，
  保留此前方案和失败记录，不再向已合并 PR 追加实现。本轮未获推送授权，不创建或修改远端 PR。
- **源码确认的缺口：** [Windows 锚点捕获](../../src/platform/windows/preview/position.rs#L92-L128)
  只读取 `GetGUIThreadInfo` 的原生插入符，再回退到 Edit/RichEdit 控件矩形；没有 UI Automation 捕获。
  无锚点时 [show/show_streaming](../../src/platform/windows/preview.rs#L1042-L1133) 明确居中。
  因而未暴露原生插入符且不是上述窗口类的输入目标会进入居中路径；用户实际目标应用尚不明确。
- **重绘假设：** [layout](../../src/platform/windows/preview.rs#L584-L650) 先修改窗口尺寸，
  再更新圆角 region，显式失效的只有 EDIT；窗口类未设置全尺寸重绘样式。
  旧边框像素被保留且未整窗重绘，与截图现象相符，但尚未在 Windows 复现，不能称为已验证根因。
  [DComp](../../src/platform/windows/preview/scrollbar.rs#L90-L169) 只负责滚动条槽，暂不归因或改动它。
- **既有证据的边界：** [artifacts 演示](../../src/ui/preview_demo.rs#L43-L79) 故意没有目标或锚点；
  [WM_PRINT 捕获](../../src/platform/windows/preview/smoke.rs#L8-L125) 会重新绘制当前窗口，
  不能证明真实光标捕获或桌面旧像素清除。此次需要补充实际屏幕的前后对照。

#### 拟采用的行为与改动面

1. **沿用 macOS 的定位语义。** 在气泡取得焦点前捕获目标的文字插入点，同一会话复用；
   每次增大、缩小或增删提示区后，都按新尺寸使用既有 `preview_origin`：优先下方，底部不足翻到上方，
   并限制在目标显示器工作区内。此处的“光标”是文字插入符，不是鼠标，不增加轮询或实时追踪定时器。
   参考 [macOS AX 捕获](../../src/platform/macos/preview/position.rs#L26-L131) 与
   [窗口调整](../../src/platform/macos/preview.rs#L313-L395)，不改 macOS 或共享放置算法。
2. **补齐 Windows 辅助功能锚点。** 在 `preview/position.rs` 保持
   `capture_anchor(thread: u32, focus: HWND) -> Option<PreviewRect>` 的调用契约，
   原生插入符不可用时尝试 UIA 插入点，再尝试焦点可编辑控件边界，最后保留原生 Edit/RichEdit 边界和居中回退。
   拟新增私有 `automation_anchor(focus: HWND) -> anyhow::Result<Option<PreviewRect>>`，
   只返回几何，不读取识别目标的正文或值，不新增跨平台接口或领域状态。
   UIA 查询必须限定在已捕获目标窗口的可访问性子树内，结束后复核前台窗口与原生焦点；
   只比较 PID 不足以区分同进程的不同窗口。密码、失效元素、非编辑页面和整个浏览器渲染窗口
   不作为锚点；插入点矩形不可用时退到可靠的输入框边界，不用邻字右边缘猜测 RTL/换行光标。
   UIA 几何按其物理屏幕坐标处理，不套用原生 client-to-screen 转换或重复缩放。
   COM 初始化、接口和 SAFEARRAY 的释放在捕获线程作用域内完成；使用平台可用的连接/事务超时，
   不在 UI 线程等待提供者，不将这些超时误称为所有调用的硬性总期限。
   `Cargo.toml` 仅启用已锁定 `windows = 0.54.0` 所需的 Windows 目标 feature，不升级绑定或新增框架。
3. **先修最终布局的重绘，保留现有尺寸算法。** 在 `WindowState::layout(&self) -> Result<()>`
   的最终 DPI/几何稳定后，对父窗口、正文、提示和边框统一失效，确保下一次绘制覆盖当前圆角区域。
   保留 `laying_out`、pending DPI、首个可见行和原生控件状态，不通过重建 HWND 清除残影。
   优先比较“最终整窗失效”与“整窗失效加 `SWP_NOCOPYBITS`”，只保留实测必要的措施；
   不默认增加每次按键的同步强制绘制，不把中间测量宽度当最终外观提交。
4. **保持现有交互。** 沿用深色背景、字号、圆角、宽高上限和细滚动条；保留生成中不抢焦点、
   最终编辑、选区、撤销、IME、滚动、确认/取消、关闭重开和图形失败回退。无需配置迁移。

#### 实施顺序、取舍与验收

- 批准后先扩展既有 `preview/smoke.rs` 与受监督的 Windows 测试驱动，在未改产品代码时固定并运行
  光标捕获和增缩窗口场景；再分别实施锚点和重绘修复，重跑相同场景。
  沿用现有进程监督和截图通路，不新增通用驱动框架、装饰 HWND、WebView 或输入法组件。
- **定位验证：** 自有独立进程的原生 EDIT 和 UIA 编辑目标；插入点位于不同文本行、鼠标在远处，
  断言气泡对应文字插入点而不是鼠标或整个应用窗口。覆盖只有输入框边界、完全无几何、焦点切换、
  失效元素和密码目标；不将注入合成 anchor 的测试当成真实捕获测试。
  浏览器/Electron 的具体提供者行为在可用 Windows 环境中确认，失败时如实记录支持边界。
- **残影验证：** 自有纯色背景板后方不包含用户数据；使用实际 `CopyFromScreen`/DWM 同步，
  捕获旧窗口与新窗口的联合区域及小范围外边距，不能只裁当前 region。
  用中文短句逐步增长、长文收缩、流式撤回至空、提示页脚增删及生成中转可编辑驱动同一个 HWND，
  检查旧边框/旧滑块消失、新边框完整、窗口外旧区域恢复背景。保留并查看原始前后截图或录屏，
  不以 WM_PRINT 重画、修图或延长现有超时来制造通过。
- **边界与回归：** 左上/右下和负坐标工作区，增长后向上翻转、缩小后回到下方，
  变宽限位后变窄回到锚点；覆盖有/无锚点及警告状态。沿用滚动、选区、撤销、DPI 重入、
  不激活和关闭复用检查。原生几何与 UIA 几何的 DPI 转换分别验证，避免只测 96 DPI 掩盖二次缩放。
- **比较与成本：** 禁用新增 UIA 应恢复原来的无光标目标回退，已有原生光标路径不应变化；
  逐项去掉整窗失效或禁用旧像素复制，比较同一增缩序列，以实际结果选择最小重绘修复。
  优于重写布局/改用分层窗或 DComp 外框：不扩大渲染和输入职责，不增加窗口/图形资源所有者。
  若基线未复现残影，先记录当前结果并继续收集报告环境，不将某个候选措施写成已证实修复。
- **检查及交付：** 批准后运行格式、共享/平台回归、macOS 构建与 Clippy、Windows 目标类型/lint；
  获推送授权并通过既有审查门禁后，在一个新的修复 bookmark/PR 中交付，由既有 CI 执行 Windows 原生案例，
  不手动 dispatch 或触发发布。物理 4K、混合 DPI、IME 与用户实际应用需适当实机，不由交叉检查代替。

#### 当前状态与待定项

- 本轮仅追加本提案；产品和测试代码未改，未提交、推送或打开新 PR。
- 待用户批准上述方案。UIA 提供者范围、所需 feature 和最小重绘组合在实施时确定；
  不以这些实现问题额外阻止方案审阅。用户目标应用、Windows 版本及显示缩放有助于复现，
  当前缺失不妨碍修补已确认的定位缺口，但会限制“用户报告已修复”的结论。
- 已执行源码、历史实现和验证通路核对，并取得固定 Opus 5.5、tools/MCP 均为空的只读方案咨询。
  咨询中的“已判定根因”、按字符右边缘猜光标和仅按 PID 识别目标未被采纳；咨询不算运行证据。
  本机仍只有 macOS，未执行本提案的 Windows 原生测试或前后对照，也未生成修改后的效果截图。

### [033] — 2026-11-08 — Windows UIA 锚点与最终布局整窗重绘（本地实现，原生验收待运行）

- **触发与范围：** 依据 [032] 的方案及用户随后要求“修复”，实现定位回退和最终重绘。
  保留 [032] 为方案基线；本条记录实际变更，不将方案批准或交叉检查视为 Windows 原生验收。
- **锚点实现：** [position.rs](../../src/platform/windows/preview/position.rs) 保留 Win32 插入符的
  优先级与 DPI 转换，缺失时调用私有 [position/automation.rs](../../src/platform/windows/preview/position/automation.rs)。
  UIA 取当前焦点元素，先沿父链确认属于捕获的顶层窗口，再读取几何；查询后同时复核前台、
  原生焦点和 UIA 焦点。直接取焦点避免枚举整个窗口子树，也不把声明焦点的祖先容器当成输入框。
  密码、只读和非编辑元素不提供新增 UIA 锚点，不读取正文或值。
- **坐标与回退取舍：** 优先活动的 TextPattern2 插入点；旧 TextPattern 只在单一选区上折叠到末端。
  提供者可能不给退化范围返回矩形，此时仅回退到 Edit/ComboBox 的输入边界，不猜相邻字符边缘，
  不把 Document/浏览器渲染窗口的整页边界用于回退。UIA 物理屏幕像素不再缩放；无可靠几何仍保留
  原有居中行为。COM 的成功/S_FALSE 初始化配平、接口和 SAFEARRAY 释放都在捕获工作线程内完成；
  连接/事务各设 500 ms，不声称有全查询硬期限。[Cargo.toml](../../Cargo.toml) 只启用已锁定
  windows 0.54.0 的 Com/Ole/Variant/Accessibility feature，没有升级绑定或增加 UI 框架。
- **重绘实现与最小选择：** [layout](../../src/platform/windows/preview.rs#L649-L666) 在 pending DPI
  清空并恢复首个可见行后，统一安排父窗口、子控件及边框完整重绘，替换仅正文失效的做法。
  保留尺寸、换行、滚动、窗口复用和共享放置算法；不改 macOS、投递或滚动条合成所有权。
  未增加同步 UPDATENOW、NOCOPYBITS、窗口重建或跟随定时器。
- **先写回归，再改产品：** 新增仅测试使用的 [geometry_probe.rs](../../src/platform/windows/preview/geometry_probe.rs)
  和 [geometry_probe.ps1](../../src/platform/windows/preview/geometry_probe.ps1)，接入既有
  [受监督原生测试](../../src/ui/setup/windows/tests.rs) 的独立 geometry 阶段，不延长原有阶段预算。
  自有 WPF 输入框的首/末行提供独立几何期望，主动移除 Win32 caret 并检查确实没有原生插入符，
  防止误测已有快路径；鼠标移到自有背景远端，另覆盖只读、按钮和密码元素。相同 HWND 在自有纯色背景上完成短句增长、
  长文收缩、撤回到空、提示增删及生成中转可编辑，检查不抢焦点。
  DWM 同步的 CopyFromScreen 原图覆盖旧/新窗口联合区域；用离屏参考只比对当前父窗口边框/背景，
  旧窗口外区域必须恢复纯色，并保存连续匹配的实际帧。此用例已通过交叉类型检查，尚未在 Windows 执行。
- **已执行检查：** `cargo fmt --check`、`git diff --check`，macOS 上的
  `cargo test --locked --features integration`、`cargo build --locked --features integration`、
  `cargo clippy --locked --all-targets --features integration -- -D warnings` 通过。
  Windows GNU 的 all-targets Clippy 在 `windows-app` 和 `windows-app,integration` 两组 feature 下通过，
  使用 `DOCS_RS=1` 跳过本机无法链接的 ort 原生包；这不链接或执行 Windows。
  临时 PowerShell 7.4.6 的实际 AST 解析与内嵌 C# 编译通过，但没有执行 WPF、UIA 或屏幕捕获。
  日志保存在本线程 `.amp/in/artifacts/preview-geometry-{macos-*,windows-*,powershell-parse}.log`；
  既有 vendor/rdev 警告未改。
- **消融与证据边界：** 原实现遇到“无 Win32 caret、非 Edit/RichEdit 窗口类”必然无锚点的源码路径
  已确认，但新增用例未能在本机取得原生红/绿对照。整窗失效与 NOCOPYBITS 的桌面对比同样无法执行；
  因此只保留必要的最终失效请求，不引入第二项未证实措施，也不把残影原因写成已验证根因。
  浏览器/Electron 提供者、查询中途失焦/失效、真实混合 DPI/4K/IME 和用户实际应用仍待 Windows 验收。
- **交付状态：** 产品、原生用例和 changelog 已在本地修改；未提交、推送或创建新 PR。
  后续需获推送/建 PR 授权并通过既有审查门禁，再让正常 Windows CI 运行上述原生场景、下载并检查截图。
