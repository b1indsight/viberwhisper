# 61 - 普通后处理复用改口与纠错学习 — Feature record

**Stage:** 本地实现完成；客户端回归通过，真实模型样例与跨平台验证限制见时间线。
**PR:** 尚未创建。本次改动已隔离到基于 `master@origin` 的独立 jj 工作区；原工作区的其他修改保持原样，尚未提交或推送。

## 1. Initial plan

### Summary and motivation

用户要求把当前流式的改口和修复错误逻辑接入普通流程后处理，使普通推理也有相同能力。
本方案将“相同能力”理解为同时覆盖口述改口整理与用户确认后的专有词纠错学习；这项范围
需要在实施前确认。如果只需要同步改口提示词，可以缩减为独立的小改动。

目前这两项能力分属不同层：

- [Realtime 默认提示词](../../assets/dictation-prompt.default.json)保留原意、处理明确改口、
  区分独立否定，并要求返回语境和带原文短句的候选。它不允许凭常识猜测或事实纠正。
- [纠错模块](../../src/dictation/correction.rs)与[记忆模块](../../src/dictation/memory.rs)
  根据成功投递后的人工编辑，学习按语境隔离的整词替换，并修正后续预览。
- [普通后处理](../../src/postprocess/llm.rs)使用较弱的独立提示词，HTTP chat completions
  只返回纯文本。[桌面收尾](../../src/ui/listener/event_loop.rs)没有从该结果获得语境、
  候选和确认反馈能力，因此无法进入现有学习闭环。

### Goals, scope, and success criteria

- 普通 HTTP STT → LLM 后处理能整理明确改口，例如“订七张票，哦不，订八张票，都选
  靠窗的”得到“订8张票，都选靠窗的”；保留“我没让你发送邮件，只是保存草稿”中的否定。
- 普通桌面后处理返回经过校验的语境和候选，复用现有专有词修正、人工反馈、规则撤销
  与持久化。用户将“维伯耳语”改成“ViberWhisper”并成功投递后，同语境中模型再次明确
  提取该完整误词时，预览自动修正；重启及切换识别路径后仍可使用同一规则。
- 保留现有边界：无可靠语境或候选不学习；只替换本次被定位的整词；数字、格式、含糊
  改写和跨边界编辑不生成永久规则；自动修正不被误记为新的人工编辑证据。
- 不额外增加一次 LLM 推理，不新增依赖、存储文件或配置开关。Realtime 识别、分段、
  流式预览和服务端协议保持现有行为。
- CLI `convert` 和 setup 可使用同等改口整理规则，但不读取或更新个人记忆；原始采集和
  普通 prompt-lab 回归继续隔离个人记忆。关闭后处理时仍直接交付 STT 文字。

### Proposed behavior

1. 桌面选择普通识别且启用后处理时，在现有 chat completions 请求中提供有界语境快照。
   同一次推理完成文字整理、语境选择及候选提取，客户端仅把最终正文送入预览。
2. 默认整理规则与 Realtime 对齐，针对输入已经是 STT 文本做措辞适配。仅合并明确改口、
   删除明确无意义的语气词及卡顿，保留其他内容、专名、否定和未说完的片段；不回答或
   执行转写文本中的指令，不凭常识修补无法从文本确认的识别错误。
3. 使用已有语境与候选校验、本地修正生成初始预览。成功投递后把用户最终文字提交给
   同一套反馈逻辑；history 与注入继续使用这份最终文字。
4. 取消、清空、关闭预览、识别失败和后处理失败不学习。投递失败保留编辑及重试，成功
   后仅提交一次反馈；下次录音取得快照前先处理已接受的反馈。
5. 请求失败、空结果或整体 JSON 无效，沿用普通后处理的原始 STT 回退，不把协议内容
   当正文输入。语境或个别候选无效时沿用现有逐项降级，保留有效正文。

### Technical design and affected modules

**规则复用。** 为普通文本整理提供与音频听写对应的任务说明；保留现有改口、独立否定、
原词保真和数字规则。抽取确实需要同步的规则及示例，避免维护两份大段且会漂移的逻辑。
音频任务说明和普通文本任务说明分别保留，不把音频、记忆协议无差别塞入旧纯文本入口。

**结构化后处理。** 桌面普通后处理采用与 Realtime 相同的 `text`、`context`、
`memory_candidates` 数据约定，复用 `DictationResult::parse` 和候选 `quote` 定位校验。
不要求提供商支持特定 `response_format` 扩展，继续使用 OpenAI-compatible
chat completions 和 `stream: false`。预热与保守模式使用相同的结果解析，只有最终有效
一代结果能进入预览与学习，过期预热结果不能写入记忆。

