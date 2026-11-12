# 69 - 托盘右键菜单选择输入设备

**阶段：** 实现及本地验证完成；Windows 菜单实机与物理麦克风切换待验证。
**PR：** [Draft PR #175](https://github.com/b1indsight/viberwhisper/pull/175)。

增加原生“输入设备”子菜单，选择后保存配置，从下一次录音生效，不中断当前录音。初始方案保留如下；后续实施与验收记录追加到文末。

## 1. 初始方案

### 动机、目标与范围

用户希望直接从当前 App 的右键菜单选择录音输入设备。目前菜单仅提供状态、历史记录和退出；麦克风选择已存在于配置及 setup，但录音器只在构造时复制该设置。[现有菜单](../../src/input/tray.rs)、[录音器](../../src/audio/recorder.rs)、[setup](../../src/ui/setup.rs)

目标是在 macOS 状态栏和 Windows 托盘使用同一套原生菜单完成选择，无需重新运行 setup 或重启 App。沿用 `audio.input_device`，不增加配置版本、依赖、完整设置窗口或通用配置热重载。

### 拟定用户体验

1. 右键菜单增加“输入设备”子菜单，放在历史记录之前。
2. 子菜单包含“系统默认”、可读取名称的输入设备、“下次录音生效”说明及“刷新设备列表”。使用原生勾选表示当前选择。
3. 选择设备后保存到 `audio.input_device`；成功后更新运行时选择。下次按热键或点击托盘开始录音时使用该设备，不需要重启。
4. 录音中仍可选择设备，但本次录音继续使用原有流；下一次录音才切换。HTTP 和 Realtime 保持各自既有分段模式。
5. 选择“系统默认”保存 `None`，每次开始录音时跟随系统默认输入设备，不更改操作系统的全局设置。
6. 启动时列出设备；连接或移除麦克风后可点击“刷新设备列表”。本次不增加后台轮询或平台热插拔监听。

以上的持久化与下一次录音生效是本方案建议，用户原始请求未另行指定。

### 异常与兼容性

- **保存失败：** 保持原来的运行时选择，恢复正确勾选，记录错误并显示可见错误提示；不会显示已经切换成功。
- **配置缺失或无效：** 拒绝持久化，不创建默认文档覆盖已有配置；保持本次运行原有设备设置。正常读取和原子保存复用 `ConfigStore`。[配置存储](../../src/core/config/store.rs)
- **配置的设备已断开：** 保留名称，以“未检测到”的禁用勾选项显示，用户仍可改选其他设备或系统默认。不静默回退。
- **枚举失败：** 显示“无法列出输入设备”，保留当前选择；不把未知状态说成设备已断开。系统默认与刷新仍可选择。
- **没有可用设备：** 显示“未发现输入设备”，仍保留系统默认和刷新入口；开始录音沿用现有无输入设备错误。
- **重名：** 菜单按原始名称去重，沿用录音器“第一个名称完全匹配项”的语义；本次不引入设备唯一 ID。名称只在显示时转义菜单助记符和换行，不修改保存或匹配用的原始字符串。[现有匹配规则](../../src/audio/recorder.rs)
- **刷新与旧事件：** 重建列表使用菜单库生成的新 ID，旧列表迟到的事件不应选中新列表中相同位置的另一台设备。
- **外部编辑：** 每次选择前重新读取磁盘文档，只替换设备字段，保留此前的其他修改；不把启动时的整份旧配置写回。现有存储没有跨进程事务锁，同时写入仍可能最后写入者胜出，本次不扩展此机制。
- 菜单同时用于桌面听写和现有 prompt-lab capture listener；保留 capture 的录音分段、归档和不落字行为。离线转换和独立 setup 不改变。

### 技术设计与接口

不增加新的设备管理服务或平台实现，沿用 `TrayAction → PlatformAction → ListenerApplication` 的现有交互边界。[平台运行时](../../src/platform/runtime.rs)、[listener](../../src/ui/listener.rs)、[事件循环](../../src/ui/listener/event_loop.rs)

- `src/input/tray.rs`：创建 `Submenu` 和 `CheckMenuItem`。新增字段 `input_device_menu: Submenu`、`input_device_items: Vec<(CheckMenuItem, Option<String>)>`、`refresh_input_devices_id: MenuId`，分别保存子菜单、选择项与原始设备名称映射、刷新动作 ID。`None` 映射系统默认；禁用诊断项不产生选择动作。
  - 新增 `TrayAction::SelectInputDevice(Option<String>)`、`TrayAction::RefreshInputDevices`。
  - `set_input_devices(&mut self, devices: Result<Vec<String>, String>, selected: Option<&str>) -> Result<()>`：重建设备项和诊断项，设置初始勾选。原生菜单更新错误必须记录，不能静默忽略。
  - `set_selected_input_device(&mut self, selected: Option<&str>)`：按已接受的选择重设所有勾选。菜单库的勾选项会自行翻转，不能把该临时状态当作保存成功；成功、失败、重复点击当前项均重新同步。
- `src/platform/runtime.rs`：增加对应 `PlatformAction`，去掉不再适用的 `Copy` 派生；通过既有 `PlatformTray`、`PlatformRuntime` 增加上述两个更新方法的转发，更新测试替身。不把录音或磁盘写入放进 tray driver。
- `src/audio/recorder.rs`：复用 `input_device_names()`；增加 `input_device(&self) -> Option<&str>` 和 `set_input_device(&mut self, device: Option<String>)`。后者仅更新下一次启动读取的字段，不停止、重建或修改 `ActiveRecording`。[活动流与持久设置的现有分离](../../docs/architecture/audio.md)
- `src/ui/listener/event_loop.rs`：新增 `refresh_input_devices(&mut self)` 与 `select_input_device(&mut self, device: Option<String>)`，在会话动作归一化之前处理设备动作。选择时按“读取最新文档、修改设备字段、原子保存、更新 recorder”的顺序执行；最后无论成功失败，都按 recorder 的实际选择恢复菜单勾选。错误提示保持原生 Unicode 文本，不显示配置全文或凭据。
- `src/ui/listener.rs`：启动完成后首次刷新菜单，选择标记来自实际用于 recorder 的设置；不为菜单另存一份整个配置文档。
- `README.md` 及相关架构文档：补充菜单入口、刷新方式和下一次录音生效的规则。

### 备选方案与取舍

- **原生子菜单，建议采用：** 无需新增窗口，直接点击设备，与现有托盘交互一致。需要处理动态菜单 ID 及独立勾选项的单选语义。
- **打开带编号输入框：** 可复用 setup 的体验，但需要输入编号，不能在菜单中直接点选设备。
- **仅本次运行生效：** 保存逻辑更少，但重启后会丢失选择，不符合常用麦克风设置的预期。
- **立即重建录音流：** 会引入同一段录音内采样率、缓冲与会话交接问题；下一次生效足以满足本次需求。
- **自动检测热插拔：** 需要增加跨平台通知或定时枚举，本次用显式刷新控制范围并保持现有 `ControlFlow::Wait`。

### 实施顺序

1. 增加录音器下一次设备选择接口及菜单动作传递，保持录音生命周期不变。
2. 增加原生子菜单、设备映射和选择同步；启动及显式刷新使用同一枚举入口。
3. 接通保存后应用和失败恢复，补充针对用户合同的最小回归覆盖。
4. 更新使用说明，执行静态检查、测试及 macOS 原生菜单验收。验收临时 fixture 和脚本完成后删除。

无需数据迁移或版本升级。回滚功能代码后，既有配置仍可被旧程序读取；在需要交付时使用一个 `feat/tray-input-device-selection` bookmark 和同一个面向 `master` 的 PR，不混入工作区中其他功能的改动。

### 验证与消融

以下为计划检查，尚未执行：

- 通过已有测试文件扩展设备选择行为：具名设备、系统默认、重复点击当前项、重名、特殊字符、空列表、枚举失败和未检测到的当前设备。永久测试只保留有真实回归风险的用户合同，不测试私有调用顺序。
- 使用临时配置验证成功保存、重启读取、保留外部已修改字段，以及缺失、损坏、不可写文档不改变运行时选择或覆盖原文件。保存失败应恢复单一正确勾选，不能只断言有日志。
- 通过录音器现有无硬件测试夹具验证修改选择不丢弃或重置活动会话；在具备设备和权限时另外观察实际录音源，本地字段测试不代表硬件切换已验证。
- 原生菜单必须单独运行检查：默认项勾选、具名设备勾选、失败恢复、刷新后的列表和断开状态。截图后实际查看渲染；`cfg(test)` 的无原生 tray 替身不能证明菜单显示或点击正确。
- 执行 `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets --all-features -- -D warnings`。Windows 原生运行需后续在 Windows 上验证，交叉编译不作实机证据。
- **消融：** 临时移除选择后勾选同步，检查重复点击或保存失败会被验收发现；临时用旧文档代替重新读取，确认保留外部修改的检查失败。消融在实现后、代码 review gate 前执行，不保留额外生产机制。

### 待确认与实施中问题

- **待确认：** 本方案整体，尤其是“保存选择、从下一次录音生效”的默认行为。确认前不修改产品与测试代码。
- **实施中确认：** 原生错误提示的具体复用入口，以及当前设备/桌面权限能支持哪些原生验收；在结果中明确未能执行的场景，不通过扩大基础设施解决。
- **范围外：** 更改系统默认麦克风、唯一设备 ID、自动热插拔、全量配置热重载、录音中无缝接流及其他音频设置。

## 2. PR 变更时间线

### 2026-11-12：接通菜单、配置与下一次录音选择

- **触发与实际实现：** 按上述方案在 `input::tray` 增加原生子菜单、新菜单项 ID、原始名称映射与勾选同步；动作经过既有 `platform::runtime`，由 `ui::listener::event_loop` 在会话归一化之前处理。启动与手动刷新复用 recorder 的设备枚举。
- **实现边界决定（已实施）：** 将“读取最新文档、仅改设备字段并原子保存”封装在 `ConfigStore::set_input_device`，由 listener 调用；相比将配置字段操作直接放入 UI，这让配置写入继续归配置模块所有，不新增存储层或全量热重载。缺失及无效文档拒绝覆盖。
- **运行时与失败处理：** 保存成功后才更新 recorder 的持久选择字段，不操作当前流、PCM、会话 ID 或分段模式；成功、失败、重复选择均同步全部勾选。保存失败使用 AppKit `NSAlert` / Win32 `MessageBoxW` 显示固定 Unicode 提示，不展示配置或凭据。
- **回归覆盖：** 先添加配置与录音器回归，缺少对应接口时编译失败；实现后通过。配置回归覆盖具名原始字符串、系统默认、保留外部修改、缺失/损坏文档不覆盖；录音器回归验证选择变化不影响活动会话及非对称 PCM 样本。
- **显示修正：** 原生截图发现 muda 0.17.1 的 macOS check-item 构造器直接显示 `&`，按 Windows 规则转义会产生双 `&`。仅 macOS 设备勾选项保留字面 `&`；Windows 仍转义，保存和匹配始终保留原始名称。修正后的原生截图已重新检查。
- **文档：** README、使用详情、input/audio 架构与 changelog 同步说明入口、刷新和下一次生效规则。未加入依赖、配置字段或后台设备监听。

### 2026-11-12：原生验收、消融与验证边界

- **macOS 原生交互（已执行）：** 临时 fixture 直接包含生产 `src/input/tray.rs`，构造单一真实 `TrayManager`，通过 `NSMenu::performActionForItemAtIndex` 调用真实菜单动作并接收 `MenuEvent`。验证默认/具名选择、重名去重、含 `&`/换行的原始字符串映射、重复点击、模拟保存失败后的勾选恢复、刷新、旧 ID 丢弃、未检测到设备、枚举失败和空列表。失败恢复检查调用生产同步入口，未据此声称真实磁盘写入失败到弹窗的完整端到端链路已验收。
- **渲染（已执行）：** 原生弹出菜单后用 timer 捕获 fixture 自己的窗口，逐张查看默认、具名选择、失败恢复、断开、枚举失败、空列表六种状态；检查单一勾选、禁用样式、单个字面 `&` 和无裁切。名称均标注“验收示例”。截图与原生状态日志保存在[本线程验收记录](https://ampcode.com/artifacts/a0837abf-6bb9-4187-bff3-a193960c73b1)。未捕获其他应用内容。
- **勾选同步消融（已执行）：** 在临时生产源码副本中只移除 `set_selected_input_device` 的同步循环，复用同一原生 fixture（关闭截图）。选择 USB 后断言失败，实际同时勾选 `None` 和 USB；正常实现通过。故保留显式单选同步。日志位置：本地 `.amp/in/artifacts/tray-check-ablation.log`（未入库）。
- **最新文档消融（已执行）：** 在现有配置回归中临时加入对照分支：先保存启动快照，外部将 `mic_gain` 改为 1.5，再改选系统默认。正常 `set_input_device(None)` 通过；`ABLATE_CONFIG_RELOAD=1 cargo test --locked device_selection_preserves_latest_settings_and_rejects_unusable_documents` 改用旧快照保存后失败，实际增益回到 1.0。故保留保存前重新读取。临时分支及验收 fixture 均不保留在产品中。
- **完整本地检查（已通过）：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets --all-features -- -D warnings`。修正标签与撤销消融分支后再次全部通过；现有 `vendor/rdev` 警告未扩展处理。
- **CI 边界：** 初始仅文档提交的 [macOS CI 成功，Windows CI 失败](https://github.com/b1indsight/viberwhisper/actions/runs/43149538933)。读取失败日志定位到既有 `ui::setup::windows::tests::native_setup_dialogs_show_and_return` 的 `scrollbar_probe.ps1:202 Pointer coordinates differ`，未涉及本功能代码，未修改该滚动条测试。实现提交将由同一 PR 的 CI 验证。
- **仍未验收：** Windows 原生菜单点击/外观、物理麦克风录音源切换、真实不可写配置到错误弹窗的完整链路；本地录音缓冲回归与模拟错误不替代这些证据。没有为此改动个人配置、系统默认输入设备或权限设置。
