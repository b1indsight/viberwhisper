# 63 - 听写审查缺陷修复 — Feature record

**Stage:** Implementation；第 1、2、4、5 项已实现并完成本地验证，交由同一 PR 审查；未合并。
**PR:** [#162](https://github.com/b1indsight/viberwhisper/pull/162)；`fix/audit-findings` → `master`。

## 1. Initial plan

### Summary and motivation

用户在[本次缺陷审查](https://ampcode.com/threads/T-01a1a256-e775-7223-9064-e8e07dd649c2)
后要求“修复除3以外问题”。本计划仅覆盖审查报告中的第 1、2、4、5 项：重复文字删除的
错误纠错学习、STT URL 凭据日志泄露、完全识别失败缺少可见提示，以及 CLI 日志污染 stdout。
第 3 项麦克风断连检测与不完整录音提示明确排除。

审查及规划基线为[当前 master 提交](https://github.com/b1indsight/viberwhisper/commit/466c0ddd3e22901310e34fa038256e0e6690167a)，
规划前已 fetch 并确认远端跟踪引用未变化。三项实测证据和两项静态调用链结论见
[审查报告](https://ampcode.com/artifacts/viberwhisper-2026-11-03-f3f178e9-d6e6-438e-b906-cbaa3e7b087e)。
本次范围内的第 1、2、5 项已有复现；第 4 项尚未 GUI 实测，不把静态检查说成运行验证。

`assets/dictation-prompt.default.json` 和 `config.example.json` 的既有并行修改不属于本次工作，
不覆盖、不暂存，也不纳入后续修复提交。

### Goals, scope, and success criteria

1. 用户把“请找张张山。”改为“请找张山。”后，不生成 `张山 → 山`；
   `Use New New York.` 改为 `Use New York.` 后，不生成 `New York → York`。
   真正的候选内部删除，如 `Codexx → Codex`，仍能学习。
2. HTTP STT 的启动诊断不包含 URL userinfo、query 或 fragment，网络错误字符串不携带
   原始 URL。实际请求地址、鉴权、重试和超时不变。
3. 完全失败或超时且没有有效文字时显示明确的只读错误气泡。Enter 不投递，Esc 关闭并
   恢复目标；错误文字不进入历史、文字注入或确认学习。
4. 有有效部分文字时，保留既有的可编辑确认及不完整警告；空成功、静音和没有音频分片
   继续不显示失败气泡。过期会话、取消和退出不产生迟到提示。
5. 全局诊断写 stderr；CLI 转写正文、JSON、配置值和保存路径等命令结果继续写 stdout。

不新增依赖、配置开关、存储文件或 schema，不增加模型调用。不修改
`src/audio/recorder.rs` 或为设备断连扩展 recorder/orchestrator 协议，不操作个人配置、
历史或记忆文件，也不自动删除已经存在的纠正规则。

### Proposed behavior

| 场景 | 当前行为 | 拟议行为 |
| --- | --- | --- |
| 删除名字前的重复文字 | 可能把完整名字学成残缺名字并误改后续预览 | 有候选内外等价删除位置时跳过该候选的纠错学习 |
| 真正删除候选内的字母或汉字 | 可形成整词纠错 | 保留现有能力，不全局禁止删除 |
| URL 携带虚构用户名、密码或 token | 启动日志完整打印 URL | 启动日志只显示脱敏的 host/path，网络错误不附带 URL |
| 全部识别失败且没有正文 | 警告随空文本一起被过滤 | 空正文、只读错误提示；Esc 关闭后可重新录音 |
| 部分失败但有正文 | 可编辑文字及警告 | 保持不变 |
| 静音或正常空结果 | 没有确认气泡 | 保持不变，不误报服务故障 |
| `convert input.wav > output.txt` | 正文文件中混有日志 | stdout 只有命令结果，诊断进入 stderr |

完全失败与超时使用不同的简短提示，均说明按 Esc 关闭；不将底层错误正文、URL、
provider 响应或凭据拼入提示。失败气泡沿用待确认状态，关闭前不开始下一次录音。

### Technical design and affected modules

| 模块 | 责任与拟修改内容 |
| --- | --- |
| `src/dictation/correction/alignment.rs` | 在已有候选编辑归属检查中补充纯删除的等价位置判断，不更换 LCS 或调整预算。 |
| `src/dictation/correction/tests.rs` | 通过真实 `Memory::prepare`、`confirm` 和下一次预览验证错误规则不生成，保留合法删除对照。 |
| `src/transcriber/api.rs` | 启动事件复用 `TranscriberConfig::metadata().endpoint`；发送和读取响应体的 reqwest 错误先移除 URL，再转为字符串。 |
| `src/core.rs` | 在现有 tracing subscriber 上指定 `std::io::stderr` writer，保留过滤和格式。 |
| `src/ui/listener/event_loop.rs` | 保留无正文失败的 warning，复用原生只读预览显示错误；保留活动会话及取消守卫。 |
| `src/ui/preview.rs` | 原始候选为空时禁止任何 Confirm 投递，即使命令里伪造了非空文字。 |
| `src/ui/preview_demo.rs` | 在现有 render-only 路径增加失败状态的原生检查与截图，不接入真实录音或投递。 |
| 相关既有测试，必要时新增最小 CLI 集成测试 | 验证输出和隐私契约，不断言日志的精确格式或私有 helper 调用顺序。 |
| 相关使用/架构说明、`changelog` 与本记录 | 实现后同步行为和实际验证边界，不夹带无关文档改写。 |

**删除的归属。** 保留现有 `Alignment::new`、`project_noun` 的完整词锚定以及插入保护。
在候选范围投影时，检查纯删除能否通过相邻重复字素移到候选外或跨越候选边界；能移动则
不将这次编辑归给候选。只检查该删除附近的连续重复文本，遇到其他编辑或不匹配即停止，
不扫描无关名字，不用字符串后缀规则代替编辑证据。

拟新增的唯一内部方法为
`fn deletion_escapes(&self, edit: &Edit, range: &Range<usize>) -> bool`，
归属于 `Alignment`，负责上述纯删除归属判断。它以 Unicode grapheme 比较左右等价位置，
在 `project` 中与现有插入判断共同使用。`project_noun` 返回 `None` 后，现有 feedback
代码已经跳过该候选，不需要修改记忆写入、规则保存或共享分段协议。
候选完整词、语境、手工词表保护及现有对齐预算均不变。

**诊断隐私。** 在移动 config 字段前取得现有脱敏 endpoint，仅用于日志；
`self.api_url` 仍持有原地址供请求使用。锁定的 reqwest 提供 `Error::without_url()`，
用于发送及读体错误。启动事件保留安全的 host/path，错误保留故障原因，不强求错误中也
保留 URL。本次不承诺清洗任意 provider 响应体或第三方 debug 日志。

**仅错误的预览。** 不新增窗口、状态字段、公共类型或方法，复用
`PreviewTranscription` 的空正文加非空 warning 表示不能交付的失败；成功空正文仍被过滤。
`finish_transcription` 对无文字失败给出独立文案；有文字部分失败沿用现有文案和确认路径。

Listener 的 `Ready` 分支在保留会话守卫后，使用现有 `show_streaming` 或
`update_text(..., ready=false, ...)` 清空正文并保持只读，再用 `show_error(..., false)`
显示提示。对应 `PendingPreview` 进入现有 `RetryBlocked`，禁止确认但允许取消。
工作线程继续等待同一命令通道，避免 `FinalizationFinished` 立即关闭错误提示。

`run_confirmation` 根据初始正文是否为空禁止交付，不能依据后续 Confirm 参数恢复交付
能力。该状态不追加“复制文字后手动粘贴”的目标错误提示，因为没有可复制的识别正文。
Esc、退出、通道断开和 Ready 移交取消继续走已有恢复目标的出口；错误气泡不会提交学习。
原生平台接口已有只读、禁用 Enter 和消息页脚能力，本计划不修改平台注入和焦点机制。

### Alternatives and rationale

- 全局拒绝删除或拒绝所有缩短名字的规则，会破坏合法名内纠错；调整 LCS 平局方向只会
  把歧义移到另一个边界。因此沿用“等价位置没有唯一归属就不学”的保守策略。
- 修改请求 URL 来脱敏会破坏 query 鉴权。现有 metadata 和 reqwest 错误 API 已够用，
  不另建通用脱敏层或错误包装类型。
- 新建模态错误对话框或第二套窗口生命周期，需要额外平台代码。只显示日志不能解决
  Windows GUI 的可见性问题；把提示塞进转写正文又有误投递风险，因此复用只读气泡。
- 单独增加错误模式 enum/字段可更显式，但当前空正文加 warning 已能表达该状态；必须
  同时保留 worker 的禁止投递守卫，不能只靠 UI 隐藏确认能力。
- 只在 `convert` 临时关闭日志会丢掉诊断，且其他 CLI 数据仍受影响；统一将日志写
  stderr 可以维持命令结果契约，不引入 quiet 开关或文件日志。

### Costs, risks, and mitigations

- 重复字素的删除存在真实意图歧义，保守跳过会少学部分本来正确的纠错；这是为避免
  持久污染所接受的代价。合法名内删除、左右边界及多段场景作为对照验证。
- 已持久化的错误规则不会因修复自动消失。本次不读取或清洗个人记忆，也不推测哪些
  历史规则应删除；如需处理，须另外确认具体数据范围。
- 无文字失败会保留气泡直到 Esc，而非自动回到可录音状态。提示须写清关闭方式，
  并验证取消、重启录音、退出和过期回调，防止形成无法退出的等待。
- 错误提示不展示底层响应，减少诊断细节；安全的日志事件和状态仍用于排查。
  网络错误移除 URL 后可从脱敏的初始化事件定位服务。
- macOS 可执行原生 render-only 验证；Windows 原生运行需对应平台，不能将本机静态
  检查冒充 Windows GUI 验收。仓库内未发现依赖应用 stdout 单独收集诊断的启动脚本；
  用户自建重定向脚本需要相应接收 stderr。

### Implementation order

以下步骤须在本计划明确批准后执行，使用同一修复 bookmark 和同一针对 `master` 的 PR，
不把已合并的旧计划 PR 重新用于本次工作。

1. 在现有纠错测试中加入中英文反例和合法删除对照，先验证旧实现失败，再补齐删除归属。
2. 修复 HTTP endpoint 日志和网络错误 URL，验证本地请求的真实地址及鉴权不变。
3. 将全局诊断改为 stderr，通过隔离配置的实际 CLI 验证正文和诊断分流。
4. 保留完全失败的提示，接通只读气泡与 worker 禁止投递，补真实确认路径回归。
5. 用 render-only fixture 验证普通状态、完全失败和超时提示，检查截图，更新相关文档。
6. 完成全量检查及针对性反向对照，再按项目要求进入代码审查和获授权后的 PR 交付。

### Validation and ablation strategy

- **纠错闭环：** 相同合法候选和确认操作下，中文及英文反例不产生错误规则，下一次
  正确名字不变；`Codexx → Codex` 仍学习。覆盖候选左右边界、无关前后编辑、
  多分段和 Unicode 字素；继续运行既有保护词表、语境隔离与重启回归。
- **URL 与请求：** 使用虚构凭据和回环 HTTP stub；捕获诊断验证没有秘密子串，
  不比较整行日志格式。制造发送及响应体截断错误，检查返回错误不带 URL；验证正常
  请求仍携带原 query 和预期鉴权，错误分类、重试与正文不变。
- **CLI：** 临时 HOME/应用数据目录、合成 WAV 或公开测试夹具、本地 STT stub；
  正常正文、静音和失败路径分别捕获 stdout/stderr。正文只出现在 stdout，诊断只在
  stderr，退出状态正确，不读取个人配置或访问外部服务。
- **失败确认路径：** 通过真实 finalization adapter 和 `wait_for_confirmation`
  验证空失败 warning 保留、没有学习句柄；Cancel 恢复目标，伪造非空 Confirm 也不
  产生文字、历史或学习反馈。复用既有取消、过期会话和 RetryBlocked 测试，不另写只
  检查私有状态或调用顺序的重复测试。有文字部分失败、正常成功、静音及 NoChunks 为对照。
- **原生呈现：** 复用 `viberwhisper-preview --artifacts`，在
  `.amp/in/artifacts/` 保存正常确认、无文字失败和超时的 PNG 并实际检查。执行只读、
  Enter 禁止和 Esc 可退出的原生检查；不捕获真实投递目标、注入文字或保存个人历史。
  截图只证明呈现，行为结论须有执行检查。
- **反向对照：** 新回归先在未修复代码上失败；修复后分别临时去掉删除歧义检查、URL
  脱敏、stderr writer 或空候选禁止投递守卫，确认同一针对性场景再次失败，再恢复。
  不保留变体或额外测试框架；本次不引入需要性能消融的新算法。
- **整体验证：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、
  `cargo clippy --locked --all-targets --features integration -- -D warnings`、
  `git diff --check`。保留需要私有数据或真实服务的忽略测试；Windows 按实际可用工具链
  和 CI 范围报告，修复后结果不能用审查基线代替。

### Open questions and deferred work

待批准的是上述四项修复及错误气泡的交互选择：完全失败时只读、Enter 无效、Esc 关闭后
重新录音。不需要先提供 API 密钥、个人数据或额外配置；计划批准前不写业务代码或测试。

具体的等价删除扫描实现、最少永久回归数量，以及 CLI 测试配置装配方式，按现有代码
在实施时确定，不改变上述契约。无需为了这些内部细节扩大对外接口。

明确延后第 3 项麦克风断连、已有错误记忆清理、Routing 内部错误交互、provider 响应体
及第三方日志的全面隐私审计、预览外观重设计、真实模型质量评测和个人脚本迁移。

## 2. PR change timeline

### 2026-11-04 — 实现四项修复及真实路径回归

- **触发与证据：** 新增记忆闭环回归在原实现中把“联系张山。”变为“联系山。”、
  `Visit New York.` 变为 `Visit York.`；候选内部 `Codexx → Codex` 对照仍正常。
  无正文失败的 adapter 回归原先拿不到预览，worker 回归则能通过伪造 Confirm 投递文字。
  真实 CLI 的正常、静音、失败场景原先均受到 stdout 日志污染；发送错误可附带 URL。
- **实际变更：** `Alignment::deletion_escapes` 比较左右相邻重复字素，发现候选内外都有
  等价删除位置时跳过学习；遇到其他编辑或不匹配即停止，不禁止正常名内删除。
  启动日志复用 metadata 的脱敏 endpoint，reqwest 发送和读体错误显式 `without_url()`；
  全局 tracing 指向 stderr。finalization 保留空失败的 warning，原生预览进入只读和
  retry-blocked 状态，worker 根据初始正文固定交付能力，不接受后来的伪造文字。
- **保留契约：** 正常请求仍发送原 query 和对应鉴权；有正文的部分失败可编辑，静音、
  空成功和 NoChunks 不报错。错误提示不进入历史、注入或学习；没有修改录音断连协议、
  配置 schema、请求预算或个人数据，不清理历史纠正规则。
- **回归取舍：** 纠错沿用 `Memory::prepare → confirm → 下一次 prepare`，覆盖左右边界、
  多字重复、组合重音字素和多段；确认测试扩展已有 worker 用例。CLI 使用公开 speech
  fixture、合成静音及回环 stub，在独立进程捕获日志，避免共享 tracing subscriber 的并发
  不稳定。不保留早期线程局部日志捕获测试。
- **证据边界：** 锁定的 reqwest 在响应体截断场景原本就未带 URL，不能将该对照说成
  先失败；保留其回归以保护发送和读体两条错误契约。provider 响应正文和第三方日志不在
  本次脱敏承诺内。

### 2026-11-04 — 原生失败呈现与配置隔离边界

- **已执行：** `cargo run --offline --locked --features integration --bin viberwhisper-preview
  -- --artifacts <artifact-dir>` 复用真实 finalization adapter 与等待线程，分别从生成中窗口
  切换为失败、直接打开超时提示；检查正文为空、不可编辑、原生 Enter 不确认及 Esc 能结束
  等待。普通确认、失败和超时截图均已检查，文案完整，无底层错误或错误的复制粘贴指引。
  夹具不捕获投递目标、不发出系统按键、不注入文字或写个人历史。
- **实现细节：** demo 的 progress 携带真实会话 ID；从普通确认进入后续失败场景前关闭
  旧窗口，避免取消事件仍带上一会话 ID。新增的原生检查仅在 integration smoke 模块中。
  失败文案复用 listener 的私有 adapter，没有新增生产窗口类型或状态字段。
- **平台限制：** Windows 的 `dirs 5.0.1` 使用 `SHGetKnownFolderPath`，临时 HOME/APPDATA
  无法安全隔离真实 CLI 配置。进程级 CLI fixture 限于非 Windows，且先以只读 `config path`
  检查路径确在临时目录后才写入；不为测试扩大产品配置接口。平台无关 API、学习、确认和
  listener 回归仍由 Windows CI 执行。本机原生呈现检查仅覆盖 macOS，不代表 Windows GUI
  实机验收。
- **发布隔离：** 从已发布的计划 bookmark 建立私有 JJ workspace，仅复制批准范围的实现
  文件，沿用 `fix/audit-findings` 和 PR #162；两个 prompt 文件继续采用原 master 内容，
  不吸收共享工作区的并行修改。

### 2026-11-04 — 反向对照和完整快照验证

- **比较方式：** 保持测试与其他修复不变，每次只临时移除一个机制；不保留实验变体。
  移除 `deletion_escapes` 的调用后，邻接重复删除再次缩短中英文名字，合法名内删除仍
  正常。移除 worker 的初始空正文守卫后，伪造 Confirm 再次实际投递提示文字。移除
  stderr writer 后，正常、静音和失败 CLI 场景重新出现 stdout 日志污染。单独恢复
  未脱敏的启动 URL 后，静音 CLI 回归检出虚构 `url-user`；无须发送 HTTP 请求即可复现。
  原发送错误回归也检出所附 URL，修复后不再出现。
- **结论与取舍：** 保留删除归属检查、初始交付守卫和诊断分流；它们各自保护独立的
  可观察契约，不用全局拒绝删除、仅 UI 禁用按键或关闭日志的简化方案替代。读体截断
  没有先失败的证据，按前述边界仅作为契约回归。
- **最终验证：** 所有变体恢复后，从发布隔离 workspace 运行 `cargo fmt --check`、
  `cargo build --offline --locked`、`cargo test --offline --locked`、
  `cargo clippy --offline --locked --all-targets --features integration -- -D warnings`，
  均通过。使用同一快照重新运行原生 artifacts 夹具并检查最终 PNG；固定 JJ 提交后对其
  父子 Git 对象执行 `git diff --check`。私有 VAD 数据集与真实 Realtime 服务测试继续
  按原设置忽略，未使用个人配置、录音、真实凭据或外部 STT/LLM 服务。
- **证据：** [本线程](https://ampcode.com/threads/T-01a1a256-e775-7223-9064-e8e07dd649c2)
  保留原实现失败、单机制反向对照、最终检查和原生截图。详细日志在
  `.amp/in/artifacts/audit-fixes-*.log`、`audit-api-cli-*.log`，最终图片在
  `.amp/in/artifacts/audit-fixes-preview/`；这些仅为本地审查材料，不提交仓库。
- **剩余边界：** Windows 原生窗口与真实跨应用焦点未在本机执行；托管 Windows CI
  的结果以 PR 检查为准。源码中既有 vendored rdev 警告未在本次修改。第 3 项录音断连
  和已保存错误记忆仍明确排除。