**确认与记忆所有权。** 通过 dictation 模块提供最小的内部入口复用 `Memory::prepare`、
`Memory::confirm` 和 `Confirmation`，不复制 diff 或整词替换算法。普通桌面路径由后台
所有者串行处理快照、准备预览及最终反馈；Realtime 保持连接控制消息与确认等待的语义。
每个活动路径只有一个记忆写入者，文件 I/O 不放到 UI 线程；正常退出排空已接受的反馈，
放弃未确认结果，不引入等待用户输入的退出阻塞。

**配置与数据。** 复用 `dictation.memory` 和同一份 `dictation-memory.json`；
`dictation.enabled` 继续只选择识别路径。`memory.enabled = false` 同时关闭普通桌面的
个人快照、学习和本地纠错。启用普通后处理不会校验或连接未使用的 Realtime URL。
存储 schema 保持 2，不迁移或改写 `memory.long_term`。

**自定义提示词与入口隔离。** 保留 `post_process.prompt` 的自定义整理用途，桌面新增的
结构化输出契约由协议说明明确约束，文档给出所需字段，不能依赖从任意纯文本猜元数据。
CLI/setup 的公开纯文本返回接口保持不变，且不创建个人记忆所有者。自定义规则与输出
契约冲突时按后处理失败回退原始 STT，不生成伪造的语境或候选。

| 模块 | 计划变更 |
| --- | --- |
| `src/postprocess.rs`、`src/postprocess/llm.rs` | 对齐普通默认整理规则；增加桌面结构化结果处理，保留纯文本调用及两种请求模式 |
| `src/dictation/config.rs`、默认提示词资产 | 提供可共享的规则边界，保持 Realtime 提示词内容与行为 |
| `src/dictation.rs`、`memory.rs`、`feedback.rs` 及必要的后台所有者 | 最小共享入口与普通路径确认闭环；复用现有候选校验和纠错实现 |
| `src/core/listener.rs`、`src/ui/listener.rs`、`event_loop.rs` | 只在启用普通桌面后处理时装配记忆配置；传递带反馈的预览结果与关闭生命周期 |
| `README.md`、架构文档、`changelog` | 说明两条路径的纠错范围、共享存储、配置语义与隐私边界 |

### Alternatives and rationale

- **只复制提示词：** 可使普通模型处理明确改口，改动很小，但不能获得人工编辑后的
  持久纠错能力。若用户只需要这一层，应缩减方案，不引入结构化结果与记忆所有者。
- **从 STT 文字全局套用替换表：** 缺少语境与候选位置，容易修改同名普通正文；不采用。
- **在现有后处理之后增加一次元数据推理：** 增加延迟、费用和两份文字的对齐问题；
  同一次后处理即可返回所需元数据，不采用。
- **复制整套纠错和存储：** 两条路径将逐渐产生不同的边界规则，用户修正也无法互用；
  选择复用既有实现和存储，同时保持传输各自独立。

### Costs, risks, and mitigations

- 普通后处理服务可能是云端。新增语境快照会把历史短句、术语与已确认纠错发送给配置的
  后处理提供商；这是本方案的重要数据流变化，需要用户批准。关闭 `dictation.memory`
  可禁用个人快照；日志仍仅记录长度、状态和错误，不输出正文、快照或密钥。
- 普通后处理只有 STT 文本，无法重新听音频。相同改口与记忆机制不保证与端到端音频
  模型同样的识别质量，也不承诺修复缺失证据的陌生词。
- JSON 输出增加 token 和模型遵循格式的要求。协议失败保留原始 STT，元数据逐项校验；
  自定义提示词需要遵守新的桌面输出契约，必须在文档中说明。
- 后台所有者与反馈时序是主要实现风险。使用现有单次确认契约，验证取消、失败重试、
  退出排空及下一次快照顺序，不新增独立永久状态或第二套替换算法。

### Implementation order

1. 确定共享整理规则与桌面结构化入口，验证普通请求携带规则、候选校验及原始文本回退。
2. 接入单写入者记忆与现有确认句柄，打通普通桌面的准备、成功投递反馈及退出处理。
3. 验证预热/保守模式、自定义提示词、关闭记忆、CLI/setup 和原始采集隔离。
4. 更新文档，完成回归与消融，再按同一方案对应的 bookmark/PR 交付；不混入已有工作区修改。

