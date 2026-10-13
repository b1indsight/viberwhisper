# 57 - 悬浮预览与回车确认 — Feature record

**Stage:** Implementation validated locally; hosted checks pending
**PR:** https://github.com/b1indsight/viberwhisper/pull/148

## 1. Initial plan

### Summary and motivation

用户希望语音识别结果先处于待确认状态，再按回车输入。用户于 2026-10-14
选择应用自己的悬浮预览：待确认文字显示在 ViberWhisper 窗口里，确认后才写入
目标应用。本文记录这一方案；目标输入框内的原生输入法预编辑列为备选路线。

当前 [finalization 流程](../../src/ui/listener/event_loop.rs) 在 STT 和后处理完成后
直接调用 `TextTyper`；[HistoryTyper](../../src/history.rs) 在投递前保存历史。
改动在识别结果与实际投递之间增加用户确认，保留现有 HTTP / Realtime 识别能力。

[旧悬浮窗计划](09-floating-window.md) 已被托盘录音控制取代，其职责是显示录音状态
和点击录音。本功能提供识别结果确认，不恢复旧录音悬浮窗；保留旧计划作为历史记录。

### Goals, scope, and success criteria

- macOS 和 Windows 的普通桌面听写都先显示待确认结果。
- 确认前不向目标应用注入识别文字；取消不会写入文字或增加历史。
- 第一次回车只确认这次输入，不在目标应用中产生回车、换行或发送动作。
- 确认后恢复并核对预览出现前记录的目标，再复用平台文字注入。
- 一个识别会话至多保留一个待确认结果；重复按键、过期结果和退出后的结果不能重复投递。
- 保留 F8 / F9、托盘录音、历史菜单和权限引导。CLI 转换、prompt-lab capture
  与 setup 录音验证继续使用各自的结果路径。

首版预览提供只读、可滚动和可选择复制的完整文字，不增加逐字流式显示、文本编辑、
候选列表或系统输入法安装。识别结束后出现待确认结果即可满足本次范围。
桌面听写默认采用此流程，不增加当前没有实际调用需求的模式开关。

### Proposed behavior

1. 用户在目标输入框中录音，按现有方式停止。
2. 后台完成识别和必要的清理。成功结果、已有规则允许保留的部分结果都进入预览；
   空结果不创建窗口，部分失败在预览中说明文字可能不完整。
3. 主线程在显示预览前记录当前目标应用、窗口和可获取的编辑控件。预览显示
   “待确认”、完整文字、目标应用名称以及“回车输入 / Esc 取消”，并提供输入、复制
   和取消按钮。窗口大小受屏幕可用区域限制，长文本通过滚动查看。
4. 预览取得键盘焦点后接收回车。等这次回车释放后才恢复目标并发起投递，避免
   按键重复或剩余释放事件影响目标输入框。点击输入按钮执行同一确认路径。
5. 确认时先保存一次历史，再调用既有注入能力；投递被平台接受后关闭预览。
   历史写入失败沿用当前记录错误并继续投递的行为。
6. Esc 或关闭窗口取消候选，清除待确认状态，并尽可能恢复原目标焦点。
7. 如果预览未获系统允许的前台焦点，窗口仍显示，并提示点击预览后按回车。
   其他应用收到的回车不触发确认；不增加全局回车拦截。
8. 待确认期间再次请求录音，只提示先输入或取消当前结果，不覆盖或排队。
   用户切换到其他应用时仍保留候选，必须回到预览才能确认。

例如，在聊天输入框里说“明天下午三点开会”，停止后先看到预览。按一次回车，
“明天下午三点开会。”被写入聊天输入框；是否发送由用户后续操作决定。

目标不存在、恢复焦点失败、权限不足或注入失败时保留可复制的文字并说明原因，
不自动改投其他窗口。已有历史的失败结果再次确认时复用保存状态，避免重复历史。
若平台报告部分投递或结果不确定，提示先核对目标内容，不自动重发全文。

### Technical design and affected modules

- `src/ui/listener/event_loop.rs`：后台收尾只产生带 `SessionId` 的文本结果事件，
  主线程核验有效会话后呈现预览。确认后另行发起投递，继续用取消门阻止退出后的操作。
- `src/ui/listener/preview.rs`（新增）：管理候选、确认中的一次投递、历史保存状态
  和失败恢复；原生窗口事件回到同一个 winit event loop。待确认属于桌面交互，
  不把窗口状态塞进录音状态机。
- `src/ui/listener.rs` 与 `src/history.rs`：调整历史与投递的组装边界，候选出现时
  不保存；确认时保留先保存再投递的损失保护，显式处理重试去重。
