# 57 - 悬浮预览与回车确认 — Feature record

**Stage:** Planning
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