### Validation and ablation strategy

以下均为计划，尚未执行：

- **请求与降级：** 用本地服务桩检查 chat completions 的系统规则、语境快照及结构化
  响应；覆盖正常结果、无效 JSON、空正文、错误候选和自定义提示词，保证协议不被注入。
- **普通路径闭环：** 通过后处理和确认入口验证误词 → 人工确认正确词 → 下次同语境
  自动修正 → 重启后仍生效；验证切换至 Realtime 可使用同一条规则。
- **反例及生命周期：** 无候选、不同语境、同名正文、词内子串、数字标点、含糊编辑不
  误学；取消、清空、后处理失败不写入；投递失败重试只学一次，下一请求读取新反馈。
- **隔离：** 两种后处理模式均有效；记忆关闭不读写个人数据；后处理关闭不调用 LLM；
  CLI、setup 和原始采集不加载个人记忆；退出不等待尚未确认的预览。
- **消融：** 相同服务响应下比较仅整理提示词、加入语境快照、再加入确认反馈与本地
  修正，证明最后一种能修复模型再次返回的已知误词；移除语境/候选边界，对照误替换反例。
- **真实模型：** 若有可用后处理服务，用改口、独立否定、专名保真、未完成句与指令
  文本测试集比较旧/新提示词。服务桩只证明客户端机制；无法进行真实模型验证时明确记录。
- **检查：** `cargo fmt --check`、相关模块测试、`cargo test --locked`、
  `cargo clippy --locked --all-targets --all-features -- -D warnings` 与差异空白检查。
  不改变预览外观；桌面验证关注确认反馈与交付行为，Windows 原生能力由可用 CI/实机验证。

### Open questions and deferred work

待批准的范围：同时接入口述改口规则与人工确认后的持久纠错，并允许普通后处理服务在
记忆启用时接收有界个人语境快照。若仅需提示词能力，缩减方案后再实施。

不包含无证据的事实纠错、重新听音频修复 STT、CLI 自动学习、跨话题全局替换、整段改写
学习、词表管理 UI 或服务端改造。内部共享入口和后台所有者的具体类型在实施时确定，
不扩张为与本功能无关的架构重整。

## 2. PR change timeline

### 2026-10-19 — 普通后处理使用共享规则与结构化结果

- **需求与实现：** 按完整接入范围，将普通 HTTP 后处理接入同一套改口、候选定位、
  语境隔离和确认学习。`PromptComponents::postprocess_prompt` 直接组合默认资产中的
  语言、数字、清理、输出及记忆规则，单独适配 STT 文本任务；没有修改 Realtime 提示词
  资产或其组合方式。
- **协议决定（已实施）：** 同一次 chat completions 返回 `memory_candidates`、
  `context`、`text`，复用 `DictationResult::parse`。不增加元数据推理，不从纯文本猜测
  候选。自定义 `post_process.prompt` 替换整理规则，保留任务与结构化协议；CLI/setup
  仍返回正文字符串，但其内部响应也采用相同协议，且始终使用空个人快照。
- **代价与边界：** JSON 增加输出 token 和模型遵循要求。无效 JSON/空正文沿用原始 STT
  回退；个别无效元数据独立丢弃。API 错误正文和可能引用个人文字的 schema 错误细节不
  保留到后处理错误中，避免进入日志；保留安全的错误类别与状态。

### 2026-10-19 — 单写入者、成功投递反馈及退出生命周期

- **实际变更：** 新增 `dictation::LearningWorker`，后台独占原有 `Memory`，串行处理
  snapshot、prepare 与单次确认。复用 `Memory::prepare` / `confirm` 和原子保存；没有
  复制对齐或替换算法，没有新增依赖、配置开关或存储 schema。
- **桌面接线：** `ListenerConfig` 携带记忆和语言；`ui::listener` 仅在 LLM 与记忆均启用
  时创建所有者；真实 finalization adapter 保留后处理的 `Confirmation`，由原有预览
  成功投递后提交最终编辑。取消丢弃反馈，下一快照不能越过已接受的确认。
- **退出与恢复：** 关闭时关闭待确认通道、排空已接受反馈并 join，未确认窗口或网络
  请求不阻塞记忆退出；损坏/未知版本文件使用临时内存，不覆盖源文件。HTTP 仅校验
  实际使用的记忆配置，不因未启用的 Realtime URL 无效而启动失败。