- `src/platform/backend.rs`、`src/platform/runtime.rs` 与 `src/platform.rs`：提供
  主线程预览窗口和不透明目标快照的公共平台边界，返回确认、取消和复制意图。
- `src/platform/macos/preview.rs`（新增）：使用现有 objc2 生态构建 AppKit 原生窗口
  与只读文本控件。保存并恢复前台应用、窗口及可获取的 AX 编辑对象；原生 UI 留在主线程。
- `src/platform/windows/preview.rs`（新增）：使用 Win32 原生非模态窗口与文本控件，
  保存前台窗口及可获取的焦点控件，处理本窗口的确认按键。复用现有 `windows-sys`。
- 平台文字注入：明确“恢复并核对目标”与“发起文字投递”的顺序。保留 macOS
  普通控件 AX 写入、Chromium / 不支持控件的粘贴，以及 Windows `SendInput`。
  能取得编辑对象时核对对象身份；只能取得应用和窗口时明确验证范围。
- `Cargo.toml` / `Cargo.lock`：只增加原生窗口、文本控件和目标恢复所需的绑定功能；
  不引入 WebView 或另一套 GUI / 事件循环。
- `README.md`、相关 architecture docs 与 `changelog`：说明默认确认流程、焦点行为、
  待确认期间的录音操作和失败时的复制办法。

跨线程事件携带会话身份和普通数据，不携带 AppKit 对象或可随意解引用的窗口句柄。
平台拥有快照的生命周期；关闭预览与退出时释放。确认按键只由拥有焦点的预览窗口处理，
现有 [rdev 监听器](../../src/input/hotkey.rs) 继续负责录音键。

### Alternatives and rationale

直接保留当前自动注入不能提供确认机会。给现有全局按键监听添加回车通知，也不能
单靠该通知阻止目标应用收到回车；需要额外的键盘拦截及与用户输入法的协调。
由原生预览窗口取得焦点并处理确认按键，把确认限制在候选可见的交互中。

