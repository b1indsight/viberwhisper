# 59 - 仓库代码准则检查 P2 修复 — Feature record

**Stage:** Published for review; local validation and Claude review passed; hosted CI pending.
**PR:** [#154](https://github.com/b1indsight/viberwhisper/pull/154)，`fix/p2-review-findings` → `master`。

## 1. Initial plan

### Summary and motivation

用户要求修复[本次整个工作副本代码准则检查](https://ampcode.com/threads/T-01a153fd-0936-75aa-a1ea-de312a192fd2)
中确认的六项 P2。它们涉及数据集路径边界、默认网络目标、听写日志隐私、命令的隐式创建、
权限测试覆盖，以及评测种子的生产保留规则。P3 不在本次范围内。

检查范围保留当前工作副本及已有的四个文件改动，包括 dictation worker 的
`runtime.shutdown_background()` 退出修复。规划前已 fetch 最新 `origin/master`，
未切换、重置或重放当前工作副本，也不移动已有的 `feat/confirmed-text-learning` bookmark。

### Goals, scope, and success criteria

1. 样本 show/list/correct 拒绝符号链接和越出数据集的 sidecar；失败时外部文件不变。
2. Realtime 省略 URL 时只使用文档约定的回环默认地址，继续默认关闭。
3. HTTP STT 与 Windows 注入成功日志只记录长度，不包含完整听写正文。
4. evaluate、sample list/show/correct、dataset validate 只打开已有数据集；缺失或不完整时
   报错，不创建数据集目录、manifest 或存储子目录。record 保留创建能力。
5. 权限提示负向测试执行实际确认 worker，覆盖文本相似但类型无关的错误与取消行为，
   删除仅为测试保留的投递逻辑副本。
6. 评测拒绝在首次请求前就会被生产初始化丢弃的种子，覆盖恰好达到 24 小时的过期边界。

不新增依赖，不更改 Realtime 协议、配置 schema、记忆有效期或数据集 schema，
不改动个人配置、真实录音、历史及记忆文件。本次不执行真实 API 请求或原生文字注入。

### Proposed behavior

- `sample correct` 遇到指向外部合法样本 JSON 的符号链接时，返回错误且不修改目标。
  正常数据集的采集、修正、校验和评测继续工作。
- 默认 Realtime URL 从开发局域网地址改为 `ws://127.0.0.1:8080/v1/realtime`，
  与 [README](../../README.md#realtime-听写与语境记忆) 及
  [配置示例](../../config.example.json) 一致。已显式配置的 URL 不迁移、不覆盖；
  本地服务仍可使用 `ws`，远端服务仍可使用 `wss`。
- 成功日志保留事件名称及字符数或字节数，CLI 正常输出、预览和历史中的正文不变。
- 对拼错或不存在的数据集路径执行 validate 等命令，非零退出且不留下新目录；
  已存在但缺少 manifest 或必要子目录的数据集不被静默修补。
- 权限判断的生产行为不变；更换测试入口，使生产判断回归时测试实际失败。
- 种子中恰好过期的观察返回明确校验错误；仍在有效期内的同一观察可用于初始化。

### Technical design and affected modules

| 模块 | 拟修改内容 |
| --- | --- |
| `src/prompt_lab/dataset.rs` | 增加只打开已有数据集的入口，复用 manifest 与存储目录校验；在统一 sidecar 读取边界检查普通文件、符号链接和规范化路径归属，让 show/list/correct 继承约束。 |
| `src/core/prompt_lab.rs` | evaluate、sample 与 validate 改用只打开入口，仅 record 继续 open-or-create。 |
| `src/dictation/config.rs` | 将默认 URL 改为文档已有的回环地址；用默认配置与显式 URL 对照验证。 |
| `src/transcriber/api.rs`、`src/platform/windows.rs` | 将两个成功事件的正文字段替换为长度字段。 |
| `src/ui/listener/event_loop.rs`、`src/ui/preview.rs` | 删除 cfg(test) 的 deliver_text 副本，将负向场景放入 wait_for_confirmation 的真实命令流；保留现有有意义的取消和类型化权限拒绝测试。 |
| `src/dictation/evaluation.rs`、`src/dictation/memory.rs` 及既有评测测试 | 通过生产初始化的保留结果检查种子是否被裁掉，避免另抄一份有效期或保留规则；保留种子独有的来源、重复身份和快照预算校验。 |
| 相关架构说明、`changelog` 与本记录 | 仅更新本次行为变化及实际验证结果；保留已有修改。 |

只打开与创建入口必须共享校验规则，但不为它们引入泛化存储框架。校验 manifest 的
schema、子目录所有权等现有约定继续生效。允许正常使用指向数据集根的路径别名，
不把拒绝 sidecar 链接扩大为禁止所有路径别名。

种子校验优先比较生产初始化前后的观察保留数量；生产 prune 只筛除观察，不修改其内容。
如需暴露保留状态，使用 dictation 内最小可见范围，不向 UI 或公共 crate API 暴露记忆内部。

### Alternatives and rationale

- Realtime 启用时强制显式配置 URL 也可避免默认连接开发机器，但会改变省略 URL 的配置
  契约，并影响借助默认配置校验词表的评测代码。本次采用已公开的回环默认值，改动更小。
- 仅在 correct 中拒绝链接会留下 show/list 与 validate 的规则差异；统一读取边界可消除
  这个差异。完整的并发恶意文件替换防护需要另行设计，不混入本次修复。
- 在 CLI 各分支单独调用 exists 会复制规则，并可能继续补建不完整数据集；只打开入口
  将读取消费者与创建者的职责分开。
- 在 seed 中新增一条 24 小时判断改动很少，但仍需同步生产规则；直接检查生产保留结果
  同时覆盖以后生产筛选条件的变化，保留评测特有的严格限制。
- 保留测试专用投递副本会继续允许生产权限判断与测试脱节，故删除副本并测试真实 worker。

### Costs, risks, and mitigations

依赖隐式创建的脚本会开始报错，需要先通过 record 初始化数据集；不提供隐藏回退。
已把旧 LAN URL 写入配置的用户仍使用其显式地址，避免擅自覆盖用户配置。
长度日志减少对正文的诊断信息，同时避免成功日志记录听写内容；不宣称所有错误消息及
第三方库日志都经过敏感信息审计。

数据集继续沿用单进程所有权约定，不承诺抵御校验后由恶意并发进程替换文件的竞态，
也不额外引入锁或修改持久化原子性。Windows 原生运行在本机不可验证，必须分别记录
可用的静态或交叉编译检查与未执行的原生检查。

### Implementation order

1. 在同一数据集变更中修复 sidecar 边界及只打开入口，加入最小回归并验证 CLI。
2. 修改 Realtime 默认地址与两个成功日志字段，检查显式配置不受影响。
3. 删除测试专用投递副本，将负向权限场景接入实际确认 worker。
4. 复用生产保留结果校验 seed，加入临界过期与未过期对照。
5. 更新相关文档，执行整体验证，按代码准则检查 diff，并通过本地 Claude 做针对性复审。

上述顺序须在计划获明确批准后执行。提交、推送及草稿 PR 的创建另待授权；如果后续发布，
六项修复使用同一个新 bookmark 与同一个针对 master 的 PR，不携带未授权的既有工作。

### Validation and ablation strategy

- **数据集回归：** 用临时生成的合成 WAV 与合法 JSON 构造外部 sidecar 链接，断言拒绝且
  外部字节未变；用不存在或不完整路径验证只打开入口不创建文件，已有正常数据集作对照。
  复用 CLI 临时复现脚本覆盖 validate 和 sample 消费者，不访问个人数据。
- **权限回归：** 向真实 worker 发送 Confirm/Cancel，断言普通错误文本不产生
  PermissionRequired；继续运行现有类型化权限拒绝正向测试与取消测试，不操作桌面。
- **seed 回归：** 固定时间比较年龄为 86,399 秒与 86,400 秒的同一观察，验证首次请求前
  的拒绝边界。继续运行既有快照预算、来源及隔离测试。
- **配置与日志：** 验证默认 URL 和显式 URL；通过现有 STT 模拟服务及静态日志字段检查
  确认正常返回正文不变、成功日志不再引用正文。确定性日志替换不单独增加永久测试。
- **整体验证：** `cargo fmt --check`、`cargo test --locked --offline`、
  `cargo clippy --locked --offline --all-targets --features integration -- -D warnings`、
  `git diff --check`。Windows 检查按已安装目标与工具链实际可用性执行并记录限制。
- **消融：** 每个新增行为回归临时撤掉对应修复，确认同一场景在旧实现失败、恢复后通过，
  不保留临时回退或重复测试框架。移走测试副本后检查没有测试继续调用它。
  本次不增加需要性能消融的新算法或机制。
- **时机：** 修复后、自动复审前执行上述对照；如复审带来改动，重跑受影响检查。

### Open questions and deferred work

待确认事项：是否批准上述六项修复方案，尤其是采用已有文档的回环默认 URL。
计划批准前不写业务代码或测试；本地计划尚未提交、推送或创建 PR。

延后事项：全部 P3、真实语音质量评测、Windows 实机验证、已有显式 LAN 配置迁移、
并发恶意文件替换防护，以及超出这两个成功事件的完整日志隐私审计。

## 2. PR change timeline

### [001] — 2026-10-19 — 六项 P2 修复与本地回归验证

- **Implemented changes:** 数据集新增严格只打开入口，创建入口共用同一套目录和 manifest
  校验；CLI 仅 record 保留创建能力。manifest、音频和样本 sidecar 复用普通文件与规范化
  路径归属校验，show/list/correct 在读取 sidecar 前拒绝符号链接。默认 Realtime 地址
  改为已公开的回环值，不迁移显式配置。HTTP STT、其 mock 和 Windows 注入成功日志
  统一只记录 Unicode 字符数。删除测试专用 deliver_text，负向权限与取消用例改用真实
  确认 worker，并覆盖捕获目标时的普通错误。seed 初始化后比较保留观察数，删除被生产
  裁剪替代的重复字段及数量规则，保留 seed 来源、唯一身份、四语境和快照预算限制。
- **Decisions and consequences:** 创建与消费职责分离，无隐式修补回退；不完整数据集也会
  失败。生产 prune 只删除观察，保留数相等足以验证没有观察被初始化丢弃，无需暴露观察
  内容或另建保留策略。既有退出修复保留；不新增依赖、协议或存储 schema。
- **Tests and comparisons:** 先在未修复业务代码上运行新增回归，确认 sidecar 越界写、
  validate 隐式创建和 24 小时 seed 过期三项失败。实现后又对同样的独立场景去掉 sidecar
  边界、恢复 validate 的创建入口、移除 seed 保留数检查，三个对应测试分别失败。
  另外临时把真实 worker 的权限判断改为匹配错误文字，新负向测试失败，证明测试实际
  覆盖生产判断。临时变体全部还原，相关 diff 摘要与实验前一致；没有保留变体或测试框架。
- **Validation:** 还原后 `cargo test --locked --offline -q` 为 306 项通过、2 项忽略；
  `cargo fmt --check`、`cargo clippy --locked --offline --all-targets --features integration -- -D warnings`、
  `cargo build --locked --offline`、`git diff --check` 均通过。已有 rdev 依赖仍有两项
  FFI/static-mut 警告，不宣称依赖无警告。
- **CLI evidence:** 临时 HOME、合成数据及本地模拟 STT 下运行 29 个 CLI 场景，覆盖五个
  消费命令对四类缺失数据集结构的不创建/不修补、四个 sidecar 链接拒绝、四个普通文件
  正向对照和一次正常 STT 日志脱敏；另验证数据集根路径别名仍能校验。STT 返回含中文和
  emoji 的固定文本，正常 CLI 输出保留原文，成功日志只含长度。单独编译实际 config
  模块验证默认关闭、回环 URL，以及显式 ws/wss URL 不被改写；不发起 Realtime 连接。
- **Local artifacts:** `.amp/in/artifacts/p2-before-tests.txt`、`p2-final-checks.txt`、
  `p2-cli-verification.json`、`p2-config-probe.txt`；临时对照日志在 `.amp/in/p2-fixes/`。
  这些本地文件不进入版本控制，也不包含个人配置、录音或凭据。
- **Claude review:** 本地 Claude Code Opus 5.5 复审修复 diff、相关调用链、测试和计划，
  结论为六项修复可以接受，未发现 P1/P2；一条可选 P3 建议为缺失目录错误增加具体路径。
  已核实当前 I/O 错误确实不附带路径，本轮按范围不改 P3。模型指出未获提供架构和
  changelog diff；主代理已另行检查这些文件的更新，不把材料缺失算成代码缺陷。
  初始化及实际响应均为 `claude-opus-5-5`，tools/MCP 均为空，退出为 0 且结果非错误。
  报告与校验元数据位于 `.amp/in/artifacts/p2-claude-review.md` 及同名前缀的 validation JSON。
- **Limitations and pending:** GNU Windows 交叉检查被 ort-sys 不提供该目标预编译库阻塞；
  MSVC 检查被本机缺少 Windows C 标准头文件阻塞，均未进入完整应用检查，不能视为
  Windows 编译通过。未运行 Windows 实机、GUI、真实 STT 或语音质量评测。代码与复审
  已在本地完成；没有提交、推送、创建 PR 或改动现有 bookmark。

### [002] — 2026-10-19 — 将独立修复集成到当前 master

- **Integration:** 在独立 jj workspace 中只提取本轮六项 P2，重放到
  [当前 master](https://github.com/b1indsight/viberwhisper/commit/c69049218e2f43d043fa966f321330ca713cf88e)。
  原工作副本及 `feat/confirmed-text-learning` bookmark 保持不变；已有运行时退出修复已在
  master 中，本 PR 不重复提交。仅手工合并 changelog 顶部两条独立记录，无业务代码冲突。
  保留 master 新增的稳定前缀、评测输入布局和对应测试。
- **Validation:** 在集成后的源码上重新执行 `cargo fmt --check`、
  `cargo test --locked --offline -q`（313 项通过、2 项忽略）、
  `cargo clippy --locked --offline --all-targets --features integration -- -D warnings`、
  `cargo build --locked --offline` 及提交 diff 空白检查，均通过；使用新构建二进制重跑
  29 个隔离 CLI 场景和数据集根路径别名对照，均通过。既有 rdev 警告及 [001] 的 Windows
  验证限制仍适用，没有因重放而宣称 Windows 验证通过。
- **Evidence:** 原工作副本的 `.amp/in/artifacts/p2-pr-base-checks.txt` 和
  `p2-pr-cli-verification.json` 记录本次集成验证，均不进入版本控制。
- **Publication review:** Claude Code Opus 5.5 静态复审
  [实际发布的代码提交](https://github.com/b1indsight/viberwhisper/commit/dee3de11638bb743aafae8218838dd04dafd8eee)
  及随附的相关源码，未发现新的可执行 P1/P2/P3。初始化与实际响应均为 `claude-opus-5-5`，
  tools/MCP 为空，进程退出 0 且结果非错误；测试由主代理独立执行，不能归为审阅者执行。
  报告及校验元数据在原工作副本的 `.amp/in/artifacts/p2-pr-claude-review.md` 和
  `p2-pr-claude-review.validation.json`。发布后仅以文档提交补充 PR 链接和此结果，业务代码未改。
