# 64 - 识别模式二选一配置与 Windows 高 DPI 窗口

**阶段：** 简化实现与 Windows 原生窗口检查已通过，设置/气泡图像已检查；捕获工具的新版 Clippy 修正待 CI，真实 4K 仍待实机。
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