- **与原计划的细化：** 所有者作为独立的 HTTP 记忆模块，避免混入 Realtime 连接控制；
  保守/预热会话收为 LLM 内部实现并共享不可变请求状态。桌面现状仍在 STT 收敛后仅传入
  一次完整文本，未借此修改预热触发时机，文档已修正此前录音期间预热的误导描述。
- **隔离：** Realtime 与 HTTP 在监听器中互斥，切换后读取同一 schema-2 文件。关闭记忆
  不读写个人文件；关闭后处理、CLI/setup、raw capture 不创建记忆写入者。实际用户配置
  和运行中的应用未切换，没有部署替换。

### 2026-10-19 — 对照验证、模型限制与交付状态

以下链接是本次本地证据，保存在忽略目录，未纳入版本控制。

- **基线与回归：** 保留[实施前 diff](../../.amp/in/http-cleanup-baseline/workspace-before.patch)、
  [基线测试](../../.amp/in/artifacts/http-cleanup-baseline.log)及新桌面入口缺失时的
  [测试先行失败](../../.amp/in/artifacts/http-cleanup-red.log)。新增的合同测试覆盖两种
  请求模式、确认→持久化→重启→本地修正→撤销、Realtime/HTTP 互用、取消、缺少候选、
  不同语境、协议错误回退、禁用记忆、自定义规则与 CLI/setup 空快照。既有预览测试
  继续覆盖失败投递重试后仅学习最终成功版本。
- **消融（已执行）：** 固定服务始终返回同一个误词“维伯耳语”。无确认反馈时不修正；
  仅上传已确认快照、跳过本地准备时仍为误词；完整路径得到 `ViberWhisper`。缺少候选
  或切换语境时完整路径保留原文，对照无边界 `replace` 会误替换。该结果支持保留确认
  所有者、本地修正与候选/语境边界。[实验代码](../../.amp/in/http-cleanup-ablation.rs)与
  [运行日志](../../.amp/in/artifacts/http-cleanup-ablation.log)保留在 scratch；临时测试
  模块已从产品源码移除。复现时临时在 `src/postprocess/llm.rs` 末尾引入
  `#[cfg(test)] #[path = "../../.amp/in/http-cleanup-ablation.rs"] mod ablation;`，执行
  `cargo test --locked --lib http_cleanup_ablation -- --nocapture`，然后移除该引入。
- **真实模型（已执行）：** 对可用的 `giangndm/qwen2.5-omni-7b-mlx-4bit` chat completions
  服务发送六条合成文本与空个人快照，对照实施前和共享规则提示词。按忽略句末句号的
  文本对比，旧规则 3/6、新规则 5/6；新规则六次均返回所需 JSON。明确改口、独立否定、
  原词、未完成片段及转写指令作为数据的样例符合预期。隐式改口“拿四本，五本吧，放在
  门口。”仍输出“拿4本，5本吧，放在门口。”，没有合并为最终数量。保留该模型限制，
  未扩大本任务去调整共享 Realtime 规则或服务端模型；这些是小样例对照，不是准确率
  基准或真实音频端到端验证。[完整结果](../../.amp/in/artifacts/http-cleanup-model-comparison.json)、
  [输出摘要](../../.amp/in/artifacts/http-cleanup-model-comparison.log)、
  [复现脚本](../../.amp/in/http-cleanup-model-check.py)。
- **本地检查：** `cargo fmt --check`、`cargo test --locked`（315 passed、2 ignored）、
  `cargo clippy --locked --all-targets --all-features -- -D warnings` 与
  `cargo build --locked --all-features` 均通过。
  [测试日志](../../.amp/in/artifacts/http-cleanup-full-tests.log)、
  [Clippy](../../.amp/in/artifacts/http-cleanup-clippy.log)、
  [构建](../../.amp/in/artifacts/http-cleanup-build.log)。两项 ignored 分别要求私有语音
  数据集和显式配置的 Realtime 服务；vendored rdev 仍有原有 FFI/static-mut 警告。
- **跨平台限制：** macOS 上尝试
  `cargo check --locked --target x86_64-pc-windows-gnu --all-targets --all-features`，
  被已有 `ort-sys` 依赖阻止：其不提供该 GNU 目标的 ONNX Runtime 预编译包，未完成
  Windows 检查；未改依赖或伪造链接库。需要 Windows/MSVC CI 或实机补验。
  [失败日志](../../.amp/in/artifacts/http-cleanup-windows-check.log)。
