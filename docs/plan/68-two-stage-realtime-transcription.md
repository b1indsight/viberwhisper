# 68 - Realtime 纯转写与文字后处理 — Feature record

**Stage:** Draft implementation；初始两阶段方案、修订 002/004 及 HTTP 报告边界修正 005 已完成隔离实现与本地检查。连续生产状态实测仍复现文字后处理复制示例，质量验收未通过，PR 须保持草稿。真实桌面交付和 Windows 验收另行执行；发布审查与 CI 结果随 PR 记录。
**PR:** [草稿 #177](https://github.com/b1indsight/viberwhisper/pull/177)，`feat/production-recognition-parity` → `master`。合并、部署、个人配置激活和实际应用切换不在授权中。

**当前设计：** 已批准的[修订 002](#002--2026-11-12--prompt-lab-复用完整生产识别路径)取代下文初始方案中“prompt-lab 保留旧协议”的安排；[修订 004](#004--2026-11-12--普通评估复用生产初态时钟会话和确认学习)进一步取代普通评估的空初态、逐样本重建与丢弃反馈。初始方案及既有时间线保留作为历史。实际实施与验证见 [003](#003--2026-11-12--共享生产识别与隔离评估的本地实现)及 004。

## 1. Initial plan

### Summary and motivation

用户报告短音频严重复制提示词中的示例，要求优先修复，并指定使用 `.156` 服务，采用
“转写 prompt，然后调用文字推理后处理”。新建会话的同音频基线再次返回完整示例句；
在同一服务上隔离纯转写后，再调用现有文字后处理，本轮未再复制该示例。

诊断已完成，但它是独立协议调用，不是应用修复或桌面端到端验证。证据和局限记录在
[短音频污染诊断](https://ampcode.com/artifacts/9bd26c46-f9dc-452b-a25f-9e0eec6867f0)及
[原始结果](../../.amp/in/artifacts/short-audio-two-stage-20261112/summary.json)。

### Goals, scope, and success criteria

- 在桌面和 `convert` 中接通 Realtime 纯转写 → 现有文字后处理，不更换提供商或模型。
- 音频推理不接收整理示例、JSON 模板、个人词库、历史、动态语境或前段转写；后处理
  接收真实 ASR 结果，再负责整理、语境、候选和已确认纠错。
- 任意段号的短有声片段都遵循同一规则；保留停顿分段、静音过滤与有声尾段，不设
  “仅第一段”或任意时长分流，也不靠丢弃音频、输出黑名单或 JSON 修补通过验收。
- 保留临时预览、权威最终校验、取消、部分失败和成功投递后单次确认的既有合同。
- 不承诺修复音频本身含糊、专名或同音字错误；不包含服务优化、UI 外观修改、部署或
  跨语言准确率保证。本轮真实模型证据只覆盖中文和空个人快照。

### Proposed behavior

复用已有开关，不新增用户配置模式：

| 配置或入口 | 识别及整理行为 |
| --- | --- |
| `dictation.enabled = true` 且 `post_process.enabled = true` | 新的纯 ASR → 文字后处理 |
| Realtime 开启、后处理关闭 | 保持现有一体式听写；已知短音频污染不会因此被自动修复 |
| HTTP 识别 | 保持现有可选后处理行为 |
| raw capture | 保持 HTTP 原始转写，不执行后处理或个人学习 |
| prompt-lab 的现有结构化 Realtime 评估 | 保持一体式协议与可重复的冻结记忆 |

新链路的第一步使用本轮验证的最小指令，要求纯文本，不要求 JSON。第二步复用
`PromptComponents::postprocess_prompt`、`PostProcessor` 的结构化 HTTP 请求与严格解析。
两个端点分别为 `.156:8080/v1/realtime` 和 `.156:8080/v1/chat/completions`；只在明确配置
后使用，不根据一个端点猜测或覆盖另一个端点。

**自定义提示词兼容。** 现有 `dictation.prompt` / `prompt_components` 属于一体式听写。
选择新链路且这些字段非空时，启动给出可操作的配置冲突错误，不把旧整理指令送入
纯 ASR，也不静默忽略它们。启用者先保存旧配置，再显式清空这些覆盖；新链路的
整理自定义仍使用 `post_process.prompt`。默认配置不变，一体式自定义继续可用。

**失败和空结果。** ASR 取消及输入/传输失败沿用现有取消、部分文字和不完整警告，不能
因为接入第二步丢掉成功片段。完整 ASR 为空时不请求后处理。文字后处理超时、失败、
无效 JSON 或空正文时回退原始 ASR，不附带伪造语境、候选或学习反馈。

### Technical design and affected modules

只新增一个内部任务选择概念 `RealtimeMode`，两个无字段变体为 `Integrated` 与
`Transcription`；不序列化。桌面和 `convert` 按上述矩阵选择，现有评估入口明确使用
`Integrated`。这是现有调用者确实需要的两个协议，不是新的用户调参面。

- `src/core/recognition.rs`、`src/core/listener.rs`：统一装配实际启用的后处理，并携带
  `RealtimeMode`；不再无条件禁用 Realtime 的文字后处理。只校验实际选中的配置。
- `src/dictation/transport.rs`：在 `Transcription` 下，session 与 response 都只使用最小
  ASR 指令；不创建文本前缀项，响应的显式输入只引用本段音频。增量文字直接作为
  临时正文，最终纯文本单独校验；不把“JSON 解析失败”当作接受纯文本的降级规则。
  继续校验关联 ID、内容索引、重复事件、成功状态、增量/最终一致性与现有大小上限，
  复用取消、排空和删除音频项的生命周期。`Integrated` 保留严格 JSON 协议。
- `src/dictation/result.rs`、`text_stream.rs`：在已有正文/流式边界内支持显式选定的纯文本
  合同，保留 NUL 和字节预算校验；没有语境或候选的第一步结果不能获得学习资格。
  不从第一步正文猜测或提取结构化元数据。
- `src/dictation/engine.rs`：`Transcription` 不加载个人 `Memory`，不取得或发送快照，
  不执行逐段学习、准备本地纠错、确认或持久化。保留录音和分段所有权，合并成功
  片段，返回 `Transcript { text, confirmation: None }`。
- `src/postprocess.rs`、`src/postprocess/llm.rs`：复用已有桌面 `LearningWorker`，由其
  独占第二步的个人记忆与确认；不新增第二份存储、替换算法或额外元数据推理。
  仅第一阶段不读写记忆文件，第二阶段在记忆启用时仍按原合同读写同一文件。
- `src/ui/listener/event_loop.rs`：已有 `process_for_delivery`、失败回退和优先使用后处理
  确认句柄的行为应直接复用；无必要不修改预览与交付状态机。
- `src/core.rs`、`src/dictation/offline.rs`：`convert` 不再在新链路中提前跳过后处理；
  ASR 完成后复用 `process_text` 与原文回退，仍不加载或学习个人记忆。
- `src/core/prompt_lab.rs`、评估入口：保持结构化协议，元数据快照与实际任务一致。
- README、相关架构说明和 changelog：说明启用矩阵、提示词兼容、隐私与延迟。

构造入口的计划签名如下，`Result` 均沿用模块现有错误类型；旧参数保留其含义：

```rust
SessionOrchestrator::new_realtime(settings: DictationConfig, key: Option<SecretValue>, memory_path: Option<PathBuf>, language: Option<String>, mode: RealtimeMode) -> Result<Self>
DictationWorker::new(settings: DictationConfig, key: Option<SecretValue>, memory_path: Option<PathBuf>, language: Option<String>, mode: RealtimeMode) -> Result<Self>
OfflineDictation::new(settings: DictationConfig, key: Option<SecretValue>, language: Option<String>, mode: RealtimeMode) -> Result<Self>
Connection::connect(settings: &DictationConfig, key: Option<&str>, mode: RealtimeMode) -> Result<Self>
```

mode 由构造时固定，不能在复用的连接中悄悄切换协议。段内前缀/动态输入和结果表示的
具体内部签名在实现时按现有所有权结构收敛，不预设新的通用策略层、两套状态机或
大量仅为传参而存在的包装类型；若需要新增核心类型，在同一记录中补充其字段与理由。

### Alternatives and rationale

- 只改完整提示词或删除示例：此前仍复制占位文本，且无法安全接受纯 ASR 输出。
- 只开启现有后处理：桌面装配会禁用它，CLI 也提前返回，第一步仍有污染源。
- 只修首段或按时长分流：停顿能在任意位置切出短片段，分流不能保证上下文隔离。
- 全面删除一体式协议：会扩大到现有结构化评估、记忆实验和自定义提示词迁移；本次
  保留这些真实调用者，用内部模式限定协议边界。
- 增加外部服务、二次听音或第三次元数据推理：不符合指定服务与两阶段范围。

### Costs, risks, and mitigations

文字阶段是本轮主要等待时间，耗时中位数约 8.5 秒；第一步约 0.65 秒。串行两步不能
承诺即时完成；保持临时文字可见、现有超时与原文回退，另行记录实际应用耗时。

纯 ASR 不参考词库，陌生专名和同音字可能变差；后处理不能重新听音频。真实音频测试
已出现这些错词保留，以及长句未补标点，必须公开记录，不能用“解析成功”代替质量验收。

升级前已同时开启两个开关的用户会改变链路；存在一体式自定义时会收到明确配置错误。
文档说明这一兼容边界，不静默删除或迁移个人提示词。个人记忆只在第二步启用时发送给
配置的文字服务；本机目标是 `.156`，程序本身不硬编码该私有地址。

### Implementation order

1. 明确内部模式与装配合同，先覆盖选择矩阵和自定义提示词冲突的回归。
2. 在现有传输中支持 audio-only、纯文本增量/最终结果及无前缀清理，验证错误边界。
3. 隔离第一阶段记忆，保留分段/部分结果；接入已有桌面后处理与唯一确认所有者。
4. 接通 `convert`，保持 prompt-lab、raw capture 和旧协议隔离；补充使用说明。
5. 跑确定性回归与真实服务同音频对照，再按同一 feature-record/bookmark/PR 交付。

个人配置激活不在本轮实现授权内。先提供脱敏差异，保留旧值；获得明确授权后才设置
两个 `.156` 端点、同一模型及启用后处理。回滚时恢复配置并关闭后处理即可回到原链路，
但原链路仍有已知污染风险。没有存储 schema 迁移；不自动替换或重启已安装应用。

### Validation and ablation strategy

以下为实施后的验收计划，不能当作已经执行：

- 用本地 WS/HTTP 桩通过公开录音/后处理入口验证首段及非首段短音频：音频请求只有
  转写指令和本段音频，前一段文字、词库与历史均不进入第一步；第二步收到完整真实
  ASR 文本。用不同短词和较长句作非对称对照，避免固定结果也能通过。
- 验证纯文本跨 delta、最终完成、NUL/大小限制及错误关联/内容/状态；旧协议仍拒绝
  非法 JSON。第一步不能用宽松解析掩盖服务错误。
- 验证空 ASR、静音、有声短尾、取消、部分失败、清理失败、后处理失败或空正文、
  CLI 输出；失败不伪造成功，不丢弃已成功的录音片段。
- 验证第二步确认 → 成功投递 → 下次同语境纠错；取消或失败不学习、重试只确认一次。
  第一阶段无确认句柄、不打开个人文件；第二阶段仍使用现有单一记忆所有者。
- 同一 `.156` 模型、相同 WAV 和新会话下比较一体式、纯 ASR、纯 ASR 加后处理；覆盖
  含糊极短音节、较清晰短问句、专名、长句、非首段短片段及改口/否定。记录示例复制、
  严格解析、文字保真与耗时；示例匹配只用于统计，不能成为产品输出过滤器。
- 对数据集标签有疑问的音频保留原音与标签差异，不当作已核验准确率；新增清晰且
  经核对的录音验证补标点、改口与短句质量。个人快照与桌面连续会话需另外实测，
  不能把目前空快照的独立调用当作证明。
- 执行 `cargo fmt --check`、针对性测试、`cargo test --locked`、
  `cargo clippy --locked --all-targets --all-features -- -D warnings`；原生桌面验证关注
  临时结果、确认和投递语义，不改变外观。不可运行的平台明确报告限制。
- 消融在实现后、代码审查前执行；当前诊断只为方案可行性提供证据。

### Open questions and deferred work

**等待批准：**上述两开关选择矩阵、旧自定义提示词的显式冲突处理、第一步记忆隔离及
第二步唯一学习所有权。当前只请求本地产品/测试实现，不请求提交、推送或配置激活。

**实现时收敛：**段内输入/输出表示、保持现有清理保证的最小改法，以及桌面取消跨越
第二步时的实际行为。发现必须改变交付合同或新增核心状态时，先补充方案而非扩大范围。

**范围外：**服务推理优化、更换模型、无证据纠正 ASR、额外语言策略、词库 UI、
服务部署、输出黑名单和任意 JSON 修复。

本方案后续获批实施时，将取代 [计划 61](61-http-postprocess-correction.md)中“Realtime
不变、仅 HTTP 使用文字后处理”的排除范围；该计划的确认、失败回退与学习边界继续有效。

## 2. PR change timeline

### [001] — 2026-11-12 — 要求 prompt-lab 与正式识别共用执行路径

- **需求更新：** 用户要求先核对 prompt-lab 是否走完整识别路径，并明确希望除音频
  输入不来自现有捕获外，其余识别处理使用相同路径。此要求来自
  [本线程](https://ampcode.com/threads/T-01a1cbe1-2afc-7223-aefb-f732ddfbddcf)。
- **查看结果：** 当前评估复用底层识别器与传输，但没有完整复用桌面会话：默认评估
  在 [`transcribe_sample`](../../src/prompt_lab/regression.rs) 中逐块调用 `Transcriber`；
  Realtime 的 [`OfflineDictation`](../../src/dictation/offline.rs) 和显式场景入口均使用
  `Segmentation::Fixed`，桌面 worker 使用 `PauseAware`。评估不调用桌面后处理及最终
  投递准备；普通评估关闭个人记忆，显式场景只使用独立种子，未走桌面确认闭环。
- **设计背景：** [现有架构说明](../architecture/prompt-lab.md)明确将工具限定为原始
  STT prompt 评估，排除 `PostProcessor`、正常历史和文本注入。因此差异属于既有边界，
  不能声称只要使用相同模型、prompt 或底层 transport 就是完整路径一致。
- **对初始方案的影响：** 初始计划中“prompt-lab 固定保留一体式协议”的安排不符合
  这个新要求，后续方案须修订，不能让桌面进入两阶段后评估仍默默运行旧链路。
  本次只记录需求与事实；尚未生成或批准新的共享接口设计。
- **证据边界：** 此前 24 次短音频测试为服务级 HTTP `input_audio` 直接调用，既不是
  prompt-lab 执行，也不是桌面端到端验证；不得拿它证明两条应用路径一致。
- **待明确的交付边界：** 识别处理的一致性需覆盖分段、请求、解析、合并、后处理、
  失败回退和本地纠错；预览人工确认、持久学习及真实文本注入是否纳入回放，仍需在
  修订方案中明确。输入方式变化不构成自动确认数据集参考答案、改写个人记忆、
  写入真实历史或向当前应用注入测试文字的授权。
- **实施状态：** 仅检查源码并补充本记录；未修改产品代码、测试、个人配置或服务。

### [002] — 2026-11-12 — prompt-lab 复用完整生产识别路径

**状态：提议，等待批准；尚未实现。** 用户要求先修订方案。本节承接 [001]，将文件评估改为使用与桌面相同的识别装配、音频分段、会话编排、结果校验、后处理、失败回退和本地纠错。只替换音频来源，不再另造评估识别器。[需求来源](https://ampcode.com/threads/T-01a1cbe1-2afc-7223-aefb-f732ddfbddcf)

#### 取代的安排与保留的行为

- 取代初始方案的 prompt-lab 排除项、评估固定选择 `Integrated` 的构造约定，以及实施步骤 4 中让评估继续走旧路径的安排。默认评估和显式场景都跟随生产配置选择协议与后处理，不因入口是 lab 而改变模式。
- 继续使用初始方案的两开关矩阵。保留 `Integrated` 的理由是已有配置与自定义提示词兼容，不再以旧评估器需要它作为理由；后处理关闭时，桌面和 lab 都仍有原污染风险。启用两阶段及清理旧一体式自定义的冲突处理继续有效。
- 两阶段第一步仍为最小纯 ASR，第二步为现有文字后处理；不切换服务、模型或传输协议。`.156` 的 HTTP 音频诊断不能替代 Realtime 产品验证。
- `convert` 仍在初始方案范围内，复用本次文件输入和公共收尾，避免形成第三套识别流程；保留它不使用个人学习的公开行为，并将空初态、丢弃未使用确认句柄作为明确宿主条件，不另写跳过纠错算法的收尾。它不作为“个人记忆条件相同”的等价性证据。

#### 共用边界：音频入队到可预览的最终结果

桌面与 lab 都从同一个 `ConfigDocument` 解析后端、模式、语言、端点、模型、实际提示词和后处理设置。共享构造只校验选中阶段，不允许 lab 自动关闭后处理或为了跑通测试改成另一后端。默认无 prompt 覆盖时，两个入口应得到相同的阶段配置；测试初始词表和记忆的显式隔离见下节，不冒充个人环境相同。

音频入口复用 [`AudioRecorder` 的设备无关缓冲、就绪封包和 stop 收尾](../../src/audio/recorder.rs)，不复制 `Segmenter` 或自己按 30 秒切“识别段”。WAV 解码后的 PCM 进入与麦克风相同的 `push_mono`、`take_ready_chunk` 和尾段处理：Realtime 输出约 200 ms 帧，由生产 worker 在重采样后按停顿分段；HTTP 在录音侧按生产停顿策略封包。样本全程属于一个生产 session，文件读取块不新建 Recording。已捕获 PCM 不再应用一次录音 gain；保留原采样率，沿用已有解码、通道转换和后端重采样，不静默裁剪、补静音或丢弃短尾。

真实服务评估按采样偏移与单调时钟以 1 倍速回放，持续处理就绪音频，最后才关闭输入。不能快速把所有块塞入现有容量为 2 的 HTTP 队列，也不能遇满队列时在 lab 私自等待到成功；相同负载下的生产溢出、失败和超时都要保留。第一版不增加倍速开关。确定性测试可以控制调度，但不得把加速测试耗时报告为真实服务延迟。[生产队列与期限](../../src/core/orchestrator.rs)

收尾从 [`finish_transcription`](../../src/ui/listener/event_loop.rs) 下移到现有 `core::recognition`。桌面也必须调用下移后的实现，UI 内不保留另一份处理分支。继续遵循：完整非空识别才后处理；后处理错误或空结果回退原文；输入失败、部分失败和收敛超时保留成功文字与警告，并沿用当前跳过后处理的行为；空结果、路由错误和取消不伪装成正常成功；后处理确认句柄优先于识别句柄。

共享识别结果到达“用户编辑前、可进入预览”的边界后，宿主分别处理：桌面进入既有预览/焦点恢复/历史/投递；lab 保存报告。lab 不向当前应用注入测试文本，不写正常历史，不自动弹原生预览。此边界不改变识别算法；报告也不能声称已验证原生编辑、焦点恢复或真实投递。这些交互仍单独执行桌面验收。

`prompt-lab record` 保持数据集收集入口，继续保存原音与待人工核对的参考材料；它的 raw-capture 结果不作为完整识别评估或本次修复证据。已有数据集无需重新录制，评估必须重新执行音频，不使用捕获时的旧转写代替结果。

#### 共享接口与所有权草案

在 `src/core/recognition.rs` 增加下列内部类型，供桌面、文件评估和 `convert` 共同调用。名称与签名是拟议接口，不代表已实现；`Result` 沿用 `anyhow::Result`。不新增通用 pipeline trait、服务适配层或评估专用会话状态机。

```rust
pub(crate) struct RecognitionPipeline {
    pub orchestrator: Arc<SessionOrchestrator>, // 现有会话、进度、取消及后台识别所有者
    pub post_processor: Arc<PostProcessor>,   // Disabled 仍使用现有变体；两阶段唯一学习所有者
    pub streaming: bool,                     // 由生产后端选择推导，供两个输入源采用同样封包策略
}

pub(crate) enum CleanupStatus {
    Skipped,                                 // 输入不完整、为空或取消等情况，不请求后处理
    Disabled,
    Applied,
    EmptyFallback,
    FailedFallback(PostProcessError),
}

pub(crate) struct RecognitionOutcome {
    pub raw_text: String,                    // 后处理前的识别正文；一体式为该协议的识别结果
    pub transcript: Option<Transcript>,      // 可预览文字与单次确认句柄；无可展示结果时为 None
    pub warning: Option<String>,             // 保留现有桌面警告，不把部分结果标成完整成功
    pub recognition_error: Option<SessionError>,
    pub cleanup: CleanupStatus,
    pub recognition_wait: Duration,          // 停止采集到 ASR 收尾完成，不冒称全部模型推理耗时
    pub cleanup_time: Duration,              // 文字阶段实际耗时；跳过时为零
}

impl RecognitionPipeline {
    pub(crate) fn from_config(
        document: &ConfigDocument,
        memory_path: Option<PathBuf>,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
        asr_prompt_override: Option<String>,  // None 用配置；Some(空串) 是显式 --no-prompt
    ) -> Result<Self>;

    pub(crate) fn finish(
        &self,
        session_id: SessionId,
        stopped_at: Instant,
    ) -> RecognitionOutcome;

    pub(crate) fn recognize_wav(
        &self,
        path: &Path,
        session_id: SessionId,
        cancelled: &CancellationToken,
    ) -> Result<RecognitionOutcome>;
}
```

`from_config` 复用 `RecognitionConfig` 和 `PostProcessConfig`，统一两阶段选择及实际后处理装配；不经含热键/设备要求的 `ListenerConfig` 构造离线评估。现有后端构造改为 `RecognitionConfig::build(self, memory_path: Option<PathBuf>, now: Arc<dyn Fn() -> u64 + Send + Sync>) -> Result<SessionOrchestrator>`，不在内部发现个人路径。桌面宿主在调用外层构造前解析原个人路径，并传 `asr_prompt_override = None`；lab 只传隔离路径及显式候选。原 raw capture 继续显式传无个人状态的依赖。HTTP 的显式空覆盖沿用省略 prompt 字段的现有语义，Realtime 则发送空 instructions。

`now` 是记忆保留期、快照、学习反馈使用的 Unix 秒时钟，沿用现有算法并传给 Realtime worker 与 `LearningWorker`；不替换网络超时、播放节拍或收敛期限使用的真实单调时钟。两个记忆所有者的构造只增加该依赖，不各建一套测试实现。普通运行使用系统时间；场景使用显式起点、步骤推进和录音内时间推进，身份记录时间策略。音频阶段不持有第二份记忆，两阶段仍由文字 worker 独占学习。

`finish` 调用现有 `finish_for_delivery` 并应用共享收尾策略。它保留可观察的错误、回退与等待时间，让 lab 无需从日志或最终文字反推阶段是否执行。桌面把其 `transcript` 和 `warning` 映射到已有 `PreviewTranscription`，保持现有交付合同。

`recognize_wav` 只负责文件输入适配：开始同一生产 session、回放、提交停止尾段、调用 `finish`；解码或输入中途失败经 `fail_session` 保留已完成文字，取消经原 `abort_session` 清理。音频层新增内部函数 `replay_wav(path: &Path, session_id: SessionId, streaming: bool, cancelled: &CancellationToken, on_chunk: impl FnMut(WavChunk)) -> Result<()>`，放在现有音频模块内复用 `ActiveRecording::new`、缓冲及封包，不向 lab 暴露 cpal 类型。它在每个播放节拍排出所有就绪块，并用相同 stop 逻辑提交尾段。文件和 live callback 从 PCM 入队起共用生产代码；必要的内部移动保持原设备选择行为。

#### 记忆、纠错和显式确认使用隔离状态

- 删除 lab 通过 `memory.enabled = false` 和 `Memory::frozen` 绕过生产逻辑的做法。记忆开关跟随所评估配置，初始化、快照、`prepare` 本地纠错、`confirm`、保存及损坏文件处理使用同一套生产实现。[当前记忆与确认合同](../../src/dictation/memory.rs)、[现有文字学习所有者](../../src/dictation/learning.rs)
- 默认每个样本使用独立空状态，显式场景在步骤间复用同一隔离状态，场景之间重置连接与状态。测试初始长期词表、历史及纠错必须来自显式测试种子或脱敏快照；配置内的个人长期词表也不能隐式带入测试状态。lab 不读取默认个人 memory 文件，不改写个人配置。种子及快照作为明确输入写入运行身份，不能把空状态结果称作已覆盖真实个人状态。
- 默认评分后丢弃确认句柄，不确认或学习，并确保 worker 能开始下个样本。现有 `Confirmation` 的 Drop 已定义为取消学习。[单次反馈合同](../../src/dictation/feedback.rs)
- 场景步骤增加显式 `feedback`：`discard`、`delivery_failed` 或 `delivered { text: String }`；省略等同 `discard`。只有 `delivered` 调用同一 `Confirmation::confirm(text)`，并等待生产串行所有者处理完反馈后再取得下一步快照。文字须在场景中独立提供，绝不从 reference 自动取值。模拟失败/取消不学习，模拟成功只写测试状态；这属于学习合同验证，不代表真实平台投递成功。
- reference、专名答案和断言只给评分/检查器；识别入口只拿 WAV、阶段配置及显式状态，避免通过接口把参考答案或禁止串传入模型。评分固定使用编辑前 final，不能用模拟用户修改后的文字提高分数。

#### Prompt 与报告必须指明实际执行阶段

无覆盖时，lab 使用与桌面相同的完整解析配置。已有 `--prompt-file` / `--no-prompt` 明确仅覆盖当前所选音频阶段：HTTP STT、一体式听写，或两阶段的纯 ASR。覆盖只改变内存中的阶段指令，不改模式、不动保存配置，也不把纯 ASR 候选重新写入旧一体式 `dictation.prompt`。构造时先处理保存配置的旧自定义冲突；实验覆盖不靠扫描提示词内容猜测它属于哪类。报告标记候选覆盖，不能声称它与未覆盖的桌面请求完全相同。第二步继续使用配置解析后的实际后处理提示词；本次不额外增加后处理 prompt CLI 开关。

新完整路径报告使用 schema 3，增加独立的执行策略标识 `production-recognition-v1`；现有 WER、专名和语义评分算法不变。运行身份记录后端/内部模式、每阶段脱敏端点与模型、语言、温度、实际组合 prompt 及摘要、输入布局、封包/分段策略、记忆开关与种子摘要、时间/反馈策略及 1 倍速回放。候选 prompt 与显式种子可包含私人信息，沿用本地报告的隐私提示，不保存密钥、认证参数或个人路径。

每个样本同时记录 `raw_hypothesis`、编辑前 final（现有 `hypothesis`）、识别错误/部分结果、后处理状态与安全错误、警告、播放时间、停止后的 ASR 等待及文字阶段耗时。保留重算结果所需的实际段边界、阶段输入/输出及纠错/反馈 trace，使报告验证能核对原文、final 和场景状态之间的关系；不保存音频 base64 或重复的敏感网络日志。后处理回退保留原文参与评分，并明确标记回退，不能计成两阶段均成功；ASR 部分失败/超时保留 partial，但运行仍为 `incomplete`，不能作为成功基线或通过最终门槛。

报告读取、重算、agent review 身份和 `--compare-to` 同步验证新执行身份。比较要求相同数据、策略、模式、非候选阶段配置和回放条件；允许的候选 prompt 或显式种子变化必须逐项列出。现有可读取的旧报告继续作为历史读取并按旧合同校验，不补造后处理/确认 trace，不冒充新路径；旧 STT-only 与新完整路径不能直接比较，需要重跑基线。跨模式消融另存报告并列分析，不绕过比较约束。

`--realtime-cases` 随之使用 cases schema 3 和新完整路径策略，改调用同一 `recognize_wav`，不再执行旧 `RealtimeEvaluator::transcribe`。断言区分音频请求/原文、文字阶段语境/候选、最终纠错与反馈后的记忆；纯 ASR 无语境元数据，不伪造空 JSON 让旧断言通过。一体式模式的元数据仍按实际协议检查。旧 cases schema 2 在联网前给出迁移说明，不能悄悄调用旧评估器；旧报告 trace 按原策略验证所需的纯重放代码保留，旧独立执行路径在调用方迁完后移除。

#### 实施顺序、验收与取舍

1. 先将生产收尾和装配下移到 `core::recognition`，让桌面改用共享入口；保持识别行为、错误与确认所有权不变，运行既有测试。不借重构改变部分失败的处理策略。
2. 让构造显式接收状态路径及记忆时钟，复用现有录音 PCM 封包/尾段实现，增加按音频时间的文件输入。设备选择的并行改动保留，不为回放重写其逻辑。
3. 按初始方案完成纯 ASR 协议、严格纯文本解析、第一阶段记忆隔离和文字后处理，统一桌面、lab 与 `convert` 的实际模式选择。
4. 替换普通及场景评估执行器，加入显式反馈、阶段结果与新报告身份；迁移示例 cases、读取/比较/agent review，并更新 `docs/architecture/prompt-lab.md`、CLI 帮助和相关说明。`record` 的采集合同保留并清楚标识。
5. 完成下列确定性回归和 `.156` 同音频真实模型对照；再执行初始方案的 fmt、test、clippy 与桌面交互验收。只有实际产品链路通过后才报告修复，不用既有 Python 调用充当证明。

验收使用有区分力的输入，通过公共入口检查输出和实际请求：

- 同一段包含清晰短句、足够长停顿、另一短尾音的 PCM，分别进入 live 的设备无关入口和 WAV 回放入口；相同配置、状态及时钟下，段边界、送出的音频、请求布局、原文、final、警告和记忆结果一致。另用跨最大段长及边界两侧的样本，识破 Fixed 切分、每块重建 Recording、漏尾、重复音频与二次 gain；不同采样率与读块大小不改变连续音频合同。
- 本地 WS/HTTP 服务返回与原文不同的文字整理结果，final 必须是整理后的文字；再返回错误、空正文和非法结构，必须保留原文并分别报告回退。中途坏块、慢段和取消的对照必须保留生产相同的部分文字、警告及清理行为，不能只断言“请求成功”。
- 给首段和非首段短有声片段注入不同前文/词库哨兵，实际第一阶段请求都不得含它们；第二阶段必须收到真实合并原文。数据集参考与独立 feedback 使用不同文字，验证参考不进入任何请求、无反馈不增加学习记录、成功反馈学到的是显式编辑文字，投递失败/取消不学习，确认重试不重复提交。状态断言使用生产保存后的语义，不要求可能被正常裁剪/重写的 JSON 字节不变。
- 用多段音频和确定性服务调度验证回放持续按音频时间送入，不能由瞬时文件灌入造成队列溢出；另让真实处理速度落后，确认仍暴露生产有界队列失败而不隐藏。耗时按停止后等待与后处理分别核对，不拿录音播放时间当推理时间。
- 修改报告中的第二阶段身份、final 或反馈 trace，读取/复核必须拒绝不一致；旧 cases、新旧执行策略或不兼容比较在联网前拒绝。静音/空输入不请求文字后处理，也不能成为带虚构正文的成功样本。
- 实施后使用同一 `.156` 服务、模型和 WAV，对比旧一体式、生产基础 ASR 原文及两阶段 final，分别记录示例复制、文字保真、解析、分段和等待时间；保留有争议的标签，不把与标签相同直接当人工核验正确。真实个人状态和原生交付另做明确授权的测试。

只给 lab 补一次后处理调用，仍会保留固定分块、独立会话和不同失败策略；复制桌面流程则容易再次漂移。共享实际生产入口增加一次受控的装配/收尾整理，但三个调用者复用同一合同。按时回放会让数据集运行更久，独立种子也不能自动复现个人环境；这是本提案公开保留的成本，不能靠加速或隐藏状态差异换取好看的分数。

**本次实际交付：** 仅新增本节及文档顶部修订索引，未写产品代码或测试，未改个人配置、服务或已安装应用。草案参考本地 Claude Code Opus 5.5 的只读咨询，并按源码核对，保留原 `convert` 范围并收敛共享接口；咨询不等于执行验证。等待批准本地实现本修订与仍有效的初始两阶段方案；提交、推送、PR、配置激活与部署仍需各自授权。

### [003] — 2026-11-12 — 共享生产识别与隔离评估的本地实现

**决策状态：已接受；本地实现与识别路径验证已完成。** 按已批准的 [002]，桌面、WAV 转换与 lab 从 `core::recognition` 共用后端选择、实际后处理装配和编辑前收尾；UI 仅映射共享结果，不再保留另一份失败回退策略。Realtime 同时开启后处理时采用 `Transcription`，关闭后处理仍采用 `Integrated`。保存的一体式覆盖在候选覆盖之前校验，避免实验指令隐藏配置冲突。

**实际边界。** WAV 解码块只负责有界读取，PCM 进入一个生产 `ActiveRecording`，按样本偏移以 1 倍速送入并处理所有就绪帧及 stop 尾段；不重复应用 gain。删除旧 `OfflineDictation` 与 `RealtimeEvaluator` 网络执行路径，历史报告只保留纯 Replay。原先仅服务旧离线路径的固定 Segmenter 选择保留为测试控制，生产使用停顿策略；VAD 失败时仍使用原有固定上限降级。setup 验证也使用共享会话与收尾，避免启用两阶段后重复整理。

**数据与所有权。** 纯 ASR 连接只引用当前音频，校验纯文本及严格响应协议，不构造记忆所有者；文字后处理的串行 worker 独占学习。lab 保留生产开关，用显式隔离文件和时钟；默认丢弃反馈，显式 delivered 才确认。schema 3 记录阶段身份、原始识别、编辑前 final、真实段边界、输入快照和反馈；一体式的原文从实际段输出取得，避免把本地纠错后的 final 冒充模型原文。逐段记录实际记忆时间，使跨保留期的录音能按当时快照重放。

**失败合同修正。** WAV 输入使用宿主取消令牌的子令牌。生产队列/录音故障可以取消输入，但不反向取消宿主；随后仍走共享不完整结果与警告，保留已完成文字。直接复用同一令牌的较简单实现会把内部故障误当用户取消，回归先复现 `WAV recognition cancelled` 后再验证修正。文字后处理错误、空正文及无效结构回退原文；输入失败、部分失败、超时不进入文字整理。

**确定性验证与消融。** `cargo fmt --all -- --check`、`cargo test --locked`、`cargo test --locked --all-features`、`cargo clippy --locked --all-targets --all-features -- -D warnings` 均通过，vendor rdev 的既有警告未改动。公共 lab 入口使用本地 WS/HTTP 对端验证不同 raw/final 的评分、原文回退、部分失败、独立编辑反馈、纠错与保留期，并拒绝阶段、final、反馈及时间 trace 篡改。生产 PCM 回放测试覆盖与 live 相同的帧/停顿策略、不同采样率、精确边界和单样本尾段、不重复增益及取消。

在一次性源码副本中仅将共享收尾的 `post_processor.process_with_trace` 换成 `PostProcessor::Disabled.process_with_trace`，`shared_finalization_keeps_cleaned_text_and_reports_raw_fallback` 按预期失败（原始文字与整理后的文字不同）；原实现通过，证明仅跑 raw STT 无法满足 final 合同。[消融日志](../../.amp/in/artifacts/production-recognition-20261112/cleanup-ablation.log)记录实际运行。取消令牌的较简单变体已有先红后绿的故障回归；报告中的纠错结果、反馈及实际时间被单独篡改且重算摘要后仍被拒绝，证明不能只凭摘要替代生产重放。临时副本已删除，验证中发现的空失败 stdout 回归由既有 CLI 测试覆盖并修正。消融副本曾共用 target，随后主工作区出现与禁用后处理一致的异常测试结果；该轮丢弃，移除副本并清理本包产物后重建，完成上述全量验证。

**真实模型对照。** `.156` 的同一 Qwen 模型与未改动 WAV，在旧原生 `convert` 中重复返回提示词示例；新原生 `prompt-lab evaluate` 经完整生产路径未再复制示例。新构建另完成整组复测，阶段证据明确记录 `audio-only-v1` 和实际文字后处理。细节、重复结果、独立阶段耗时及复现入口见[诊断记录](https://ampcode.com/artifacts/9bd26c46-f9dc-452b-a25f-9e0eec6867f0)和[证据目录](../../.amp/in/artifacts/production-recognition-20261112/)。文字阶段仍保留部分错音且未充分补标点，不能把消除示例复制等同于准确率全面修复；报告尚待语义复核，没有标成最终门槛通过。

**交付与未验证范围。** 真实测试使用独立 HOME 和明确 `.156` 两个端点，不读取个人记忆、不写正常历史、不触发原生预览或文本注入；前后个人配置 SHA-256 相同。个人应用未替换、未重启，Windows、真实个人记忆、麦克风实录和目标应用交付未在本轮执行。当前仅完成本地实现及上述验证，未提交、推送、创建 PR 或部署。

### [004] — 2026-11-12 — 普通评估复用生产初态时钟会话和确认学习

**背景与已批准决策。** 用户进一步要求 lab 与 main 除候选 prompt 外保持一致，并确认使用相同配置、只读生产记忆快照、生产时钟和会话复用，学习只写隔离副本。此修订取代 [002]/[003] 中普通评估使用空记忆、清除配置词库、每样本重建和默认丢弃反馈的安排；显式场景保留人为种子/时钟/反馈，但明确标作诊断。输入仍为 WAV，不触发原生预览、历史或真实注入。

**实施。** 普通评估通过生产 `Memory::load_or_ephemeral` 捕获配置词库、保留期内观察和已学纠错，保留完整规范化状态而非只复制可见四个上下文；禁用记忆不读文件，损坏文件与 main 使用同一恢复逻辑。源文件不写入。临时文件初始化后，整个数据集复用一个 `RecognitionPipeline`、连接、生产系统时钟与串行学习所有者。成功且非空的编辑前 final 按原文确认，模拟 Enter；失败或不完整结果不学习。reference 永远只用于评分，不传给识别或反馈。

**时间和可验证证据。** 快照、文字阶段输入和确认均记录实际记忆所有者使用的时间；`confirm_with_ack` 等待反馈处理及保存尝试，不能据此宣称持久化一定成功。报告 schema 仍为 3，执行策略升级到 `production-recognition-v2`，保存初始 `memory_seed` 并按样本顺序用生产 Memory 重放。普通 prompt 比较拒绝不同初始记忆、非候选阶段和执行策略，须重跑相容基线；显式场景仍可记录种子变化。含个人状态的报告必须保持私有，不直接附到公开 PR。

**取舍。** 不直接复制原始记忆字节：规范化捕获复用生产校验、裁剪与损坏恢复，同时避免报告保留无效私人数据。增加 owner 返回的真实时间与确认应答，避免用稍晚的报告时间重算保留期；桌面的既有异步确认保持不变。普通评估假设每次接受 final，能验证连续识别与学习合同，不能冒充真实用户编辑或平台投递验证。

**已执行的验证。** 本地 HTTP 对端分别经桌面装配入口和公开 `evaluate` 处理同一音频与种子；实际上传的生产处理后 PCM、文字请求、final 和后续学习一致，仅 ASR 候选指令不同。WAV 原文与上传 PCM 的差异已追到现有响度调整；测试比较 main/lab 的实际上传，并检查采样格式与样本数，不把原始容器字节当等价合同。普通 Integrated WS 回归验证跨样本复用连接、配置词库/历史/纠错及上一 final 的反馈，参考答案不进入请求。两类测试均确认源记忆字节和 mtime 不变。独立递增时钟测试区分快照、确认和后续快照的真实时间；记忆捕获回归覆盖隐藏上下文/纠错、过期、禁用、损坏与缺失恢复。从最新 `origin/master` 新建专用 jj workspace，只提取本功能差异，排除并行 gain 的代码、集成点和文档；在该工作区通过 fmt、默认与全特性完整测试、all-target/all-feature Clippy。CLI help 也已实际运行。

**状态消融。** 在专用工作区分别临时替换普通初态为空种子、将普通反馈改为丢弃，每次只改一个机制，运行同一个 `normal_lab_matches_main_requests_and_learning_except_candidate_prompt`。前者因 `production input memory differs from replay` 失败，后者因 `ordinary samples must confirm their actual unedited final` 失败。恢复实现后重跑默认与全特性完整测试均通过。它们证明保留初态与连续确认是生产一致性的必要条件；真实 owner 时间另由区分输入、确认和报告时间的时钟回归覆盖。[初态消融](../../.amp/in/artifacts/production-recognition-20261112/state-seed-ablation.log)、[反馈消融](../../.amp/in/artifacts/production-recognition-20261112/state-feedback-ablation.log)。

**真实模型质量阻塞。** 当前隔离构建在 `.156` 上完整评估五段 WAV，实际策略为 v2、生产时钟、自动确认 final；调用能结束，但第二段 ASR 原文正常，文字服务却输出 `用Codex连接Redis，再让Codex检查。`。报告中第二步原始 JSON 已包含该句，非本地纠错引入。用同一生产 prompt、模型、temperature、`stream:false` 和 Rust JSON 键序复放文字请求，两次带实际 Redis 历史的请求都复制示例；只清空 contexts 的对照保留本次原文。较早改变 JSON 键序且省略 stream 的探测未复制，不能拿它当精确复放。上下文和请求布局影响此服务输出的事实已复现；服务内部机制仍未知，不能断言是缓存泄漏。[原生结果](../../.amp/in/artifacts/production-recognition-20261112/state-parity-native-summary.json)、[精确文字对照](../../.amp/in/artifacts/production-recognition-20261112/state-context-exact-cleanup-probe.json)。

本轮三条短音频未再复制示例，但连续状态的第二阶段仍会污染结果并学习错误 final，因此不得声称污染已全面解决或质量 gate 已通过。保持生产路径一致才能暴露此问题；不通过恢复空状态、禁用学习、改变键序或增加输出黑名单掩盖它。当前计划仅交付路径一致性和已批准的两阶段实现；PR 保持草稿，下一步需修订文字 prompt 方案并重新获得相应批准，不能合并或切换个人应用。个人配置、源记忆及已安装应用均未改动。

### [005] — 2026-11-12 — 无记忆所有者的 HTTP 报告按生产状态校验

**自检发现与修正。** 在发布审查仍运行时补查默认 HTTP 入口，使用非空配置词库、关闭后处理和本地 HTTP 对端。五段识别都成功，但报告构造因 `production input memory differs from replay` 失败：实际路径没有记忆所有者，纯验证却只按全局开关启用了种子记忆。该问题由原生 CLI 独立复现，不依赖审查结论。新增同一公开 `evaluate` 入口的回归先得到相同失败，再让 Replay 仅在所选 Integrated 或 cleanup 阶段实际拥有记忆时启用记忆；保留配置开关和报告种子，不改变生产识别、配置或其他模式的学习。

**验证。** 回归修正后通过；重跑同一原生五段场景成功生成报告，final 均等于本地对端文字，before/after 均为空且只有 STT 请求，没有伪造确认学习。完整默认/全特性测试、all-target/all-feature Clippy 与 fmt 再次通过。[原生失败](../../.amp/in/artifacts/production-recognition-20261112/http-without-cleanup-probe.json)、[修正后原生结果](../../.amp/in/artifacts/production-recognition-20261112/http-without-cleanup-green.json)、[回归修改前](../../.amp/in/artifacts/production-recognition-20261112/plain-http-replay-red.log)、[修改后](../../.amp/in/artifacts/production-recognition-20261112/plain-http-replay-green.log)。

**发布边界。** 较早目标的双路审查在 600 秒后超时，未生成完整结构化报告，不能视为通过；后续修正也尚未审查。已停止 bookmark 推送与 PR 创建，没有根据不完整审查信号做进一步修改或自动重跑审查。完整门禁通过前不发布；004 中的真实文字污染另需后续 prompt 方案和质量验收，不因测试通过而消失。

### [006] — 2026-11-12 — 延长审查后仅发布草稿 PR

**明确授权。** 用户要求“延长审查时限，重跑通过后提草稿 PR”。本轮将 `JJ_REVIEW_TIMEOUT_SECONDS` 从默认 600 秒提高到 1800 秒，对隔离工作区的完整最新提交执行同一双路只读审查。只有获得有效通过结果，才把 `feat/production-recognition-parity` 指向门禁返回的精确提交并推送，创建面向 `master` 的草稿 PR。执行失败或 critical 发现仍停止发布，不把局部结果当成通过。

**范围与报告。** 本次授权不包含按发现修改代码，也不包含文字 prompt 的新设计；所有有效审查发现随 PR 或交付结果完整报告。已有本地验证及连续状态污染证据保持不变，草稿 PR 明确区分路径一致性完成、质量验收未通过和未执行的原生验收。不公开个人状态报告，不合并、不部署、不激活个人配置或替换已安装应用。

### [007] — 2026-11-12 — 跟随 CI 工具链修正测试分块写法

**失败与修正。** [首轮 macOS CI](https://github.com/b1indsight/viberwhisper/actions/runs/43181427563/job/131058081588) 的构建及两组测试通过，但 Rust 1.99.1 的诊断模式 Clippy 拒绝三个测试中的常量大小 `chunks_exact`；此前本地 Rust 1.94 不含该检查。使用已安装的 1.99.1 和完全相同的 CI 命令复现三个错误后，将调用改为 `as_chunks::<N>().0.iter()`，保留块大小、顺序和忽略不足整块尾部的行为。语音选择谓词解引用一次以适配数组引用，避免直接应用自动建议产生的类型错误。不关闭 lint、不改变 CI 或工具链配置，不改生产识别行为；两个既有审查发现和文字污染均未在本轮修改。

**验证与发布边界。** 同一 `cargo +1.99.1 clippy --locked --all-targets --features diagnostic-logs -- -D warnings` 先红后绿；1.99.1 的诊断模式完整测试、1.94 的全特性完整测试与 fmt 通过。现有回归不变，无新增行为或额外状态，消融不适用。本补丁作为已发布提交的子提交独立审查，门禁通过后仅更新同一草稿 PR，不改写已发布历史；远端结果由新一轮 CI 决定。[复现日志](../../.amp/in/artifacts/pr177-clippy-199-red.log)、[修正后 Clippy](../../.amp/in/artifacts/pr177-clippy-199-green.log)、[1.99.1 测试](../../.amp/in/artifacts/pr177-test-199-green.log)、[1.94 测试](../../.amp/in/artifacts/pr177-test-194-green.log)。