目标输入框内的原生预编辑能提供输入法下划线和目标应用感知的 composition。
它需要额外组件：macOS 的 InputMethodKit 输入源，Windows 的 TSF COM 输入法 DLL，
以及主程序到这些组件的进程间通信、目标会话管理、安装注册和输入源启用流程。
录音、联网识别和后处理可以继续由主程序承担。
参见 [Apple InputMethodKit](https://developer.apple.com/documentation/inputmethodkit)、
[Apple composition 更新](https://developer.apple.com/documentation/inputmethodkit/imkinputcontroller/updatecomposition())、
[TSF 架构](https://learn.microsoft.com/en-us/windows/win32/tsf/architecture) 与
[TSF compositions](https://learn.microsoft.com/en-us/windows/win32/tsf/compositions)。
Windows 原生 IME 的签名及 app-container 限制还需单独解决：
[IME 要求](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)、
[第三方 IME 签名要求](https://learn.microsoft.com/en-gb/windows/apps/design/input/input-method-editors)。
这条路线扩大了分发和维护范围，用户本次选择悬浮预览，因此留作后续独立需求。

### Costs, risks, and mitigations

预览会暂时改变键盘焦点，必须在文字实际投递之前恢复并验证目标。
Windows 可能拒绝前台切换，即使满足一般条件也不保证成功；按
[SetForegroundWindow 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow)
检查结果，保留候选并允许点击激活预览，不通过模拟点击或强制切换绕过限制。

浏览器、终端和自绘编辑器的编辑对象暴露程度不同。实现时记录目标识别和焦点恢复的
实际验证范围，不宣称所有第三方控件都能恢复到同一个 DOM 输入框。不可核实的目标
应保留候选并提供复制。原生调用返回成功代表平台接受投递请求，不等于目标已持久化文字。

文本可能含敏感内容，预览只保留当前候选；取消、退出和成功投递后释放。
保留现有安全控件拒绝规则。平台失败不能导致自动投递或自动重复发送。

### Implementation order

1. 分离识别结果与投递事件，加入候选和确认状态，并覆盖有效会话、取消和重复确认。
2. 建立平台预览与目标快照边界，实现 macOS 原生窗口、按键释放和焦点恢复。
3. 实现 Windows 原生窗口、焦点恢复与前台切换失败处理，接入同一个交互流程。
4. 接入确认时的一次历史保存、现有注入、权限引导和保留候选的错误反馈。
5. 更新用户及架构文档，完成两端验证、消融比较，再走代码审查。

### Validation and ablation strategy

- **行为测试：** 确认前零注入、零历史；取消清除候选；会话过期和退出后结果被忽略；
  重复回车与按键重复只发起一次投递；失败重试不重复保存历史；恢复目标失败不投递。
  用假平台、内存事件和可控投递结果验证，避免依赖网络或真实桌面计时。
- **平台验证：** macOS 在 TextEdit、Chromium 聊天输入框和终端验证；Windows
  在记事本、Chromium 聊天输入框和终端验证。检查中文、emoji、换行、长文本、Esc、
  按住回车、鼠标确认、切换窗口、目标关闭、权限不足与投递失败。
- **兼容性：** HTTP 和 Realtime 都经相同预览路径；CLI / capture / setup 保持各自
  流程；历史菜单、录音键与托盘退出继续工作。跨平台检查来自 CI，桌面人工验证
  必须记录实际执行结果，未执行的检查保留为待验证。
- **必需检查：** 实现后运行 `cargo fmt --check`、`cargo check --locked`、
  `cargo test --locked`、`cargo clippy --locked -- -D warnings`，并完成 Windows CI 检查。
- **消融：** 用同一批模拟结果及原生演示，比较“立即注入”“仅全局回车通知”和
  “预览本地确认”。再移除目标核验和按键释放等待，确认这些机制是否能被简化而不引入
  错误窗口写入或聊天发送。保持音频、模型和 prompt 相同，不做无关的识别性能实验。
- **时机：** 实现后、自动代码审查前完成比较；任何简化重新执行受影响检查。

### Open questions and deferred work

批准前需要确认本文的焦点交互：首版预览尝试取得焦点；系统拒绝时用户点击预览再确认。
原生输入法预编辑路线仍是备选，切换路线需要更新本文的方案范围。

原生控件的具体尺寸、绑定 feature、快照表示和异步投递结果类型在实现中按平台 API
确定。逐字预览、预览内编辑、连续追加口述和原生 IME 不在本次实现范围。

## 2. PR change timeline

<!-- 初始计划。后续有实质需求或方案变更时按发生顺序追加。 -->

### [001] — 2026-10-14 — 原生悬浮预览、目标保留与确认投递

**状态：已实现；本地验证通过，Windows 完整构建与桌面验收仍需后续验证。**

识别和后处理结果先发布到待确认窗口。主线程窗口处理确认、取消和复制，工作线程
等候命令，确认后恢复目标、校验目标、保存历史一次再投递。取消和退出不会保存未确认
结果；重复/过期回调不会重复发起投递，可能已部分输入的 Windows 结果禁止全文重试。
部分识别失败和不可用目标都保留提示，确认失败保留原文供重试或复制。

原生窗口使用 AppKit 的只读 NSTextView / NSScrollView 和 Win32 的只读 EDIT 控件。
沿用 winit 调度，只有现有依赖的 feature 变化；Cargo.lock 增加的两条边都指向已经
存在的 libc，没有新增 package。已有 integration feature 下增加独立预览演示入口，
用示例文字验证 UI；其 --smoke 模式通过真实原生按键处理路径验证按键重复和释放。

**设计调整：** 初始计划拟在主线程显示窗口时获取目标。实现改为在收尾工作线程获取并
保留原生目标，向主线程只发送文字、目标名称、SessionId 和命令发送端。目标对象的
获取、恢复、校验及释放都在同一工作线程，避免为 AX 代理或 HWND 添加未经证明的
Send/Sync 保证，也使等待用户操作及焦点恢复延迟持续留在后台。平台预览通过独立的
编译期 facade 导出，既有托盘 backend / runtime 无需增加窗口 driver 层。

**验证：** `cargo test --locked --features integration -q` 为 253 passed / 2 ignored；
7 项 preview 回归覆盖等待确认、取消、重复/过期事件、重试历史去重，以及焦点延迟中
退出或切换目标时的零注入。`cargo check --locked`、
`cargo clippy --locked --all-targets --features integration -- -D warnings`、
`cargo fmt --check` 和发布包契约检查通过。
`target/debug/viberwhisper-preview --smoke` 在本机 macOS 原生窗口上退出码为 0，
按住回车期间通知数为 0，释放后为 1。

本机 Windows 完整 cargo check 在现有 C 依赖 tinyfiledialogs / zstd-sys 阶段因缺少
Windows SDK 头文件失败。使用仅依赖 anyhow / windows-sys 的临时检查包，直接加载
本次 Win32 源文件，在 x86_64-pc-windows-msvc 上通过 check 与 `clippy -D warnings`；
此结果是 Rust 接口验证，不能代替 Windows CI、真实窗口焦点恢复或跨应用输入验收。
TextEdit / Chromium / 终端的实际跨应用投递及 Windows 桌面操作尚未手工验收。

### [002] — 2026-10-14 — 确认机制的消融比较

**决定：保留确认命令等待、末次目标/取消校验、历史去重和按键释放确认。**

在 `.cache/2026-10-14/preview-ablation-148/` 建立同一源代码的隔离副本，仅修改一个
机制，使用相同模拟候选文字、假目标、命令序列与原生示例窗口。各副本用独立 Cargo
package identity 防止共用 target 目录导致测试二进制复用；首轮缓存复用结果作废。

| 简化版本 | 同一回归/原生检查的结果 | 保留机制的原因 |
| --- | --- | --- |
| 发布预览后立即确认，省去命令等待 | `worker_requires_confirmation_and_cancel_does_not_commit` 失败 | 取消仍会输入并保存历史 |
| 省去焦点延迟后的目标校验 | `focus_changed_during_platform_delay_prevents_injection` 失败 | 会向已切换目标发起投递 |
| 省去焦点延迟后的取消校验 | `cancellation_during_platform_focus_delay_prevents_injection` 失败 | 退出后仍会发起投递 |
| 每次重试都保存历史 | `failed_confirmation_retry_saves_history_once` 失败 | 同一候选保存多次 |
| KeyDown 即确认 | 原生 --smoke 退出码 1，报告确认早于 Enter 释放 | 确认时键仍按住，剩余事件可能进入目标应用 |

原实现对应回归均通过；KeyUp 原生 --smoke 退出码 0。比较不改变音频、模型、prompt
或后处理，验证的是交互和投递约束。命令与日志保存在各副本的 `ablation.log` 和
汇总 `results.json`；这些临时材料不提交版本控制。

### [003] — 2026-10-15 — 可编辑气泡与真实演示投递

**状态：已实现；macOS 原生投递已验证，Windows 桌面验收仍待验证。**

用户查看 [001] 的窗口后要求只保留气泡输入框，去掉按钮，直接编辑并用回车输入，
并指出示例回车没有写入目标。这次更新替代初始计划和 [001] 的只读预览及按钮设计；
继续采用应用内预览、工作线程保留目标的路径，不增加输入法组件或依赖包。

macOS 使用透明无标题栏 NSWindow、原生圆角绘制和可编辑 NSTextView；Windows 使用
圆角 WS_POPUP 和可编辑 EDIT。正常状态只显示编辑框，故障时显示简短的内联说明。
Enter 在释放时提交编辑器当前文字，Esc 取消，Shift+Enter 换行。macOS 的 marked
text 和 Windows 的 IME composition 消息让选词回车继续由原生输入法处理。
确认事件及工作线程命令携带编辑后的文字；清空后回车不保存历史、不投递。
失败重试不重复保存同一修订，但修改后再次确认会保存新的修订，供失败时恢复。

演示入口现在使用同一个确认工作线程及原生 typer，不录音、不联网、不保存演示历史。
正常启动后有三秒选择目标，回车实际投递；`--smoke` 使用当前测试目标，通过原生编辑
API 替换文字、建立 marked text 并输入 Shift+Enter、重复回车及释放，验证不会提前
确认、通知一次且返回编辑后的文字。原先示例只打印操作的行为已经移除。

**验证：** `cargo test --locked -q` 和 integration 测试均为 254 passed / 2 ignored；
`cargo clippy --locked --all-targets --features integration -- -D warnings`、
`cargo fmt --check` 通过。现有依赖仅增加 AppKit 绑定 feature，Cargo.lock 没有新增
package。本机新建 TextEdit 空白文档，运行 `target/debug/viberwhisper-preview --smoke`
退出码 0；随后 AppleScript 读取文档得到“已编辑的文字：中文、English 和 emoji 🎙️。”，
证明实际输出是编辑后的内容。正常演示窗口也完成一次回车投递，退出码 0。
真实中文输入法候选界面、Chromium / 终端及 Windows 桌面仍未手工覆盖；Win32 源文件
在隔离检查包的 Windows 目标 check 与 Clippy 通过，该检查不代替完整 Windows CI。

**消融比较：** `.cache/2026-10-14/bubble-ablation.py` 在独立 package identity 的隔离
副本中各删去一个机制，使用相同输入与同一项回归。恢复原始 STT 文字、忽略确认事件
的修订，会使 `worker_requires_confirmation_and_cancel_does_not_commit` 失败；历史只
保存第一版会使 `edited_retry_saves_revised_text` 失败；去掉 marked-text 检查的原生
`--smoke` 退出码 1，报告提前确认，正式版本为 0。因此保留编辑后的文本载荷、修订级
历史去重及原生组词保护。无按钮气泡是用户明确要求的外观调整，采用直接原生绘制；
没有额外渲染层或依赖可删。对照命令和日志保存在 cache 中，不提交版本控制。