- **交付状态：** 仅本地实现与验证，保留既有其他修改；尚未提交、推送、创建 PR、合并
  或部署。后续发布仍须走仓库的 master PR 流程。

### 2026-10-19 — 真实客户端复验与首次模型结论更正

- **触发与发现：** 继续检查首次对照中的隐式改口失败。临时 Rust 测试直接调用
  `PostProcessor::new_desktop` / `process_for_delivery`，关闭个人记忆且不读取用户配置；
  在没有修改产品提示词的情况下，保守、预热两种模式均将“拿四本，五本吧，放在门口。”
  整理为“拿5本，放在门口。”。因此首次脚本的 5/6 不能直接代表真实客户端结果。
  [两种模式日志](../../.amp/in/artifacts/http-cleanup-live-followup-before.log)。
- **差异消融（已执行）：** 固定原提示词、模型、温度和生成参数，只改变输入 JSON 的
  字段顺序及空白。该模型在该样例上，`text` 在前时两种空白格式均漏合并；`contexts`
  在前时两种格式均正确合并。Rust 实际序列化使用 `contexts` 在前的紧凑 JSON，首次
  Python 脚本使用 `text` 在前的 JSON。这个输入是模型阅读的字符串，JSON 语义相同不
  代表推理输入相同。[四组对照](../../.amp/in/artifacts/http-cleanup-input-format-check.json)、
  [日志](../../.amp/in/artifacts/http-cleanup-input-format-check.log)、
  [脚本](../../.amp/in/http-cleanup-input-format-check.py)。
- **决定（已实施）：** 修正 scratch 对照脚本，使新路径请求使用与 Rust 一致的紧凑、
  排序键 JSON，并移除客户端没有设置的 `max_tokens`；产品代码和共享提示词保持不变。
  此前尝试的任务优先顺序调整仅在 scratch 中比较过，未引入产品，避免为脚本输入差异
  增加不必要的提示词改动。原始实验产物保留，不覆盖。
- **更正后的模型对照：** 使用同六条合成文本、相同模型与空个人快照重跑，旧提示词
  3/6，新路径 6/6，六次结构化 JSON 均有效。该结果替代上一条关于实际普通客户端仍
  漏合并此例的推断；模型对输入顺序敏感的限制仍成立，不能外推到任意模型或真实语音
  准确率。[新结果](../../.amp/in/artifacts/http-cleanup-model-client-comparison.json)、
  [摘要](../../.amp/in/artifacts/http-cleanup-model-client-comparison.log)。
- **实际入口反例：** 除上述两次隐式改口请求外，Rust 实际入口还通过八条请求，覆盖
  原有明确改口、独立否定、原词、未完成片段、指令作为数据，以及并列数量、追加数量、
  不同主体不误合并。并列数量控制直接使用 `4` / `5`，单独检查内容保留；另一份
  text-first scratch 扩展对照中的中文并列数字未转成阿拉伯数字，不将其宣称为格式化
  全面通过。[实际入口日志](../../.amp/in/artifacts/http-cleanup-live-followup-controls.log)、
  [扩展对照](../../.amp/in/artifacts/http-cleanup-prompt-followup.log)。临时 Rust 测试代码
  [保存在 scratch](../../.amp/in/http-cleanup-live-followup.rs)，产品模块中的引入已移除；
  复现时临时在 `src/postprocess/llm.rs` 末尾引入
  `#[cfg(test)] #[path = "../../.amp/in/http-cleanup-live-followup.rs"] mod live_followup;`，
  运行 `cargo test --locked --lib http_cleanup_live_ -- --nocapture --test-threads=1`，
  然后移除引入。它验证真实 HTTP 后处理，不包含麦克风采集和原生目标投递。
- **回归及跨平台：** 移除临时引入后，格式检查、315 项通过 / 2 项跳过的全量测试、
  全 targets/features Clippy 再次通过。
  [全量测试](../../.amp/in/artifacts/http-cleanup-followup-tests.log)、
  [Clippy](../../.amp/in/artifacts/http-cleanup-followup-clippy.log)。补试 Windows/MSVC 的
  `cargo check --locked --target x86_64-pc-windows-msvc --all-targets --all-features`，
  仍被本机缺少 Windows C 标准库头文件阻止，`tinyfiledialogs` / `zstd-sys` 无法编译；
  未完成 Windows 验证，需要 Windows CI 或实机。
  [MSVC 阻塞日志](../../.amp/in/artifacts/http-cleanup-windows-msvc-check.log)。
