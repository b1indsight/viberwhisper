# 62 - 简化听写响应结构 — Feature record

**Stage:** Implementation；产品代码及本地验证已完成，实现尚未推送或合并。
**PR:** [#161](https://github.com/b1indsight/viberwhisper/pull/161)（draft，目标 `master`）。
**Bookmark:** `refactor/dictation-response-schema`，后续方案与实现沿用同一个 PR。

**当前修订：** [003](#003--2026-10-28--分离八项前缀校验并消除重复扫描) 修复八项
上限的评测冲突与全文定位的重复扫描；[002](#002--2026-10-28--实现字符串协议与保守定位)
已按 [001](#001--2026-10-28--未知语境使用保留字符串) 实现保留字符串 `"未知语境"`。
原始方案与修订历史保留，冲突处以已批准的后续修订为准。

## 1. Initial plan

### Summary and motivation

用户要求去掉 `context.kind`，将 `memory_candidates` 改成字符串数组，按出现顺序输出
专有名词。用户怀疑复杂格式影响模型推理；目前没有同条件模型对照，不能把这一判断
当作已经验证的性能或准确率结论。

[当前响应解析](../../src/dictation/result.rs)要求语境区分 `existing/new/uncertain`，
候选包含 `kind/text/quote`。[HTTP 后处理](../../src/postprocess/llm.rs)与 Realtime
共享这一约定，因此必须同步修改提示词、解析、确认学习和评测，避免只改变示例后
导致候选全部失效。

### Goals, scope, and success criteria

- 模型只需返回话题字符串或 `null`、有序专名字符串数组及正文，候选不再包含对象。
- Realtime 与普通 HTTP 后处理采用同一新协议；流式正文提取和成功投递后的学习继续工作。
- 客户端只对能可靠定位的完整专名应用同语境纠错；无效元数据不丢失有效正文。
- 不改动正文保真、明确改口、数字和独立否定规则；不增加模型调用、依赖或配置开关。
- 保留输入语境快照、稳定词表 → 音频 → 动态语境的请求顺序、确认界面和存储 schema 2。
  人工词表与存量记忆中的 `TermKind` 不属于本次输出结构简化范围。

### Proposed behavior

输出仍固定为 `context`、`memory_candidates`、`text`，例如：

```json
{
  "context": "编程工具",
  "memory_candidates": ["Codex", "Redis", "Codex"],
  "text": "用 Codex 连接 Redis，再让 Codex 检查。"
}
```

- `context` 是简短话题标签，沿用已有话题时原样返回其标签；明确的新话题使用新标签。
  无法确定时返回 `null`，且候选为空数组。不再要求模型声明新建还是复用。
  当前快照的 `id` 与标签相同，客户端继续用标签隔离记忆，不改输入对象结构。
- 候选只提取最终正文中的完整专有名词，不再自动提取 `phrase`。按正文出现顺序逐次列出，
  同名多次出现也重复列出，不包含被改口删除的旧词或只在参考中出现的词。
- 每项沿用最多 80 个 Unicode 字符的限制；按出现顺序最多输出前 8 项，统一到解析器
  已有的 8 项边界，取代原提示词的“最重要的 2 个不同专名或固定说法”。
- 生产解析保留有效邻项；候选超出 8 项时沿用放弃整组候选的处理，不偷偷截断。
  无效语境、无语境或非数组候选均不触发候选纠错和术语学习；有效正文照常交付，
  有效语境下的确认正文仍可进入原有短期语境。
- JSON、正文类型或正文中的 NUL 等整体协议错误沿用原有失败/HTTP 原始 STT 回退。
  缺失或错误的可选元数据只降级元数据，评测仍将其判为协议不合格。

**去掉短句后的定位约定。** 复用现有 NFKC、大小写归一化和 Unicode 完整词匹配，
按归一化名称分组，先处理较长的完整名称，排除已由完整名称占用的内部片段。
只有某组列出的次数等于正文中剩余可匹配次数，且位置不自交叉时，才接受这些位置。
交叉重叠或不能完整对应的组跳过；最后按正文位置排序，供纠错和反馈复用。

例如正文出现两次 `Open Eye`、数组只列一次时，不猜模型选择了哪一次；该名称整组
跳过，但其他可定位专名仍有效。数组列出两次时可定位两处；部分修改导致的规则冲突
继续交给现有确认反馈合并逻辑，不产生互相矛盾的永久规则。

### Technical design and affected modules

**结果边界。** 在 `src/dictation/result.rs` 删除 `ContextChoice` 和 `CandidateWire`，
`DictationResult` 保留以下字段：

| 字段 | 类型 | 责任 |
| --- | --- | --- |
| `text` | `String` | 通过现有正文校验的原始模型正文 |
| `context` | `Option<String>` | 通过现有标签校验的话题；`None` 表示未知或无效 |
| `memory_candidates` | `Vec<Candidate>` | 已定位、按正文位置排列的完整专名 |
| `raw` | `serde_json::Value` | 保留原始模型对象，供评测检查类型、遗漏和原始顺序 |

内部 `Candidate` 只保留 `text: String`（模型提供的名称）和
`span: Range<usize>`（正文中的 UTF-8 字节范围）。保留名称本身以延续归一化等价拼写
的记忆语义，删除运行时 `kind`；不添加另一套 wire 类型或兼容模式。

计划接口：

- `DictationResult::parse(text: &str) -> Result<Self>`：Realtime 校验，保留现有 JSON
  及正文字节上限；不再接收仅供旧 `existing` 分支验证的 `context_ids`。
- `DictationResult::parse_transcript(text: &str) -> Result<Self>`：共享 HTTP 整篇解析，
  不引入 Realtime 分段长度限制。
- `fn locate_candidates(text: &str, names: &[String]) -> Vec<Candidate>`：结果边界内的
  有界候选定位，复用 `normalized`、`term_spans`、`word_boundary`。
- 删除 `context_label()`，调用处直接读取 `context.as_deref()`。
- 评测字段反序列化使用
  `fn deserialize_context_assertion<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>`，
  其中 `D: serde::Deserializer<'de>`，返回标准库 `Result`。外层 `None` 表示未要求
  检查语境，`Some(None)` 表示明确要求 `null`，`Some(Some(label))` 表示要求该话题。
  不增加依赖或带 `kind` 的断言对象。

| 模块 | 计划变更 |
| --- | --- |
| `assets/dictation-prompt.default.json`、`config.example.json` | 同步新协议、候选出现顺序及全部 JSON 示例；保留正文规则和现有提示词长度约束 |
| `src/dictation.rs`、`result.rs` | 移除旧输出类型及导出，统一保守定位；完整名与重叠处理只维护一份 |
| `src/dictation/correction.rs`、`memory.rs` | 消费已定位专名；删除仅为模型 `phrase` 候选存在的运行时分支，新候选存储为 `ProperNoun`；保留人工词表保护、存量文件读取和确认提交边界 |
| `src/dictation/transport.rs`、`evaluation.rs`、`learning.rs`、`src/postprocess/llm.rs` | 更新共享解析调用及相关测试数据；不改传输、预热、后台所有者或投递生命周期 |
| `src/prompt_lab/realtime.rs`、`regression.rs`、现有测试及 Realtime fixture | 迁移字符串断言，严格检查原始顺序，标识新协议并拒绝误用旧报告 |
| `README.md`、相关架构文档、`changelog` | 更新输出示例、定位限制、自定义提示词迁移和评测重跑说明 |

### Alternatives and rationale

- **只改提示词：** 无法通过现有对象解析，破坏纠错与记忆；必须调整共享消费链。
- **保留仅含 `label` 的语境对象：** 仍有一层无实际用途的包装；采用字符串或 `null`。
- **字符串一律替换全文：** 会把同名普通正文当作选中的专名；采用次数一致才定位的
  保守规则，不引入通用序列对齐或动态规划来猜测模型意图。
- **保留 `quote` 或同时接受新旧输出：** 能保留选择单次同名位置的能力，但保留模型
  格式负担或增加长期维护分支；本次统一切换新协议，并明确接受定位能力收窄。
- **删除全部 `TermKind` 或迁移记忆文件：** 会扩大到人工词表与存量数据，不采用。
  新模型不再生成短语候选；存量短语观察继续作为语境参考，人工词表中的短语继续保护。

[既有单次出现定位决策](58-confirmed-text-learning.md#008--2026-10-18--短句定位候选并收窄到候选内部的编辑)
说明了误替换和假撤销风险；本方案改变其输出证据来源，继续保留“不确定就不学习”。

### Costs, risks, and mitigations

- 字符串列表无法证明模型从多个同名位置中只选了哪一处。安全降级会减少部分学习，
  尤其是重复名称只输出部分、或前 8 项限制截断某个名称时；不以全局替换弥补。
- 模型可能乱序、漏项或多列重复项。生产侧只接受能定位的组，评测额外比较原始数组
  与正文位置顺序，避免客户端排序掩盖模型不遵循新协议。
- 完整名与短名相邻、包含或交叉时必须保守处理，沿用已有普通正文假撤销反例作为
  回归约束；不得因删除 `quote` 而扩大确认编辑的归属范围。
- 自定义 `dictation.prompt` 或 `prompt_components` 若仍要求旧格式，需要用户迁移。
  不自动改写实际用户配置；内置默认值及示例一起更新。旧格式元数据降级，正文保留。
- Realtime cases 升为 schema 2，policy 改为 `realtime-context-memory-v5`。旧版 Realtime
  cases/报告应在读取边界明确提示迁移或重跑，不拿旧输出按新规则重新计分；跨策略
  直接对比拒绝。非 Realtime 的历史报告兼容性不变，不扩展整套报告 schema。
- 简化结构可能减少格式生成负担，但不承诺识别准确率或速度提升；真实模型对照前
  不报告这类改进。

### Implementation order

1. 获得方案审批及具体发布授权，使用同一个 bookmark/draft PR 承载方案与实现。
2. 同步结果边界、候选定位及记忆/纠错消费，删除仅属于旧输出结构的类型和分支。
3. 更新两条推理路径共享的提示词与所有示例，调整现有传输和后处理测试响应。
4. 迁移 prompt-lab 断言及 fixture，更新 policy 和读取边界的不兼容提示。
5. 补充有真实回归风险的合同测试，完成对照验证，更新文档后再进入代码审查。

不迁移存储文件或个人配置。回滚时整体回退本次协议、提示词和评测修改；不得只回退
其中一端。尚未产生的新旧评测结果保持独立，不改写既有报告。

### Validation and ablation strategy

以下是实施后的检查计划，尚未执行：

- **响应合同：** 已有/新标签、`null`、错误元数据、混合数组、超过 8 项、空正文、
  非法 JSON，以及 HTTP 长正文与 Realtime 分段边界；确认有效正文不会变成协议文字。
- **定位反例：** 有序重复专名、同名只列一处、完整名和独立短名并存、交叉重叠、
  拉丁词内子串、中英文和兼容字形；对不能定位的项断言不纠错，并保留有效邻项。
- **学习闭环：** 从两条公开处理入口验证确认编辑 → 成功交付 → 同话题下次纠错；
  保留取消、跨话题、人工保护词、假撤销和重复候选冲突的现有行为测试。
- **评测可信度：** 乱序即使可被客户端重新排序也判 metadata 失败；漏字段、旧对象、
  无语境但有候选失败；语境断言缺省、显式 `null` 和字符串往返后仍互不混淆；
  旧 Realtime 输入拒绝，旧非 Realtime 报告继续可读。
- **消融：** 用固定正文比较直接全文替换、只取首次匹配、次数一致定位三种处理，
  以同名普通正文、重复同名和完整名包含短名为反例，验证定位约束是否必要；实验
  在实施后、代码审查前运行，不将临时脚本留在产品代码中。
- **模型对照：** 有可用且允许测试数据发送的服务时，用同一模型、同一音频/文本及
  同一语境分别跑旧/新提示词，记录协议合格率、专名顺序、正文保真、输出量和延迟。
  先使用公开合成数据；没有可用服务时明确限制，服务桩不作为推理质量改进的证据。
- **检查命令：** `cargo fmt --check`、`cargo build --locked --all-features`、
  `cargo test --locked`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、
  `git diff --check`。不改 UI 外观；Windows 原生运行验证取决于可用 CI/实机。

### Open questions and deferred work

待批准的约定是：`context` 使用字符串或 `null`、重复专名逐次列出且最多前 8 项、
无法可靠定位时跳过相关候选，以及旧 Realtime 评测需要重跑。没有需要另行收集产品
信息才能确定的阻塞问题。

规划咨询已通过受限 Claude Code 完成，初始化和响应均核验实际模型为
`claude-opus-5-5`；这些建议仅用于方案，不代表运行验证。

本次不做推理性能调优、自动迁移任意自定义 prompt、兼容旧模型响应、改写个人词表、
编辑事务采集或扩大永久学习范围。定位实现的局部写法可在上述合同内决定。

## 2. PR change timeline

<!-- 初始方案待审批；后续只追加实质性需求、实现和设计变更，不记录例行流程事件。 -->

### [001] — 2026-10-28 — 未知语境使用保留字符串

**需求与状态。** 用户要求用特殊名词表达未知语境。本修订拟采用精确字符串
`"未知语境"`，让模型的 `context` 始终为字符串。方案仍待整体审批；本次仅修订文档，
未修改产品代码或测试。使用该字符串能否改善模型遵从率，尚未经过模型对照。

未知语境示例：

```json
{
  "context": "未知语境",
  "memory_candidates": [],
  "text": "好，就按这个来。"
}
```

**替代关系。** 本条替代 Initial plan 中用 `null` 表达未知语境、相应兼容和验收约定，
以及新增 `deserialize_context_assertion` / 双层 `Option` 的设计；原始文字作为历史保留。
正文规则、按出现顺序输出专名、存储 schema 2、请求布局及其他范围保持原方案。

**选项与决定。** `null` 可以直接表达无值，但模型需在字符串和 null 两种类型间选择，
评测也需要区分省略与显式未知。本修订选择固定字符串作为未知标记，保留其他正常
话题字符串，不再引入自定义反序列化方法、三态枚举、别名或配置项。

**模型与客户端约定（待实施）：**

- 使用共享常量 `UNKNOWN_CONTEXT: &str = "未知语境"`，只按精确字面量匹配。
  默认提示词及其所有示例、配置示例、HTTP 协议说明与评测使用同一约定。
- 普通合法标签解析为 `Some(topic)`；`"未知语境"` 解析为 `None`。
  `DictationResult.context` 继续使用 `Option<String>`，`raw` 保留原始字符串。
- 未知时 `memory_candidates` 必须为 `[]`，不执行按语境纠错，不学习该话题、术语或
  纠错规则。有效正文及原有全局转写历史流程不受影响。
- 模型返回 `null`、缺失字段、错误类型，或未知标记搭配非空/非数组候选，均不符合
  新协议。生产路径保留有效正文、降级相关元数据；评测的 metadata 检查判失败，
  不将这些错误当作合法未知语境。

**评测断言简化（待实施）：**

- `Expectations.context` 使用 `Option<String>`：字段省略表示不检查；
  `Some("未知语境")` 要求原始输出正好是该字符串；`Some(topic)` 要求指定话题。
- 沿用普通可选字段的反序列化，评测配置里的 `context: null` 可等同于省略，
  不表示“必须未知”。这只是评测配置的可选断言，不是提供给模型的协议。
- 显式语境断言比较原始字符串，不比较运行时 `None`，避免合法未知和错误元数据
  被混为一类。取消计划新增的 `deserialize_context_assertion`，无需双层 `Option`。

**保留名与兼容边界（待实施）：**

- `"未知语境"` 仅用于协议控制，不能作为真实话题标签。`MemoryConfig::validate`
  对同名人工词表语境给出需改名的明确错误；不自动修改用户配置。
- 不修改通用 `valid_label`：它还用于场景 ID 和现有记忆修剪，不能把协议保留名的
  限制扩大到这些用途。
- 在 `Memory::untrimmed_snapshot` 汇总全部来源的标签后、`take(4)` 前排除保留名，
  不让旧的同名观察或纠错占用语境槽位。旧记录仍可读取、沿用既有修剪规则，
  不做自动改名或专项数据迁移，也不把它们作为话题发送给模型。

**定向验证计划。** 覆盖正常话题和未知标记、错误 null/缺失/类型、未知却带候选，
以及断言缺省、未知字符串、普通字符串的序列化往返。验证同名人工配置被拒绝，
通用标签及场景 ID 校验不变；用旧同名记录排在前面的快照证明过滤不挤占 4 个有效
话题槽位，初始化/构造快照不改写源文件。模型对照固定模型与音频，只改变未知表示，
记录协议遵从情况；未执行前不宣称质量或性能改善。

**验证状态。** 本次为文档修订，产品测试和真实模型比较尚未执行。规划咨询的初始化
和响应均核验实际模型为 `claude-opus-5-5`；咨询建议不作为运行验证。

### [002] — 2026-10-28 — 实现字符串协议与保守定位

**需求与实现。** 落实原方案及 [001](#001--2026-10-28--未知语境使用保留字符串)：
Realtime 和 HTTP 后处理共用话题字符串、专名字符串数组、正文的新输出协议。
[结果边界](../../src/dictation/result.rs)删除 `ContextChoice`、候选 wire 对象及
`context_label()`；两条解析入口不再接收语境 ID 列表。未知语境直接得到 `None`，
保留原始 JSON 供评测核对；没有引入 `deserialize_context_assertion` 或双层 `Option`。
候选只保留模型拼写与已校验的 UTF-8 范围；无效元数据不丢弃有效正文。

[纠错](../../src/dictation/correction.rs)和[记忆](../../src/dictation/memory.rs)
共同消费结果边界给出的互不重叠位置，删除运行时短语分支和第二次重叠选择。
新观察统一为 `ProperNoun`，人工词表与存量短语仍保留原有 `TermKind`，存储 schema 2
不变。未知语境不能应用或学习按话题隔离的记录；保留名人工词表返回需改名的错误。
旧同名记录仍能读取，在选择四个话题前过滤，不改写原文件或通用 `valid_label`。

默认提示词和配置示例同步新格式；原有 task/language/numbers/cleanup 及十六个示例的
正文不变，新增重复专名示例。请求布局、正文流式提取、确认界面和成功交付边界不变，
没有新增推理调用、依赖或配置开关。

**定位选择与代价。** 同名数组必须覆盖该名称全部可定位出现，不能从多个同名位置
中任选一处。完整名称先占用其内部片段，即使完整名称本组次数不合法，也不让短名
借该位置纠错；交叉和自重叠组跳过。这样会少学部分名称，但避免误改正文或学习片段。
有效邻项继续保留，超过八项则丢弃整组候选。生产结果按位置排序，评测检查原始顺序，
不会用客户端排序掩盖模型输出不合格。

**消融设置与结果。** 使用临时 Rust 单测 `schema_grounding_ablation`，复用正式
`term_spans` 与 `DictationResult::parse`，对所有策略提供相同文本和名称数组；预期
UTF-8 范围独立手写。八组固定输入包括：同名只列一次、完整重复、完整名加独立短名、
交叉、自重叠、完整名次数错误但包含短名、普通唯一名、全角 Unicode 对照。

| 策略 | 本组固定用例满足定位合同 |
| --- | --- |
| 按名称取全部完整词匹配 | 3/8 |
| 每个名称只取第一次匹配 | 2/8 |
| 只检查次数，不处理包含或重叠 | 4/8 |
| 正式定位：完整名优先、次数一致、排除交叉和自重叠 | 8/8 |

例如 `New York; York` 必须保留完整名与独立短名，不能把完整名内的 `York` 计为
独立出现；完整名次数错误时，短名也不能在其内部获得修正规则。因此保留上述机制。
执行命令为 `cargo test --locked --lib schema_grounding_ablation -- --nocapture`；
输入、预期与各策略实际范围保存在[本地实验记录](../../.amp/in/artifacts/dictation-schema-ablation.log)
（不提交）。临时源文件和 include 已删除；正式回归可用
`cargo test --locked --lib candidate_groups_require_unambiguous_complete_occurrences` 复现。
这些是客户端合同实验，不是模型识别率或推理速度对照。

**评测迁移。** [Realtime 评测](../../src/prompt_lab/realtime.rs)使用
`Option<String>` 语境断言；省略/null 不检查，保留字符串检查原始输出。候选断言为
字符串数组，快照和 seed 术语仍有类型。cases schema 2 与 policy
`realtime-context-memory-v5` 在嵌套反序列化前校验；旧 Realtime cases/报告需迁移并
重跑，不能按新规则重计分。直接类型校验和比较入口也拒绝旧策略，非 Realtime
schema-1/schema-2 报告仍可读。回放必须包含当前 `prompt_input` 证据。

**验证与限制。** 新重复专名回归先在旧产品代码上失败，再在新代码上通过；覆盖未知
语境、保留名配置、四话题容量、存量文件不改写、候选界限、确认学习、两种 HTTP 会话、
Realtime 传输与评测元数据。内置提示词与示例相同，组合后低于 4096 字符上限。
`cargo fmt --check`、`cargo build --locked --all-features`、`cargo test --locked`、
`cargo clippy --locked --all-targets --all-features -- -D warnings` 和 `git diff --check`
均通过；第三方 rdev 的既有 macOS 警告未改动。
[完整本地检查记录](../../.amp/in/artifacts/dictation-schema-validation.log)不提交。
未运行真实模型的新旧提示词对照，也未进行 Windows 实机验证；不宣称识别质量或延迟
改善。两项依赖外部数据/服务的既有测试仍为 ignored。本次不更改 UI 外观。

### [003] — 2026-10-28 — 分离八项前缀校验并消除重复扫描

**触发与状态。** 独立审查指出：正文有九次同名出现时，按提示词提供八个字符串会因
完整组被丢弃而误判 metadata；全文定位的边界与包含检查还会重复扫描。用户要求修复
并定位复杂度原因。本条已在本地实现和验证，尚未推送。它修正 [002] 中将元数据合格
等同于全部候选可用于纠错的约定，保留“不完整名称组不纠错、不学习”的安全边界。

**原因与实现。** 旧 `term_spans` 每次命中都会调用 `word_boundary`，后者为起点和终点
分别从正文开头重新枚举 Unicode 边界；短名检查又对每处命中从头遍历此前完整名位置。
当出现次数随正文增长时，两处都能产生平方级工作量。旧交叉检查受次数一致条件约束，
并不是此次无界平方增长的第三个来源。

[结果边界](../../src/dictation/result.rs)现在为每次名称扫描建立边界表，匹配循环查询
为 \(O(1)\)；包含和交叉检查使用源位置有序的游标扫描。候选最多八项、每项最多八十
字符，因此完整定位在正文长度上为 \(O(n)\) 时间和空间；读取正文及统计全部同名位置
本身不能是 \(O(1)\)。前八项校验每组只收集前八个位置，总共最多排序六十四个位置，
不对全文全部命中做排序。

共享解析同时计算内部 `metadata_valid: bool`，不添加模型 JSON 字段。少于八项时，
数组必须按顺序覆盖所提供名称的全部有效出现；恰好八项时，允许是这些名称的前八次
出现。名称不存在、乱序、包含片段、交叉、自重叠、非法字段及超过八项仍判失败。
[评测](../../src/prompt_lab/realtime.rs)直接使用该判定，不复制定位逻辑，也不将
“数组长度为八”一概放行。`memory_candidates` 继续只含次数完整的安全名称组；
`candidates_present/absent` 仍断言这些保留候选，而非所有原始数组项。

**消融与实测。** 临时单测 `candidate_grounding_scaling` 在同一 release 构建中比较
修复前源码、只缓存 Unicode 边界、正式修复三个版本。固定正文为 `Codex `、
`New York; York; `、`ordinary words ` 的重复，分别使用八个 `Codex`、
`["New York", "York"]`、`["Codex"]`；都断言正文保留且不返回次数不完整的候选。
每组解析三次取中位数，包含 JSON 和候选定位，不含输入构造。下表为完整名与独立短名
交替输入的实测毫秒：

| 正文字节 | 修复前 | 仅缓存边界 | 正式修复 |
| --- | ---: | ---: | ---: |
| 16 KiB | 325.384 | 2.158 | 1.557 |
| 32 KiB | 1330.335 | 5.104 | 3.152 |
| 128 KiB | 未运行 | 44.757 | 12.884 |
| 256 KiB | 未运行 | 152.586 | 25.526 |

仅缓存边界仍保留随重复次数扩大的包含遍历；正式版本在本组输入翻倍时接近两倍耗时，
因此保留游标扫描。单名称与无匹配对照也通过；以上是本地合成解析延迟，不是模型推理
或端到端听写延迟。实验命令为分别设置 `GROUNDING_PHASE=before/boundary_only/after`
并执行 `cargo test --locked --release --lib candidate_grounding_scaling -- --ignored --nocapture`。
固定种子 `0x12de1a00b3ff259a` 的临时 `candidate_grounding_equivalence` 比较含中文、
兼容字形、组合字符、重复、包含、交叉及畸形元数据的输入，正文、话题、原始 JSON 和
保留名称的拼写/UTF-8 范围均与修复前一致；元数据仅增加合法八项前缀这一类通过情况。
[完整消融与差分记录](../../.amp/in/artifacts/dictation-grounding-ablation.log)不提交。
实验入口、临时源码与原始咨询流均已移除，正式代码不依赖这些临时文件。

**回归与验证。** 新增公开解析/评测回归
`capped_candidate_prefix_is_valid_without_authorizing_partial_name_groups`，先复现九次出现
配八项时失败，再验证修复通过；同时区分七/八/九项、有效邻项、乱序、虚构和重叠。
原有纠错、人工保护和确认学习检查继续通过。清理实验源码后重新执行
`cargo fmt --check`、`cargo build --locked --all-features`、`cargo test --locked`、
`cargo clippy --locked --all-targets --all-features -- -D warnings`、`git diff --check`
均通过，见[本地完整复验](../../.amp/in/artifacts/dictation-grounding-validation.log)。
既有外部依赖测试仍为 ignored，第三方 rdev 警告未变；未扩大真实模型或 Windows 原生
验证范围，也没有修改提示词、配置、请求顺序或学习权限。
