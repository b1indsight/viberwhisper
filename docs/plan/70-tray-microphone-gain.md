# 70 - 托盘音频增益菜单 — Feature record

当前实现：macOS 右键菜单内嵌增益滑块，Windows 菜单项打开无标题原生浮层，范围 −10～+10 dB；无保存/取消按钮，松开滑块或键盘调整即自动保存，从下一次录音生效，不中断当前录音。

**阶段：** 自动保存交互已完成实现与 macOS 原生检查；独立工作区的构建、完整测试、诊断特性测试及 macOS/Windows GNU 全目标全特性 Clippy 均通过，详见修订 005。Windows 实机验收仍待进行。Windows 浮层外观改造按修订 007 和用户修订 008 在本地实现，仍待 Windows 实机确认。
**PR 范围：** 用户已要求将本功能独立提交到 `master`，使用 `feat/tray-microphone-gain`；不包含共享工作区的识别/prompt-lab 改动。最新交付状态见[本线程](https://ampcode.com/threads/T-01a1ce33-0c50-75bf-94bc-e7b4ab386556)。合并、发行、替换已安装应用及修改个人配置不在本次授权内。

**适用决策：** [修订 002](#002--2026-11-12--将滑块集成到菜单交互)取代修订 001 的独立窗口安排；[修订 004](#004--2026-11-12--移除按钮并自动保存调整)取代此前的显式保存/取消交互。初始方案和历史修订保留原文，不代表当前交互。

## 1. 初始方案

### 动机、目标与范围

用户要求“添加一个右键菜单项用来控制音频增益大小”。现有配置已有 `audio.mic_gain`，录音回调按该倍数缩放并限幅，但右键菜单只提供输入设备选择，没有增益入口。[音频配置](../../src/audio.rs)、[录音器](../../src/audio/recorder.rs)、[托盘菜单](../../src/input/tray.rs)

目标是在 macOS 状态栏和 Windows 托盘中直接调整应用内的软件录音增益，保留重启后的选择。沿用现有字段和默认值，不更改系统麦克风音量，不增加设置窗口、配置版本、依赖、自动增益或完整配置热重载。桌面监听与现有 capture listener 使用同一入口；离线 WAV 回放不重复应用采集增益。

### 拟定用户体验

1. 在“输入设备”后新增“音频增益：当前值×”子菜单。
2. 提供 `0.5×`、`1.0×（原始音量）`、`1.5×`、`2.0×`、`3.0×`、`4.0×` 六个预设，原生勾选表示已保存并被运行时接受的值。
3. 当前值不是预设时，额外显示禁用且勾选的“当前：值×（自定义）”。保留原值，不取整或自动改成最接近的预设；本次不增加任意数值输入框。
4. 选择预设后，读取最新配置，仅更新 `audio.mic_gain` 并原子保存。成功后更新运行时值及勾选，从下一次录音生效，无需重启。正在录制的音频及本次后续采样仍使用开始录音时的增益。
5. 末尾显示“下次录音生效”。保存失败时恢复原勾选，子菜单标题标记“保存失败”，说明项显示“保存失败，已保留原值；请检查配置文件”，并记录具体错误日志。下次成功保存后清除错误标记，不使用阻塞停止热键或音频处理的模态弹窗。
6. 重复选择当前项也按保存后应用的规则执行，并重新同步单选状态，防止原生勾选项自行取消选中。

档位、持久化和下一次录音生效均为本方案建议，尚待批准。内置默认值继续为 `1.0`，不会因为新增菜单而修改已有配置。[默认值与有限数值反序列化](../../src/core/config/document.rs)、[现有字段解析](../../src/core/config/fields.rs)

### 异常与兼容性

- 缺失、损坏或不可写配置：拒绝覆盖，不创建默认配置冒充保存成功；保持原运行时值并显示上述失败状态。
- 既有自定义有限值：保持兼容，包括预设以外的数值；不借菜单功能收紧 CLI 或配置的范围。
- 较高增益可能产生削波；沿用现有 I16/F32 采样限幅，不声称放大能够恢复原音频没有采集到的细节。
- 保存前重新读取文档，保留此前对输入设备、提示词等其他字段的修改；沿用存储现有的并发边界，不新增跨进程锁。[现有设备字段保存方法](../../src/core/config/store.rs)
- 重新构建增益列表时使用新菜单 ID，迟到的旧事件不能选中新列表中的另一项。菜单更新失败必须记录，不静默忽略。
- 不顺带调整输入设备菜单的错误提示或其他任务已有的未提交代码。

### 技术设计与接口

沿用 `TrayAction`、`PlatformAction` 和 listener 的现有边界，不新增增益服务、通用配置动作或用户设置类型。[平台运行时](../../src/platform/runtime.rs)、[监听循环](../../src/ui/listener/event_loop.rs)

- `src/input/tray.rs`：新增 `TrayAction::SelectMicGain(f32)`，因 `f32` 不支持 `Eq`，移除该枚举的 `Eq` 派生，保留 `PartialEq`。新增 `mic_gain_menu: Submenu` 和 `mic_gain_items: Vec<(CheckMenuItem, f32)>`，分别持有原生子菜单及菜单 ID 对应的增益值；禁用自定义项不产生动作。六个预设是模块内常量，不引入新类型。
  - `set_mic_gain(&mut self, gain: f32, save_failed: bool) -> Result<()>`：重建列表、标题和静态提示，按真实运行时值同步唯一勾选；返回菜单更新错误。使用浮点值的最短可往返显示，不用固定小数位掩盖非预设值。
- `src/platform/runtime.rs`：增加 `PlatformAction::SelectMicGain(f32)`；在现有 `PlatformTray`、`PlatformRuntime` 与测试替身中增加同签名的 `set_mic_gain` 转发。菜单事件仍由主线程处理。
- `src/audio/recorder.rs`：增加 `mic_gain(&self) -> f32` 和 `set_mic_gain(&mut self, gain: f32)`。setter 只更新下次启动使用的字段，不修改 `ActiveRecording`、流、PCM、会话或分段模式；当前回调已经按值捕获旧增益。[录音启动](../../src/audio/recorder.rs)
- `src/core/config/store.rs`：增加 `set_mic_gain(&self, gain: f32) -> Result<(), ConfigError>`，参照 `set_input_device` 重新读取最新文档、仅修改目标字段并原子保存。内部调用值来自有限预设，不增加新的配置校验策略或错误类型。
- `src/ui/listener/event_loop.rs`：增加 `select_mic_gain(&mut self, gain: f32)`，在会话动作转换前处理该动作。先保存，成功才调用 recorder setter；成功失败均按 recorder 实际值同步菜单。增加 `refresh_mic_gain(&mut self)` 供初始化使用；保存错误只影响该菜单状态及日志，不打断录音。
- `src/ui/listener.rs`：创建 application 后初始化一次增益菜单，数据来自实际 recorder，不另存整份配置快照。
- README、配置指南、input/audio 架构及 changelog：补充入口、预设、软件增益与下次录音生效规则。保留其他功能已有修改。

### 备选方案与取舍

- **固定预设子菜单，建议采用：** 沿用现有原生菜单，无需窗口或平台专用滑块；兼顾减小和放大。不能在菜单输入任意数值，现有 CLI 和配置仍可使用。
- **自定义输入框或滑块：** 连续调节更灵活，但需要新增窗口、输入校验和跨平台交互，本次需求不要求这些机制。
- **立即修改活动录音：** 会使同一段音频前后增益不同，且需要共享状态；下一次生效直接复用现有回调的所有权。
- **只在当前进程生效：** 改动更少，但重启会丢失选择；建议与相邻输入设备菜单保持持久化规则一致。

### 实施顺序

1. 增加录音器下一次增益接口和配置字段保存，扩展现有配置、录音回调回归。
2. 接入原生子菜单及动作转发，补齐启动初始化、成功应用和失败状态恢复。
3. 更新文档，运行针对性及完整检查，按既有原生菜单方法验收实际渲染。

没有数据迁移。回滚菜单代码后配置仍可被旧程序读取。需要交付时使用独立的 `feat/tray-microphone-gain` bookmark 和一个面向 `master` 的 PR，不混入工作区其他任务。提交、推送及创建 PR 需另获授权。

### 验证与消融策略

以下是实施后的验收计划，当前未执行：

- **配置：** 用临时文件验证保存后重载，保存前外部修改输入设备等字段仍被保留；缺失、损坏及保存失败不会覆盖文件或更新运行时值。优先扩展现有配置测试，不读取或改动个人配置。
- **录音：** 使用现有无硬件回调夹具，以非对称正负样本验证旧回调保留原增益、新回调使用新值；检查衰减、放大和正负限幅，以及活动缓冲/会话不被重置。永久测试只保留用户可观察的录音与持久化合同，不为 getter/setter 单独建测试。
- **原生菜单：** 按[输入设备菜单的既有验收方法](69-tray-input-device-selection.md)，恢复临时独立 fixture，直接使用生产 tray 代码及真实 `NSMenu` 动作分发。覆盖预设选择、重复选择、自定义值、唯一勾选、保存失败恢复及提示；正常、非预设、失败状态截图后使用 `view_media` 查看。完成后删除临时 fixture，不把 `cfg(test)` 的空托盘替身当作原生证据。
- **比较：** 在临时源码副本移除勾选同步，确认重复点击/失败恢复检查能够发现错误；正常实现再跑同一场景。没有新增音频算法，不安排额外音质基准或自动增益消融。
- **检查命令：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets --all-features -- -D warnings`。记录其他并行任务造成的基线失败，不修改无关代码制造通过。
- **边界：** macOS 原生 fixture 不等于物理鼠标从托盘开始的完整操作链路；Windows 原生外观、实际麦克风采样如未运行必须明确报告，不能用交叉编译或模拟代替。

### 待确认与范围外

待批准以上方案，尤其是六个预设档位、持久化和下一次录音生效。确认前不编写产品或测试代码。

任意数值输入、连续滑块、实时增益、系统音量控制、自动音量归一化，以及替换或重启已安装应用均不包含在本次方案中。

## 2. PR 变更时间线

### [001] — 2026-11-12 — 改用分贝范围与原生滑块

**需求更新：** 用户要求“用-10dB到+10dB, 最好用滑块”，见[本线程](https://ampcode.com/threads/T-01a1ce33-0c50-75bf-94bc-e7b4ab386556)。调节范围与滑块优先由用户确定；下面的小窗口、0.1 dB 步长和保存交互为推荐方案，尚待批准。

#### 交互与兼容性

- 右键在“输入设备”后新增“音频增益（当前 dB）…”，点击打开单实例原生小窗口；重复点击聚焦已有窗口，不丢弃正在调整的值。取代六档预设子菜单，不做跨平台自定义菜单内嵌视图。
- 窗口提供 −10.0～+10.0 dB 的水平滑块，步长 0.1 dB；标出 −10、0、+10，旁边显示当前数值。0 dB 表示原始音量。支持鼠标和键盘调整，提供“保存”“取消”。
- 拖动只更新草稿；保存成功才更新配置和 recorder，下次录音生效。取消、Esc 或关闭丢弃草稿；保存失败在同一窗口内提示并保留草稿以便重试，不弹模态窗口。
- 窗口保持非模态，不调用 `runModal` / `DialogBoxIndirectParamW`，停止热键、音频分片和既有预览事件继续由同一主循环处理。当前录音不受调整影响。
- 设置窗口不得成为听写投递目标。编辑器可见时不启动新录音，重复开始操作仅聚焦编辑器；已有录音的停止操作仍正常处理。关闭编辑器后恢复原有录音入口。
- 保留 `audio.mic_gain` 的线性倍数格式，使用振幅转换 \(g = 10^{d/20}\)，正值反算 \(d = 20\log_{10}(g)\)。−10 dB、0 dB、+10 dB 分别约为 0.3162、1、3.1623 倍。显示精度不触发配置写入，也不增加第二处采样缩放。
- 现有 3.0 倍约为 +9.54 dB；仅打开或取消不能将其改写为滑块显示的 +9.5 dB。保存按钮仅在实际编辑滑块后启用，未编辑时保留原始线性值。
- 对超出调节范围的正值、零值和负值显示明确说明；滑块初始位置可夹在范围端点或 0 dB，但不把这个显示位置当作已保存值。零值标注静音，负系数说明不能以振幅 dB 表示；只有实际调整并保存才替换为范围内的正值。不收紧 CLI 或配置原有范围。

#### 最小实现边界

- `src/input/tray.rs`：改为普通 `MenuItem`，只发出无负载的 `TrayAction::AdjustMicGain`；保留该枚举的 `Eq`。`set_mic_gain(&mut self, gain: f32)` 只更新菜单当前值标签，不再维护预设勾选列表。
- `src/platform/runtime.rs`：新增对应 `PlatformAction::AdjustMicGain`，沿用 `PlatformTray` 的菜单标题更新转发，不在 tray driver 内保存配置。
- `src/platform/gain.rs`：新增小型平台选择门面，按已有 preview 的编译时选择方式导出 `NativeGain`。增益编辑公共状态只需 `original_gain: f32` 和 `draft_db: Option<f32>`，分别保存原始值与实际调整后的草稿；`None` 表示没有编辑，不能意外保存经显示精度取整的值。范围、换算和草稿规则由此处统一，供两个平台复用；不放入 config 模块，不增加通用设置状态机。
- 原生回调使用 `GainEvent::Save(f32)`（待保存的线性系数）和 `GainEvent::Cancel`，经现有 `EventLoopProxy` 到 listener。拟定 `NativeGain` 接口为 `new(notify: Box<dyn Fn(GainEvent)>) -> Self`、`show(&mut self, gain: f32) -> Result<()>`、`show_error(&mut self, message: &str)`、`close(&mut self)`、`is_visible(&self) -> bool`；可见时 `show` 只聚焦而不覆盖草稿。保存发出后暂禁重复提交，结果返回后关闭或允许重试。
- `src/platform/macos/gain.rs`：沿用主线程 `NSWindow`、delegate 与 target/action 所有权，增加 `NSSlider`、数值标签及按钮。`Cargo.toml` 只启用现有 `objc2-app-kit` 的 `NSSlider` feature 及其必要关联 feature，不新增 crate。[现有原生窗口](../../src/platform/macos/preview.rs)、[已有依赖](../../Cargo.toml)
- `src/platform/windows/gain.rs`：使用 modeless Win32 窗口和已有 `TRACKBAR_CLASSW`，将整数位置 0～200 映射为 −10.0～+10.0 dB；沿用现有窗口生命周期、字体和 DPI 策略，处理鼠标、键盘及 `WM_HSCROLL`。不直接照搬 setup 的模态循环。[现有原生窗口](../../src/platform/windows/preview.rs)、[setup 模态路径](../../src/ui/setup/windows.rs)
- recorder 和 `ConfigStore::set_mic_gain` 仍按初始方案只更新下一次录音字段、保存前重载最新文档；listener 改为菜单打开编辑器、保存事件持久化、成功后更新 recorder 与菜单并关闭、失败回写窗口错误。共享 listener 模式仍覆盖桌面和 capture。
- 不实现初始方案的 `SelectMicGain(f32)` tray 动作、`mic_gain_items`、预设列表重建或勾选同步。新增窗口是此次滑块要求带来的必要范围，不扩展成完整设置窗口。

#### 验证、风险与状态

- 验收转换的三个基准点及非对称值、正负边界、滑块键盘步进；验证旧自定义值、超范围、零与负值打开/取消均不写文件，实际编辑后才产生合法新值。
- 验证保存后重载、保留外部已修改的其他字段、保存失败草稿可重试；活动录音保持原 gain，新录音使用新 gain，未编辑不能偷偷量化旧值。
- 原生验收改为菜单打开与真实滑块窗口：初始 0 dB、−10 dB、+10 dB、既有超范围值、保存失败，截图后逐张查看；验证拖动/键盘/保存/取消/单实例聚焦，尤其窗口可见时的停止热键和音频处理不被阻塞。Windows 若无实机只能列为未验证，不把本地绑定存在当作运行证明。
- 取消初始方案中针对预设勾选的消融；用原始值保持与未编辑不写入的对照替代，检查去掉草稿区分后会错误量化 3.0 倍设置。完成后删除临时 fixture，保留有真实回归风险的合同测试。格式、构建、完整测试和 Clippy 仍按初始方案。
- 比预设菜单增加了两个平台的小窗口与生命周期维护；采用系统控件和已有主循环，不引入 UI 框架或阻塞式对话框。实际 DPI、键盘行为与预览并存需在实现时验证，不提前声称可用。
- **决策状态：** dB 范围与滑块偏好已由用户提出；本修订的具体实现方案待批准。尚未修改产品、测试、依赖或个人配置，未提交、推送或创建 PR。

### [002] — 2026-11-12 — 将滑块集成到菜单交互

**背景与决定：** 用户追问“滑块不能内嵌在菜单吗”“windows 能实现吗”，随后明确“接受，按这个方向实现”，见[本线程](https://ampcode.com/threads/T-01a1ce33-0c50-75bf-94bc-e7b4ab386556)。以菜单集成为当前方向，取代修订 001 的两个平台独立有标题窗口；不改变 −10～+10 dB、0.1 dB 步长、持久化和下一次录音生效规则。

- macOS 使用现有 `NSMenuItem` 的 custom view，直接放入 `NSSlider`、当前 dB、刻度、说明与保存/取消；无需打开额外窗口。
- Windows 标准菜单不能直接托管交互式子窗口；采用点击“音频增益”后在鼠标附近出现的 `WS_POPUP | WS_BORDER` 原生 trackbar 浮层。失焦、Esc 或取消收起，保持菜单式体验，不替换整个托盘菜单或引入 UI 框架。[Trackbar](https://learn.microsoft.com/en-us/windows/win32/controls/trackbar-controls)、[Win32 菜单](https://learn.microsoft.com/en-us/windows/win32/menurc/about-menus)
- 两平台复用 `platform::gain::GainDraft` 的原始线性值、可选整数草稿及保存状态；保留未编辑的 3.0 倍、超范围、静音和负系数值。范围只约束滑块新值，不收紧配置/CLI。
- 保存产生 `TrayEvent::MicGain` → `TrayAction::SelectMicGain` → `PlatformAction::SelectMicGain`。listener 先原子保存最新文档，成功后更新 recorder 的下一次录音增益；失败保留运行时值和可重试草稿。macOS 提交会关闭原生菜单，失败提示和草稿在重新打开时可见；Windows 保留仍打开的浮层。已按下“保存”并入队的提交不会被之后关闭动作撤回。
- 菜单或浮层可见时忽略 Idle 状态的新录音动作，避免成为听写目标；活动录音停止、退出和其他会话事件不受此条件阻断。未新增模态循环、系统音量写入或音频缩放阶段。

**实际改动：** 新增 `platform/gain.rs`、`platform/macos/gain.rs`、`platform/windows/gain.rs`；接入 tray/runtime/listener、`ConfigStore::set_mic_gain` 和 recorder getter/setter。AppKit 只增加已有依赖的 `NSSlider` feature。配置保存、活动/下一次录音和草稿兼容性各保留一个聚焦回归；README、配置及音频/输入架构说明同步更新。

**取舍：** 菜单内交互需要平台专用控件与生命周期，Windows 还需管理 HWND、字体、焦点、monitor 工作区与 DPI；保留系统控件和现有事件循环，避免新增跨平台 UI 框架。尚未在 Windows 实机验证外观与多屏行为，不能把交叉编译当作运行证据。未修改个人配置或已安装应用。

### [003] — 2026-11-12 — 兼容性对照与原生验收

**已执行的检查：**

- `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs` 及 macOS `cargo clippy --locked --all-targets --all-features -- -D warnings` 均通过。已有 vendor/rdev 警告保留，不修改无关依赖。
- Windows GNU 完整工作区的 `cargo check --offline --locked --target x86_64-pc-windows-gnu` 和同目标 `cargo clippy --offline --locked --target x86_64-pc-windows-gnu --all-targets --all-features -- -D warnings` 均通过。Windows 模块与真实共享状态、tray-icon/windows-sys 的独立 fixture 在 GNU/MSVC 上的 check 和 Clippy 也通过。完整 MSVC 工作区检查停在 zstd-sys：本机交叉 C 工具链找不到 `string.h`，因此未完成，不记为通过。
- 配置回归验证仅修改 gain、保留最新设备/提示词字段，缺失及损坏文档不被覆盖。PCM 回归用不对称正负样本验证保存后旧回调和本次录音不变，新回调采用新 gain；不访问物理麦克风。
- 原生 macOS fixture 直接导入生产共享状态和 AppKit 控件，在真实 `NSMenu` tracking loop 中执行 target/action、Home/End、Enter/Esc、取消、关闭、失败重试及旧值保留；点击 thumb 后通过真实菜单窗口的 `sendEvent` 验证方向键 0.1 dB 步进。默认、−10、+10、旧值超范围和保存失败状态均截图并逐张检查，无重叠或裁切。

**发现与修正：** 原生鼠标动作可在 thumb 未移动时通知，初版会启用保存并量化旧值；菜单中的 slider 也未自动取得键盘焦点。最终在共享 `GainDraft::change` 忽略未移动的通知，避免两平台重复判断，并在 macOS slider 点击路径设置 first responder。共享兼容性回归和原生点击/窗口键盘分发复验均通过。

**消融：** 对共享状态的临时源码副本移除“未编辑草稿为空”的区分，改为初始化时将既有值夹取/量化成整数 draft，再以 `rustc --edition=2024 --test` 运行同一兼容性回归。简化版在“未编辑不能保存”的断言失败，正式实现通过；因此保留原始值与可选草稿。未新增音频算法，无需音质算法消融。

**证据与限制：** 日志与五种截图汇总于[音频增益菜单方案](https://ampcode.com/artifacts/670f4445-4e46-41f4-b0f8-1d0d0aa26ed6)。额外尝试的合成拖动事件只触发了原位动作，未成功移动 thumb，不能作为物理拖动证据；完整鼠标从托盘开始的路径、物理热键/麦克风、Windows 运行及混合 DPI 仍未实测。临时 fixture 与消融副本在验收后删除。全部功能代码仍为本地未提交状态，未推送、创建 PR 或替换应用。

### [004] — 2026-11-12 — 移除按钮并自动保存调整

**背景与决定：** 用户要求去掉保存和取消，停在滑块当前位置就直接设置，见[本线程](https://ampcode.com/threads/T-01a1ce33-0c50-75bf-94bc-e7b4ab386556)。据此替换修订 002 的显式确认交互：两平台移除按钮并缩短控件区域；松开鼠标或键盘调整自动保存，菜单保持打开。Esc、Enter 和点击外部仅收起，不撤销调整；下一次录音生效规则保持不变。

**实际改动与取舍：**

- macOS 的生产 `NSSlider` 改为非连续 action，松开时提交；方向键及 Home/End 直接提交。Windows 的 `TB_THUMBTRACK` 只更新显示，松开、方向键及分页/首尾通知提交；收起浮层也保留最终位置。复用原有 listener 的先保存、再更新 recorder 路径，不增加写文件线程或音频缩放阶段。
- 共享 `GainDraft` 区分原值、正在拖动的值、在途提交及最后一次已结束的调整。保存仍串行；等待期间的多次完成调整合并到最后一次，旧确认不覆盖新拖动，也不提前提交未松开的拖动。无需禁用滑块或增加按钮。保存失败恢复已保存位置并提示重新调整；后续已完成的调整仍会提交。
- 延续旧值兼容性：只打开或原位点击不量化、不保存。README、配置说明、输入/音频架构及 changelog 已同步。

**验证与对照：**

- 先修改失败恢复回归，在按钮版上失败（仍显示失败草稿 −10.0 dB，而非原值 +9.5 dB）；实现后通过。另一个回归验证慢确认期间连续释放的最终值、较早确认不重置滑块，以及尚未释放的拖动不被提前保存。[失败基线](../../.amp/in/artifacts/gain-auto-red.log)、[正式共享状态结果](../../.amp/in/artifacts/gain-auto-state-tests.log)
- 消融副本改为保存期间直接忽略后续提交。相同连续调整回归失败，正式实现通过，因此保留“最后一次已完成调整”的待提交位置。副本仅位于忽略的临时目录，未修改生产代码。[简化版结果](../../.amp/in/artifacts/gain-auto-no-queue.log)
- 新原生 fixture 直接导入生产共享状态和 AppKit 控件，使用真实 winit proxy 与 NSMenu tracking loop：验证无按钮、非连续 action、真实窗口键盘分发、菜单保持打开时可处理自动提交、上下限、失败恢复和重新调整、快速连续调整、Esc 收起和旧值原位动作不提交。五个最终状态均重新截图并逐张核验，无重叠或裁切。[原生结果](../../.amp/in/artifacts/gain-auto-native.log)
- 自动保存版的 `cargo build --locked --features integration`、普通完整测试、格式检查及 Windows GNU 全目标全特性 Clippy 曾通过。此后工作区另行修改了 `dictation::feedback::Feedback` 及其调用方；复核时这些接口及相关测试仍未匹配，最新诊断测试和 macOS/Windows GNU 全目标全特性 Clippy 均无法编译，不记为通过，也未修改这些无关文件。[先前普通测试](../../.amp/in/artifacts/gain-auto-full-tests.log)、[先前 Windows 检查](../../.amp/in/artifacts/gain-auto-windows-clippy.log)、[最新诊断检查阻塞](../../.amp/in/artifacts/gain-auto-diagnostic-final.log)、[最新 macOS Clippy 阻塞](../../.amp/in/artifacts/gain-auto-clippy-final.log)、[最新 Windows Clippy 阻塞](../../.amp/in/artifacts/gain-auto-windows-final.log)

**交付与限制：** 仍为本地未提交代码，未推送、创建 PR、替换已安装应用或修改个人配置。物理拖动、完整托盘鼠标入口、物理麦克风/热键与 Windows 运行/混合 DPI 仍未实测。临时 fixture、消融副本和二进制在检查后清理，保留截图和日志。

### [005] — 2026-11-12 — 从共享改动提取独立增益功能

**背景与决定：** 共享 default 工作区同时包含本功能和另一线程的识别/生产路径/prompt-lab 改动，不能直接提交整个工作副本。为满足用户“整理这个变更成独立的PR直接提交PR”的要求，新建 `tray-gain-publication-20261112` 本地 jj workspace，以当时最新的 `origin/master`（`d58e7e559febf07619dd6b3b76b81a42d52ebd04`，已合入输入设备菜单）为基底，仅复制经检查的增益专属文件；README、changelog、配置说明、recorder 和 listener 按增益 hunk 单独移入。不修改或提交另一线程的差异，不携带 default 的未发布父提交。

**重新验证：** 独立工作区的 `cargo fmt --check`、`cargo build --offline --locked`、`cargo test --offline --locked`、`cargo test --offline --locked --features diagnostic-logs`、macOS `cargo clippy --offline --locked --all-targets --all-features -- -D warnings` 和 Windows GNU 同命令加 `--target x86_64-pc-windows-gnu` 均通过。新增持久化、旧/新回调和共享自动保存回归包含在完整测试中。修订 004 的共享工作区编译阻塞未带入本功能；已有 vendor/rdev 警告保留，不修改依赖。验证日志汇总于[音频增益菜单方案](https://ampcode.com/artifacts/670f4445-4e46-41f4-b0f8-1d0d0aa26ed6)。

**原生证据与边界：** 三个共享/原生增益模块与修订 004 执行过原生 fixture 的生产源码逐字一致；重新检查默认、两端、旧值超范围及失败恢复截图，均无按钮、重叠或裁切。不将 Windows 交叉编译当作原生运行验收；物理拖动、完整托盘入口、麦克风/热键及 Windows 外观、焦点、混合 DPI 仍保持未实测状态。原生 MSVC CI 结果以独立 PR 的检查为准，本地完整 MSVC 仍未验证。

### [006] — 2026-11-12 — 删除逐属性转发与重复控件状态

**背景与决定：** 用户要求精简没有实际作用的 getter/setter 和转发方法，并追问 Windows 实现长度，见[本线程](https://ampcode.com/threads/T-01a1ce33-0c50-75bf-94bc-e7b4ab386556)。在修订 005 的独立 workspace、已发布功能提交的子 change 中进行行为保持重构，不修改 default 中其他线程的识别/prompt-lab 差异，也不重写已发布提交。

**实际改动与取舍：**

- `PlatformRuntime` 保留主线程所有权、事件转换、历史复制和 text-typer 句柄；listener 直接调用其 crate 内可见的 `tray` 组件。删除 runtime/trait 实现中重复的七组 UI 属性转发及无用途的测试桩，`PlatformTray` 仅要求事件处理。统一调整增益、设备、录音状态和历史的调用方式，避免为同一托盘保留两套入口；不新增控制器或 wrapper。
- recorder 的增益改为 `pub(crate) gain`，删除只读写该字段的两个方法。下一次录音规则记录在字段文档上，流和回调的按值捕获保持不变。`ConfigStore::set_mic_gain` 的重载/原子保存、托盘的菜单和控件同步、`GainDraft` 的保存确认/连续输入合并都保留，这些方法承载实际行为。
- Windows 改用既有子控件 ID 和 `GetDlgItem`，不再重复保存七个子 HWND 的 `Cell`；创建控件统一为局部表。控件顺序、文案、样式、布局表达式及滑块参数与已发布版本对照一致。保留窗口类、字体生命周期、monitor 工作区、DPI 重入、焦点、键盘和销毁顺序，不以新框架、拆文件或压缩格式减少表面行数。共享增益模型与 macOS 原生控件源码未改。

**验证：** 完整普通测试、构建、格式检查、macOS 与 Windows GNU 全目标全特性 Clippy 均通过。诊断完整测试首次与普通构建并行时两个 CLI 日志断言失败，串行复跑同一完整测试通过；保留首次失败和最终通过日志，不修改日志产品代码或测试。额外的临时源码对照验证 Windows 七个控件的 ID/顺序/类型/文案/样式、全部布局和五个滑块参数均未改变。此次仅删除结构和状态，没有新增机制，不需要新的永久测试或消融实验；既有增益持久化、活动回调、旧值兼容及连续调整回归继续通过。

**交付与限制：** 本轮仍为独立 workspace 的本地重构，尚未更新 [PR #176](https://github.com/b1indsight/viberwhisper/pull/176)。本机没有 Windows 运行环境，不能把交叉编译和源码对照记为 Windows 外观、焦点或混合 DPI 实测；此前的原生验收限制继续有效。

### [007] — 2026-11-13 — Windows 浮层外观对齐 macOS 菜单滑块

**背景：** 功能已随 [PR #176](https://github.com/b1indsight/viberwhisper/pull/176) 合入。用户在 Windows 上反馈调节浮层“有点难看”，要求“和 MacOS 调节成外表完全一样”，见[本线程](https://ampcode.com/threads/T-01a1d46e-e1ff-75d1-8aaf-7126dbe8ee9e)。当前 Windows 实现与 macOS 的差距来自三处：

- 窗口是 `WS_POPUP | WS_BORDER` 加系统 `COLOR_MENU` 浅色背景，直角细边；“音频增益”标题和当前值分成两行，面板 320×150。
- 仓库没有嵌入 Common Controls v6 清单，`TRACKBAR_CLASSW` 按 comctl32 v5 经典样式绘制：凹槽、凸起箭头形滑块、`TBS_AUTOTICKS` 刻度线，获得焦点后还有虚线框。
- 文字使用系统菜单字体默认字号（96 DPI 约 12px），没有 macOS 的 13/11 两级字号。

**目标规格：** 修订 004 的 macOS 默认状态截图转换为 sRGB 后取样，并对照 `macos/gain.rs` 的控件 frame。单位为 96 DPI 逻辑像素，与 macOS pt 一一对应。

| 元素 | macOS 实测 | Windows 目标 |
| --- | --- | --- |
| 面板 | 视图 312×106，菜单上下各留 5；圆角约 12；背景 #121212；1px 边 #2D2D2D | 312×116 无标题面板，圆角 12，同色背景和边框 |
| 数值行 | 单行“音频增益  +0.0 dB”，13pt，#DBDBDB，frame (18, 8, 276, 22) | 合并标题与数值为一行，13px，同色同位置 |
| 轨道 | x 18–294，高 6，两端圆角；未填充 #292929，起点至滑块中心为 #1D7CFE | 相同尺寸与颜色 |
| 滑块 | 20×16 胶囊形 #DCDCDC，中心行程 28–284 | 相同 |
| 刻度 | “−10 dB”“0”“+10 dB”，11pt #DBDBDB，无刻度线 | 相同，去掉 trackbar 刻度线 |
| 提示 | 11pt #DBDBDB，frame (18, 82, 276, 20) | 相同 |

**选项：**

1. **嵌入 comctl32 v6 清单，改用系统主题。** 改动最少，但只会变成 Windows 11 风格的 trackbar，并会同时改变 setup 等所有原生控件的外观，仍然不像 macOS。不采用。
2. **保留原生 trackbar，用 `NM_CUSTOMDRAW` 自绘轨道和滑块；面板沿用 Windows 结果气泡已有的 macOS 深色自绘方式。** 鼠标拖动、键盘、`WM_HSCROLL` 自动保存、trackbar 的无障碍数值、DPI 和焦点逻辑都不变，只改像素。建议采用。
3. **完全自绘新滑块窗口。** 外观同样可控，但需要重写已验证的鼠标捕获、键盘和自动保存路径，还会失去 trackbar 暴露的无障碍数值。风险更高，没有外观上的额外收益。

**拟定做法（选项 2，待批准）：**

- 面板：去掉 `WS_BORDER`。背景填 #121212，画 1px #2D2D2D 圆角边，再用 `CreateRoundRectRgn` 裁出圆角，做法与[结果气泡](../../src/platform/windows/preview.rs)相同。保留 `CS_DROPSHADOW`、置顶、单实例、鼠标附近定位以及多屏/DPI 处理。
- 文字：删除独立标题控件，数值行显示“音频增益  +x.x dB”。`WM_CTLCOLORSTATIC` 返回 #DBDBDB 文字和 #121212 背景。数值用 13px，刻度和提示用 11px，字体族取系统菜单字体（Segoe UI / Microsoft YaHei UI），按 DPI 缩放。
- 滑块：trackbar 改为 `TBS_NOTICKS`，并加大原生 thumb，使点击区域覆盖绘制出的滑块。在 `NM_CUSTOMDRAW` 的 channel 阶段，用 Direct2D DC render target 抗锯齿绘制轨道、蓝色填充和胶囊滑块，并跳过原生 channel/thumb/tics 的绘制。GDI 没有抗锯齿，16px 高的圆形滑块会出现锯齿，所以不用 GDI。位置变化时重绘整个控件，避免残留旧的填充，同时隐藏虚线焦点框。只为已有的 `windows` 依赖增加 `Win32_Graphics_Gdi` feature（`BindDC` 需要），不新增 crate。
- 高对比度：与结果气泡一致，开启时改用系统颜色。
- 不改：`GainDraft`、自动保存、Esc/Enter/失焦收起、下一次录音生效等交互，也不改 macOS 代码。

**无法做到完全一致的部分（接受方案即接受这些差异）：**

- 字体：Windows 没有 SF Pro 和苹方，改用 Segoe UI 与微软雅黑 UI。字号相同，字形不同。
- 背景：macOS 菜单是半透明毛玻璃，会透出桌面；Windows 使用截图中的 #121212 纯色。
- 主题：与 Windows 结果气泡一样固定为深色，系统切到浅色时不会变浅。蓝色固定为 macOS 默认强调色，不跟随 Windows 强调色。
- 圆角：用窗口区域裁切，边缘没有抗锯齿，与气泡相同。Win11 的 DWM 圆角更平滑，但半径固定为 8，且 Win10 不支持。
- 位置：macOS 滑块直接嵌在菜单里；Windows 菜单无法容纳控件，仍是点击菜单项后在鼠标旁弹出的独立面板。
- 点击轨道空白处：trackbar 按 1 dB 分页移动，macOS 直接跳到点击位置。本次只改外观，不改这一行为。

**验证与消融计划：**

- 在 `integration` 特性预览 demo 的 `--artifacts` 路径中增加 Windows 增益浮层截图，覆盖默认 0 dB、−10、+10、超范围旧值和保存失败，分别在 96 DPI 和合成 144 DPI 下截取。截图复用结果气泡的 `WM_PRINT` 方法，并断言背景、轨道两侧颜色和滑块中心像素。CI 的 Windows 作业已经上传该目录，下载后与 macOS 截图并排逐张检查。
- 消融：临时去掉“位置变化时重绘整个控件”，按键盘和拖动序列截图，确认会残留旧填充，以证明这一步必要；验证后恢复。
- 检查：`cargo fmt --check`、本机 build/test/clippy、Windows GNU 交叉 Clippy，以及 PR 的 Windows MSVC CI。
- 限制：CI 截图是真实的 Windows 渲染，但不包括用物理鼠标从托盘打开的完整操作。拖动手感和混合 DPI 仍需在 Windows 实机确认。

**状态与交付：** 方案待批准，尚未修改产品或测试代码。实现将使用新 bookmark `feat/windows-gain-mac-look`，单独向 `master` 提交 PR；推送和创建 PR 需要另行授权。

### [008] — 2026-11-13 — 沿用 Windows 菜单的字体与配色，滑块整体自绘

**需求更新：** 用户在[本线程](https://ampcode.com/threads/T-01a1d46e-e1ff-75d1-8aaf-7126dbe8ee9e)回复修订 007：“字体用现在的字号和字体”“背景使用纯色的背景”“主题我们和 Windows 当前的右键栏主题颜色一样”“不要在 Windows CLI 里截图”。以下修改取代修订 007 中对应的部分，其余（macOS 布局、去掉标题行与刻度线、交互不变）照旧执行。

**修改后的决定：**

- **字体：** 保留原来的系统菜单字体和字号（`lfMenuFont`），不改为 13/11px。
- **颜色：** 不再固定使用 macOS 深色。背景、文字和边框从本进程的 `MENU` 主题读取（`MENU_POPUPBACKGROUND`/`MENU_POPUPITEM`/`MENU_POPUPBORDERS`），读取失败时回退到系统菜单颜色；高对比度直接使用系统颜色。应用和 winit/muda 都没有调用 `SetPreferredAppMode`，所以托盘右键菜单在系统深色模式下也是浅色，浮层与它一致。主题变化时重新读取。轨道使用菜单文字色的淡色混合，填充使用系统高亮色 `COLOR_HIGHLIGHT`，滑块使用背景色加边框色描边。
- **形状：** 取消 12px 区域裁切。Win11 按标准菜单的做法请求 DWM 小圆角（`DWMWCP_ROUNDSMALL`），边框由系统绘制；Win10 不支持该属性，保持直角，并按菜单边框色画 1px 边框，与 Win10 菜单一致。背景为纯色。
- **验证：** 不在 Windows CI 或预览 CLI 中增加截图，相应的截图断言和消融对照一并取消。
- **滑块实现（取代修订 007 的选项 2，改为选项 3）：** 不能截图后，trackbar 自绘依赖的原生 channel/thumb 几何、刻度和焦点框行为都无法观察。因此改为在浮层窗口内自行绘制并处理输入，几何完全由代码中的 macOS 尺寸决定：轨道 18–294、高 6，滑块 20×16，中心行程 28–284。Direct2D DC render target（软件渲染，无需 GPU 设备）负责抗锯齿，文字用 GDI 绘制，整窗经 buffered paint 绘制以避免拖动闪烁。随之移除 trackbar、子控件、子类化和 common controls 初始化。

**交互影响：** 自动保存规则不变：拖动时只预览，松开或失去鼠标捕获时提交；方向键 ±0.1 dB、Home/End 到两端，立即提交；Enter/Esc/失焦只收起，收起时提交未结束的拖动。点击轨道直接跳到该位置，按住滑块拖动时保持抓取点，和 NSSlider 一致；原位点击滑块不会改写旧值。与原 trackbar 相比，去掉了 PageUp/PageDown、滚轮调节，以及屏幕阅读器识别的滑块控件角色。

**依赖：** 只为已有依赖增加 feature：`windows-sys` 的 `Win32_Graphics_Dwm`，以及 `windows` 的 `Win32_Graphics_Gdi`（`BindDC` 需要），不新增 crate，`Cargo.lock` 不变。

**验证与限制：** 本机通过 `cargo fmt --check`，以及 Windows GNU 目标的 `cargo check` 和全目标全特性 `cargo clippy -D warnings`。`ort-sys` 没有 GNU 预编译库，检查时用空的 `ORT_LIB_LOCATION` 跳过下载（只检查不链接）。本机没有 Windows，外观、主题取色（尤其是 Win11 `MENU` 主题是否提供这些颜色属性）、DWM 边框、拖动与键盘都未经实际运行，需要在 Windows 上确认；MSVC 构建以 PR CI 为准。