- **交付状态：** 本轮补充验证和更正记录，未进一步改动产品实现，未切换运行配置，
  未提交、推送、创建 PR、合并或部署。

### 2026-10-19 — 在最新 master 上隔离并复验

- **触发与范围：** 原工作区含多项其他改动，不能把整个工作区相对 master 的差异发布。
  从实施前快照重建本任务的差异，并在 `http-cleanup-publication` jj 工作区以
  [当前 master](https://github.com/b1indsight/viberwhisper/commit/cf1898554d87e1a422f336d62f14a62823f15f94)
  为基线三方合并。本次仍只修改 16 个任务文件；保留上游的稳定前缀协议、内存预算、
  流式预览撤回和取消恢复改动。未更改依赖、锁文件或共享默认提示词资产，也未移动原工作区。
- **验证：** 隔离版本 `cargo test --locked` 为 324 passed、2 ignored；格式检查、
  全 targets/features Clippy（`-D warnings`）和全 features 构建均通过。
  [测试](../../.amp/in/artifacts/http-cleanup-isolated-tests.log)、
  [Clippy](../../.amp/in/artifacts/http-cleanup-isolated-clippy.log)、
  [构建](../../.amp/in/artifacts/http-cleanup-isolated-build.log)。
- **真实入口与消融：** 使用隔离版本及 master 的默认提示词重新执行现有 Rust scratch
  验证。两种模式的隐式改口、八条保真/否定/并列数量等控制请求全部通过；固定响应消融
  再次确认只有成功反馈与受候选/语境约束的本地修正组合才能修复反复误词。三项 scratch
  测试均通过，随后移除临时模块引入；未使用用户个人记忆、录音或目标应用。
  [日志](../../.amp/in/artifacts/http-cleanup-isolated-live-and-ablation.log)。这些仍是有限
  合成样例，不代表真实音频准确率。Windows 验证仍受上一条记录的本机环境限制。
- **交付边界：** 独立审查待执行；尚未创建任务 bookmark 或 PR，未提交、推送、合并或部署。
  上述证据保留在原工作区的忽略目录，未纳入版本控制。

### 2026-10-19 — 修复整段 HTTP 文本误用 Realtime 单段预算

- **触发：** 独立审查报告一条 medium：HTTP 整段后处理复用 `DictationResult::parse`
  后，正文超过 16 KiB 会被拒绝并回退原始 STT；原解析器另有 64 KiB JSON 上限。
  [首次审查](../../.amp/in/artifacts/http-cleanup-isolated-review.json)。用户要求修复该项。
- **决定与实现：** 在 `dictation/result.rs` 分开整段文本与 Realtime 单段入口，公共的
  JSON、正文安全性、语境及候选定位校验仍只有一份。HTTP 改用 `parse_transcript`，
  保留此前普通后处理没有固定整段长度上限的行为；Realtime 的 `parse` 继续施加两项
  单段字节上限。没有直接提高或删除 Realtime 上限，也没有增加配置开关或复制校验器。
- **复现与对照：** 新回归测试先在未修复实现上以 `InvalidResult` 失败，修复后通过。
  同一测试对 6000 / 24000 个汉字的长正文，覆盖 CLI/桌面、保守/预热、记忆开/关，
  确认完整整理结果与尾部保留。独立边界控制确认 Realtime 允许 16384 字节正文及
  65536 字节 JSON，任一超过一字节仍拒绝。这组前后对照支持将大小预算留在单段入口，
  不引入额外机制；无需为这个边界拆分另加复杂消融。
  [修复前](../../.amp/in/artifacts/http-cleanup-length-red.log)、
  [修复后](../../.amp/in/artifacts/http-cleanup-length-green.log)、
  [Realtime 控制](../../.amp/in/artifacts/http-cleanup-length-realtime-control.log)。
- **验证：** `cargo fmt --check`、全量测试（326 passed、2 ignored）、全 targets/features
  Clippy（`-D warnings`）和全 features 构建通过。两项跳过与 vendored rdev 原有警告
  不变；Windows 仍受前述本机环境限制。
  [测试](../../.amp/in/artifacts/http-cleanup-length-tests.log)、
  [Clippy](../../.amp/in/artifacts/http-cleanup-length-clippy.log)、
  [构建](../../.amp/in/artifacts/http-cleanup-length-build.log)。
- **范围与交付：** 本轮仅改解析边界、调用点、对应回归测试及说明，共五个文件；原工作区
  保持原样。后续仅进行一次获授权的复审；未提交、推送、创建 PR、合并或部署。
