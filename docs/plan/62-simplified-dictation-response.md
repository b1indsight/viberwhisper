# 62 - 简化听写响应结构 — Feature record

**Stage:** Planning；方案待用户审批，未修改产品代码或测试。
**PR:** [本分支的方案 PR](https://github.com/b1indsight/viberwhisper/pulls?q=is%3Apr%20head%3Arefactor%2Fdictation-response-schema)（draft，目标 `master`）。
**Bookmark:** `refactor/dictation-response-schema`，后续方案与实现沿用同一个 PR。

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
