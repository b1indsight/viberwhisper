# 60 - Realtime 流式预览与最终文本纠正 — Feature record

**Stage:** Published as a draft PR; unresolved review findings below.
**PR:** [#156](https://github.com/b1indsight/viberwhisper/pull/156)，分支
`feat/streaming-confirmation-preview`；未合并或部署，原工作副本中的其他任务修改保留。

## 1. Initial plan

### Summary and motivation

用户要求识别结果流式显示；完整结果到达后，如果内容发生变化，保留相同前缀，
从最早有差异的词开始删除旧尾部，再显示正确的新尾部。用户在
[本线程](https://ampcode.com/threads/T-01a154f6-9340-761a-861e-3d11eb1c8faf)
选择应用现有预览气泡：最终仍由 Enter 确认后才向目标应用输入，不直接流式注入。

当前 [transport](../../src/dictation/transport.rs) 累积的是结构化 JSON 的 delta，
只用于与 `response.output_text.done` 比较；两者不同会报错。最终结果从
`response.done` 解析后，经 [dictation worker](../../src/dictation/engine.rs) 返回。
[桌面流程](../../src/ui/listener/event_loop.rs) 在整次识别完成后才打开气泡。
本功能接续 [Realtime 听写](52-contextual-streaming-dictation.md)、
[原生确认气泡](57-floating-confirmation-preview.md)和
[确认后学习](58-confirmed-text-learning.md)，不改写这些记录的历史决策。

规划前已 fetch 最新 `origin/master`；保留当前工作副本中的其他任务修改，
没有切换、重置、重放工作副本或移动现有 bookmark。实现时需要与这些修改协调，
本功能只使用一个独立 feature record、一个 bookmark 和一个 PR。

### Goals, scope, and success criteria

- macOS、Windows 的 Realtime 桌面听写在收到可解码的正文增量后更新同一个气泡，
  不等待整个响应完成；只显示正文，不显示 JSON、语境或记忆候选。
- 完整结果通过既有校验后，用最终预览文字纠正已显示内容。相同文字不修改控件，
  纯追加只追加；需要纠正时从首个差异词的起点替换到旧文本末尾，不清空重建全文。
- 流式预览是暂定内容；确认前目标应用零注入、历史零新增、跨录音记忆零提交。
- 生成阶段只读、可选择复制、不可确认；完成纠正后恢复现有编辑、Enter 松键确认、
  Shift+Enter 换行和 Esc 取消。该只读策略属于本计划的待批准产品选择。
- 同一次长录音的多段结果顺序正确，保留重复口述和既有语言拼接规则。
- 取消、退出、过期事件、网络失败和截断不会把暂定文字变成可确认结果。
- HTTP STT、CLI convert、prompt-lab、setup 的输出契约不变；不新增服务请求、
  依赖包、配置 schema 或模式开关，不调整用户的 prompt、个人配置或记忆文件。

### Proposed behavior

1. 收到正文增量时，在现有位置和样式的气泡中逐步追加。JSON 的顶层 `text`
   字段可以出现在其他字段之前或之后；没有可显示正文时不显示 JSON 或空气泡。
2. 录音仍在进行时，气泡展示已收到的识别文字但不主动取得焦点。停录后沿用预览的
   焦点策略，生成期间仍只读。不得在每次更新时重新抢焦点、重新居中或取消手动拖动。
3. 整次录音完成且完整结果通过校验、已有本地纠错完成后，计算已显示正文与最终预览
   的差异，执行一次尾部替换，再进入可编辑、可确认状态。最后一个增量不等于完成。
4. 例如流式文字为“我们明天下午去北京。”，最终文字为“我们明天上午去上海。”：
   保留“我们明天”，删除“下午去北京。”，一次插入“上午去上海。”。
   英文 `I like coding` → `I love coding` 保留 `I `，替换 `like coding`。
5. 最终结果本身完整但与暂定 delta 不同是正常纠正，不再因该差异报错；
   `response.output_text.done` 和 `response.done` 两个完成事件仍必须一致，
   且 `response.done.status` 必须为 `completed`。只收到 delta 或 text.done 不能确认。
6. 服务未发送 delta 时，保留当前的一次性完整预览；不通过定时播放完整字符串伪造流式。
7. 生成期间 Esc 取消整个当前听写；如果仍在录音，同时取消录音和对应识别，不让下一批
   delta 重新打开气泡。取消不写入目标、历史或记忆；退出采用同样的失效规则。
8. 失败时删除未完成段的暂定文字，只保留现有规则允许交付的已验证分段，并显示原有
   部分失败提示。没有已验证文字时关闭暂定气泡并报告错误，不开放确认未校验的文字。

**时机边界：** 本次不改变服务请求时机。当前客户端仍在每个 30 秒音频段结束或停录
后的尾段发送 `response.create`，所以短录音会在停录后流式显示生成结果；超过 30 秒
的录音可以在录音期间显示已经请求返回的段。此次不承诺从开始说话起就逐字识别。
若 prompt 把语境和候选放在正文之前，还需等服务生成到 `text` 才能显示正文。

### Technical design and affected modules

**正文增量属于 dictation。** 在 `src/dictation/` 内增加内聚的增量正文解析逻辑，
由 `transport.rs` 在关联正确且去重后的 delta 上调用。解析结构化输出的顶层 `text`
字符串，跳过嵌套字段，处理跨 delta 的键名、引号、反斜杠、换行及 Unicode 转义，
不把未完成转义或孤立 UTF-16 代理项显示为错误字符。保留有界响应大小和现有最终
`DictationResult::parse` 校验；增量解析不替代最终校验，也不直接产生记忆候选。
遇到无法安全解释的暂定结构时停止增量展示，由完整结果决定最终成功或失败。

移除“非空 delta 必须等于最终文本”的拒绝条件，保留响应身份、输出位置、事件去重、
完成状态、两个完成事件一致性及最终 JSON 校验。原始模型结果、最终准备后的预览和
用户确认文本仍分别保留；暂定流式文字不参与纠错学习的来源对齐。

**有界进度传递。** `engine.rs` 按分段索引组合“已验证前段 + 当前暂定段”，复用
`src/text.rs` 的语言拼接规则。通过 `core/orchestrator.rs` 路由带 `SessionId`、
递增修订号和完整正文快照的领域进度；dictation/core 不依赖 UI、winit 或原生窗口。
UI 适配只保留最新进度并合并唤醒，避免按 token 无界积压事件或让窗口操作阻塞接收。
最终完成和取消有明确的终止状态；迟到的暂定快照不能覆盖最终结果或复活旧会话。

**气泡和确认仍由 UI 拥有。** `src/ui/preview.rs` 增加暂定阶段，生成期间拒绝确认，
但允许取消。`ui/listener.rs` 和 `ui/listener/event_loop.rs` 将领域进度适配为预览
更新，允许同一个有效会话在 Recording/Stopping 阶段展示进度。已有交付 worker 的
目标捕获提前到首次展示前，并由同一 worker 保留到确认或取消；不在完整结果到达时
重新捕获已取得焦点的气泡。AX 对象/HWND 不跨线程传递，不引入第二套投递实现。
`core/recording_session.rs` 如需扩展，仅增加按会话身份取消的录音生命周期事件，
复用现有 cancel-recorder / abort-orchestrator effects，不把预览状态放入 core。

**词边界和原地替换。** 共享预览逻辑计算最长相同前缀，再将需要重写的位置回退到
旧、新文本共同安全的词起点；中文复用已有 `jieba-rs`，其他文字复用现有 Unicode
分词依赖。比较保留原始大小写、空格、换行和标点，不做规范化后再修改原始文本。
共享补丁描述为“保留的前缀 + 替换尾部”，不使用通用全篇编辑距离矩阵。

两端 NativePreview 增加同一窗口的阶段切换及尾部替换接口，使用原生文本范围修改，
不调用 `show` 重建状态或每次 `setString` / `SetWindowTextW` 重置全文。
范围换算同时考虑 UTF-8 字节、UTF-16 单位、Windows CRLF 和 grapheme cluster，
不拆 emoji、组合字符或代理对。复用原生尺寸计算和屏幕定位，保持未改前缀、已有
选区和用户滚动位置；仅在用户原本位于末尾时跟随追加滚动。

原生只读期间仍支持选择、复制和 Esc；Enter 不确认、不排队。进入可编辑阶段时重置
按键确认状态，防止生成期间按住 Enter 在完成后意外提交；生成产生的临时替换不进入
用户编辑撤销历史。最终编辑、失败重试、历史修订去重和成功投递后的确认学习继续复用
现有代码，不增加自动确认或新的正文日志。

主要修改范围：

- `src/dictation/transport.rs`、`engine.rs`、模块导出及内聚的增量正文解析模块。
- `src/core/orchestrator.rs`，必要的 `recognition.rs` 组装和录音取消事件。
- `src/ui/preview.rs`、`ui/listener.rs`、`ui/listener/event_loop.rs`。
- `src/platform/preview.rs`、`platform/macos/preview.rs`、`platform/windows/preview.rs`。
- 现有 transport/engine/preview 测试、`ui/preview_demo.rs`、两端 native smoke。
- README、相关架构文档、changelog 和本记录；Cargo 仅在原生绑定需要时增加已有依赖
  的 feature，不新增 package。现有其他任务修改不属于本功能。

### Alternatives and rationale

- 每次重置整个文本控件更简单，但会重置选区、滚动和布局状态，不能满足只重写差异
  尾部的要求。采用共享词边界补丁和原生范围替换，保留既有窗口。
- 直接向目标应用流式输入需要跨应用删除和编辑范围保护；用户已选择气泡，不实现。
- 生成期间允许编辑需要合并“用户修改”和“模型更新”两条修订序列。先只读后编辑
  能避免丢失人工输入，不额外引入文本协同或冲突解决机制。
- 修改服务端为纯文本流、另发元数据会改变协议及部署范围；本次保持现有 JSON 协议。
- 保持 delta/final 必须一致会拒绝用户要求的最终纠正；改为最终校验结果决定权威文本，
  但不放宽两个最终事件之间的一致性及完成状态约束。

### Costs, risks, and mitigations

- 增量 JSON 解析和多种文本索引会增加复杂度。把前者限制在 dictation 输出协议、
  后者限制在预览补丁与平台转换；用同一组中英混排、转义、换行和 emoji 样本验证。
- 中文“词”需要分词约定，分词也可能随尾部上下文变化；回退到双方共有的安全边界，
  允许多重写一个完整词，不为减少替换字符而拆词或拆 grapheme cluster。
- 临时文字可能最终被纠正或撤销。生成状态不允许确认，失败只交付已验证部分，
  UI 不能因看起来像完整句子就把暂定内容当成成功结果。
- 录音期间首次展示提前了原生目标的保留时间。目标仍需在投递前恢复并验证，
  权限失败和目标丢失沿用现有保留候选、禁止误投其他窗口的规则。
- 每个 delta 直接排队会放大 UI 延迟。使用有界最新快照和合并唤醒，验证慢 UI 下
  的最终收敛与取消，而不引入固定时间睡眠或重放动画。

### Implementation order

1. 实现领域正文进度、增量 JSON 解码和允许最终纠正的 transport 处理，保留完整校验。
2. 接入分段聚合、有界进度和会话生命周期；保留最终 Transcript 与反馈能力的所有权。
3. 扩展预览暂定/就绪状态及提前捕获目标，接入取消，确保生成期间无法交付。
4. 实现共享词边界尾部补丁、macOS 和 Windows 原生更新及只读到编辑的切换。
5. 扩展现有演示和 smoke，运行行为与渲染验证，更新文档并记录验证限制。

### Validation and ablation strategy

- **协议到预览：** 用可控 WebSocket 服务拆分结构化 JSON，在暂不发送完成事件时
  观察气泡已收到纯正文；再发送与 delta 不同的有效完成结果，验证尾部纠正和最终一致。
  同一公共路径覆盖没有 delta、事件重放、多段重复口述、失败/截断和取消后的迟到事件。
- **文本边界：** 验证中英文词内修改、纯追加、删除、同文不操作、标点、换行、
  转义/代理对跨 delta、组合字符及 ZWJ emoji。检查补丁应用后严格等于最终原始文本，
  首个差异词之前的控件内容不被替换；无需为每个内部 helper 单独增加永久测试。
- **确认契约：** 生成期间零注入/历史/学习，Enter 不提前或延迟确认；Esc 停止当前
  会话；最终可编辑，确认使用编辑后的文字，重试不重复保存，旧进度不能覆盖人工编辑。
- **原生验证：** 扩展 `viberwhisper-preview` 的现有 integration 演示及两端 smoke，
  覆盖暂定、最终纠正、可编辑确认三个阶段，检查选区、滚动、尺寸、按键释放和真实
  文本范围替换。在 macOS 运行并截图查看流式阶段和最终阶段；Windows 本机不能运行
  的项目明确记录，交叉编译检查不能冒充桌面验收。演示不用真实录音或个人历史。
- **必需检查：** `cargo fmt --check`、`git diff --check`、`cargo check --locked`、
  `cargo test --locked`、integration 测试及
  `cargo clippy --locked --all-targets --features integration -- -D warnings`。
- **消融：** 在隔离副本用同一正文事件序列比较“只等完整结果”“每次全文重置”和
  “原地尾部替换”，检查首次可见正文、实际替换范围、选区及最终文字；分别移除生成期
  确认保护、会话失效检查和完成状态校验，确认相同行为检查能捕获提前投递、旧会话复活
  和截断交付。未带来实际价值的额外机制删除，不为消融增加产品配置。
- **时机：** 实现后、任何代码审查前完成消融；采用简化后重跑受影响检查。
- **真实服务：** 本线程此前诊断收到 `session_unavailable`，未进入识别阶段，不能
  当成流式成功或失败证据。联调需在服务可用且不抢占其他评测时执行；模拟服务证明
  客户端行为，真实请求记录服务是否发送 delta，二者分别报告。

### Open questions and deferred work

**批准前确认：** 生成阶段只读、禁止确认，最终校验完成后才开放编辑；用户选择的
气泡确认投递方式已确定。本文待批准，不因选择气泡就视为整个技术方案已批准。

不改变 30 秒/停录请求时机，不做服务端增量音频推理、直接目标输入框预编辑、
生成期间人工编辑合并、全文词语对齐动画或逐词延时播放。协议字段、解析器私有表示
和原生范围 API 的具体类型留给实现，不扩大外部接口或新增依赖。

## 2. PR change timeline

<!-- 初始计划。后续有实质需求或方案变更时按发生顺序追加。 -->

### [001] — 2026-10-19 — 正文流、单窗口修订与会话取消

- **Context / requirement:** 用户要求在现有气泡流式显示正文，完整结果有变动时只重写
  首个差异词起的尾部；确认前仍不得向目标应用输入或提交学习。
- **Implemented changes:** `dictation::text_stream` 增量解码 JSON 顶层正文并跳过元数据，
  处理跨 delta 的转义和 UTF-16 代理对；transport 保留关联、去重、大小上限、完成状态和
  两个完成事件一致性，允许 delta 草稿与权威完成结果不同。engine 按原有语言规则组合
  已验证前段和当前正文；原始结果、已纠错的最终预览与用户确认反馈保持分离。
- **Decision / status:** 使用录音专属的单槽 `Progress` mailbox；同一锁保护最新文本、
  合并唤醒和关闭状态。没有排队的正文快照，因此不再额外传递修订号；会话身份与终止
  检查负责拒绝过期事件。它同时保留取消 token，使 stop 后已脱离 orchestrator 的
  收敛任务也能被 Esc 取消。
- **Implemented changes:** Realtime 提前启动原有交付 worker，在录音开始时捕获并保留
  目标，再等待停录信号；进度独立唤醒 UI，不让网络等待阻塞窗口更新。流式阶段没有
  确认命令通道，最终校验后才创建 `PendingPreview`。Esc 关闭气泡并取消录音/识别，
  生命周期保持 Stopping，等旧 worker 清理并恢复目标后才允许下一次录音，避免恢复
  焦点与新会话捕获目标相互干扰。HTTP、CLI、评测和 capture 保留原有路径。
- **Implemented changes:** 两端原生气泡使用共享 `TextPatch` 回退到双方共同的安全词
  边界，并用原生范围替换尾部。macOS 转为 UTF-16 范围；Windows 同时处理 CRLF。
  原窗口、前缀选区、滚动与手动位置得以保留；生成期间只读可选，完成时清除临时撤销
  和确认键状态、开放编辑。追加不重写前缀，同文不替换内容。
- **Evidence:** 新增 WebSocket→orchestrator 契约回归：服务等消费者看见正文后才发送
  完成事件，确认草稿确实提前到达；覆盖最终纠正、重放、截断、完成事件不一致和已分离
  finalizer 的取消。复用长录音重复段回归检查进度最终拼接；新增 mailbox 慢消费/关闭、
  中英 Unicode 尾部修订及按会话取消回归。初次完整 integration 检查为 310 passed / 2
  ignored，随后增加取消生命周期用例；最终检查结果见后续记录。

### [002] — 2026-10-19 — 原生渲染验证与机制消融

- **Implemented changes:** 扩展既有 integration demo 展示追加→最终纠正→可编辑；
  macOS `--artifacts` 模式只捕获自身视图，不捕获输入目标、不输入文字、不改个人配置、
  历史或剪贴板。启用现有 AppKit 依赖的位图绑定，仅用于 integration PNG 截图，无新增
  package。native smoke 检查只读、前缀选区、中英文/emoji/换行修订和 Ready 转换。
- **Evidence:** 执行 `cargo run --locked --features integration --bin viberwhisper-preview
  -- --artifacts .amp/in/artifacts/streaming-preview`，退出码 0。已查看暂定正文
  “我们明天下午去北京。”与最终“我们明天上午去上海。”的实际原生 PNG，文字完整、
  无 JSON/标题栏/按钮或布局遮挡。结果位于 `.amp/in/artifacts/streaming-preview/`。
- **Ablation setup:** 在忽略目录 `.amp/in/streaming-ablation` 复制代码、manifest、
  assets、vendor、测试 fixture 和示例配置，使用同一依赖缓存逐次只改一个机制；每次
  检查后还原隔离副本，未修改正式实现。下表记录实际执行结果；详细日志位于
  `.amp/in/artifacts/streaming-ablation-*.log`。

  | 简化变体 | 相同检查与观测 | 结果 |
  | --- | --- | --- |
  | 不发布 delta 正文，只等完成 | `cargo test --locked --lib provisional_text_precedes`，5 秒内未见正文 | 101，回归失败 |
  | 每次从位置 0 替换全文 | 相同原生 `--artifacts` fixture，前缀选区丢失 | 1，`Streaming correction rewrote the selected prefix` |
  | 移除生成期确认保护 | 相同原生 fixture，生成中产生确认回调 | 1，`Generating preview confirmed before completion` |
  | mailbox 不检查 closed | `cargo test --locked --lib slow_consumers_coalesce`，关闭后仍可读到 late 文本 | 101，回归失败 |
  | 移除 completed 状态校验 | 同一协议回归把截断文本保留为可交付部分结果 | 101，`partial_text.is_empty()` 失败 |

- **Decision / consequences:** 保留正文解析、单槽终止规则、原生生成期保护、词边界尾部
  替换与完成状态校验。上述较简单变体均丢失已批准要求；没有加入修订排序层、人工
  动画、额外配置或第二套交付流程。
- **Platform / service limits:** 完整 Windows 交叉构建受 ort GNU 归档及 MSVC C/SDK
  头文件限制；独立小 crate 引用实际 Windows preview、position、shared patch 和 smoke
  源码，GNU 目标 check 已通过。此结果不代表 Windows 桌面实测。真实 Realtime 服务
  仍被另一线程串行评测占用，本线程未抢占服务；模拟协议和原生 UI 验证不能证明该
  服务当前会发送 delta。此前 session_unavailable 的诊断结论仍保持不确定。

### [003] — 2026-10-19 — 最终验证与交付边界

- **Evidence:** 正式工作副本执行 `cargo fmt --check`、`git diff --check`、
  `cargo check --locked`、普通与 `--features integration` 的 `cargo test --locked`
  均通过；两次测试分别为 311 passed、0 failed、2 ignored。忽略项需要私有 VAD 数据集
  和真实 Realtime 服务。`cargo clippy --locked --all-targets --features integration
  -- -D warnings` 通过，仅保留已有 vendor/rdev 警告。完整终端日志为
  `.amp/in/artifacts/streaming-final-verification.log`。
- **Evidence:** 再次执行原生 `--artifacts` fixture，退出码 0；已查看最终纠正和生成阶段
  PNG，正文及布局符合预期。协议回归确认完成事件发出前已经收到纯正文，最终不同文本
  能正确收敛；本次没有向真实目标应用注入测试文字。
- **Delivery status:** 已完成本地实现、差异自查和上述验证；未提交、推送或创建 PR，
  不宣称已安装或发布。保留工作副本中其他任务的修改。Windows 原生运行及真实服务
  delta 联调尚未验证，不暂停另一线程的串行评测，也不改动其配置。

### [004] — 2026-10-19 — 基于最新 master 整合发布版本

- **Context / requirement:** 用户要求 review 后推送。远端已合入稳定前缀协议（PR #153）、
  P2 修复（PR #154）和 prompt-lab 简化（PR #155）；原草稿基于更早代码，不能直接整体
  推送，否则会回退已发布改动。
- **Implemented changes:** 在独立 `streaming-publication` Jujutsu workspace 以最新
  `origin/master` 为父提交，只移植本功能。`finish_streaming` 接受当前 `PromptInput`，
  保留录音冻结词汇、音频前的 prefix acknowledgement、动态后缀、双 item 删除和评测
  trace；新增协议回归改用 canonical `commit_segment` / `delete_segment` helpers。
  不迁移其他任务的配置、记忆、数据集或 prompt 改动，原工作目录与 bookmark 不变。
- **Evidence:** 整合版本重新通过 fmt、check、普通/integration 测试和严格 Clippy；
  两次测试各 318 passed、0 failed、2 ignored，包含现有稳定前缀回归。原生截图 fixture
  退出码 0，已查看最终纠正与生成阶段 PNG；终端日志为
  `.amp/in/artifacts/streaming-publication-verification.log`，截图位于
  `.amp/in/artifacts/streaming-publication/`。正文解析、mailbox、词边界补丁及原生交互
  与 [002] 已消融验证的实现相同；协议契约回归再次验证完成前正文和取消后的清理。
- **Delivery status:** 该隔离版本待审查和推送；真实服务与 Windows 桌面验证限制不变。

### [005] — 2026-10-19 — 审查结果与草稿发布

- **Evidence:** Oracle 与独立 Codex gate 审查了同一实现
  [`333aa9b7`](https://github.com/b1indsight/viberwhisper/commit/333aa9b78032e39d3d13deb398bebfeddf3eb234)。
  gate 完成结构化结果解析，退出码 0，无 critical；结果为 2 medium、2 low，已向用户
  完整报告。Oracle 同时指出下表前两项。gate 本地报告为
  `.git/jj-reviews/poryqruwtzoq-333aa9b78032.json`。

  | 严重度 | 尚未修复的问题 | 位置 |
  | --- | --- | --- |
  | Medium | Windows 气泡增长后不再按 work area 限位，可能超出屏幕边缘 | `platform/windows/preview.rs:332` |
  | Medium | Esc 与 Ready 移交并发时，早退或命令通道断开可跳过已捕获目标的恢复 | `ui/listener/event_loop.rs:741` |
  | Low | 已验证的空文本快照被忽略，已撤回的暂定文字可能继续显示到后续更新 | `ui/listener/event_loop.rs:312` |
  | Low | Windows smoke 未取得前台时返回成功，可能未执行流式断言 | `platform/windows/preview/smoke.rs:9` |

- **Decision / consequences:** 遵循仓库 critical-only 门禁及 findings-driven rework 规则，
  本轮只报告发现，不自动改代码或重跑审查。按用户要求推送通过门禁的精确提交并创建
  [草稿 PR #156](https://github.com/b1indsight/viberwhisper/pull/156)，PR 描述列出全部未修复项，
  不标记 Ready、不合并。此后只追加本记录，不改动已审查实现。
- **Delivery status:** 已推送到用户自己的仓库；尚未合并、安装或部署。原工作副本及
  其他任务修改未移动；真实服务 delta 和 Windows 桌面运行仍未验证。
