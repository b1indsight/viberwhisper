# 67 - 保留 30 秒上限，按语音通断提前提交识别

**阶段：** Implementation；已在独立 workspace 接入两条桌面路径并通过本地自动检查；真实中文语料、服务联调及 Windows 实机验收仍待完成。初始方案保留在第 1 节，实际实现与限制见第 2 节。
**PR：** 以同一 Draft PR 提交到 `master`，验收未完成前不标记 Ready，不合并或发布；[按 feature 分支查找 PR](https://github.com/b1indsight/viberwhisper/pulls?q=is%3Apr+head%3Afeat%2Fpause-aware-recognition-segments)。
**Bookmark：** `feat/pause-aware-recognition-segments`。
**初始基线：** 2026-11-10 再次 fetch 后的 `master@origin`，即 [v0.5.3，34288ff9](https://github.com/b1indsight/viberwhisper/commit/34288ff9bc2ad6cf4fdba4864ac2ec6c7896c275)。
**送审基线：** [7aa37aef，诊断日志功能](https://github.com/b1indsight/viberwhisper/commit/7aa37aef)，保留其现有行为。
**工作区：** 独立 JJ workspace `pause-aware-segments-plan-20261110`，目录 `.amp/in/feature-workspaces/pause-aware-segments`；不修改主工作区或另外两个 feature 的工作区。
**需求来源：** [父线程](https://ampcode.com/threads/T-01a1c430-82fd-70fe-9333-d5756751cb9b)；[本线程调查与顾问咨询](https://ampcode.com/threads/T-01a1c439-d77d-71d2-9889-001df09562eb)。

文中 `.amp/in` 链接指独立 workspace 的本地验证材料，不随 Git 提交分发；检查结论及可查看截图另在上述本线程中提供。

## 1. 初始方案

### 摘要与动机

用户最新要求原话：**“第三点 保留30s限制，30s内根据通断发送识别”。** 本方案保留每个识别音频段的 30 秒硬上限，在段内出现持续的讲话停顿时提前提交；连续讲话达到上限仍切段，手动停录仍提交尾段。整次录音可以超过 30 秒，停顿切段不会替用户停止录音。

暂按“发送识别”指音频提交理解：HTTP 发起该段 STT 请求，Realtime 提交该段音频并创建响应。Realtime 的提前 PCM 上传与最终 commit 是两个时机。不增加自动向目标应用发送文字；继续使用现有预览、停录后的编辑及 Enter 确认。语音活动检测（VAD）只提供声学上的人声/非人声概率，不证明句意完整，也不判断语气。

例子：用户口述约 8 秒后停顿，当前通常等到 30 秒或用户停录才请求识别；拟在持续停顿达到初始阈值后提交这一段，录音继续。用户接着说话时开启下一段；连续口述 65 秒且没有合适停顿时仍按 30/30/5 秒处理。提交变早不保证服务返回更快，HTTP 也不会因此新增录音中的文字流式预览。

### 现状调查与约束

以下是基线源码确认的事实，后面的数值策略和接口均为提议。

- 音频模块固定上限为 30 秒和 23 MiB，按较小的完整帧容量处理；离线 WAV 读取器也使用它。[时长与大小](../../src/audio.rs#L7-L9)、[容量计算](../../src/audio/chunk.rs#L68-L84)、[WAV 读取器](../../src/audio/wav_file.rs#L19-L32)。
- 普通 HTTP 录音按固定容量发出 WAV；Realtime 改用 200ms WAV 信封，并将录音器 PCM 积压限制为 60 秒。HTTP 的录音器缓冲目前没有同样的容量限制，不能把它描述成已完整有界。[录音缓冲](../../src/audio/recorder.rs#L171-L227)、[模式选择](../../src/ui/listener.rs#L118-L124)。
- 回调只做下混、增益、追加 PCM 和就绪通知；winit 线程取块并在锁外编码。现有职责不允许把 ONNX 推理或网络请求移进回调或 UI 线程。[回调与取块](../../src/audio/recorder.rs#L434-L504)。
- HTTP 只有一个顺序转写 worker，待处理完整 WAV 队列容量为 2。满队列时当前标记该块失败并继续；不能原样用这个容量接收 200ms 帧，也不能丢掉中间帧后拼接出表面完整的音频。[HTTP 会话](../../src/core/orchestrator.rs#L370-L425)、[worker](../../src/core/orchestrator.rs#L512-L544)。
- Realtime 的 worker 通道容量为 300 个录音帧；`Recording::push_pcm` 当前在 720000 个 24kHz 样本处结束一段，停录时刷新重采样尾部。等待一段响应期间，后续音频在本地排队。[队列](../../src/dictation/engine.rs#L23-L25)、[输入与结束](../../src/dictation/engine.rs#L378-L429)。
- Realtime 明确使用 `turn_detection:null`，commit、前缀、响应及清理都由客户端拥有。静音段清空服务端缓冲，不创建识别响应；小于 100ms 的有效尾段补静音到 2400 样本。[会话配置](../../src/dictation/transport.rs#L87-L98)、[完成一段](../../src/dictation/engine.rs#L431-L495)。
- 已有 Silero v6.2 VAD 在整块 WAV 的 16kHz 分析副本上工作，每次重置状态，遇到第一个达到 0.3 的帧便返回。512 样本即 32ms；它是整段含人声过滤器，不能直接作为持续停顿检测器。[批量状态与推理](../../src/audio/vad.rs#L11-L109)。
- 上传前的整段能量/VAD 门控及 HTTP 增益、固定重试有独立职责；本功能不能用端点检测取代它们。VAD 失败当前保留上传，纯数字静音先被能量门控过滤。[预处理](../../src/audio/preprocess.rs#L11-L58)、[HTTP 提交](../../src/transcriber/api.rs#L256-L265)。

### 目标、范围与成功标准

1. 两条桌面听写路径共享同一停顿判定，保留 30 秒和现有字节上限；正常停顿可在上限内提前提交。
2. 初始策略中 100–600ms 的短停顿不触发提前切段；较长停顿达到阈值才触发。保留短词和尾音，不要求累计讲满某个时长才处理。
3. 正常处理中，在分段层每个成功接收的源样本恰好归属一段：不裁剪、不重叠、不复制预录音；按输入顺序识别和合并。溢出、损坏或取消按下文显式失败/丢弃策略处理；整段静音过滤和识别模型仍可能误判，不能由样本守恒推导出“识别绝不漏字”。
4. 正常停录刷新所有已接受输入的尾段，不等停顿阈值；取消不提交尚未提交的尾段，不交付取消会话的文字。
5. 未完成音频和网络工作有明确上界，溢出显式失败，不能阻塞回调/UI 或静默丢音后报告成功。
6. 保留 Enter/Esc、只读进度、历史、目标恢复、确认后学习及单会话身份契约。

| 入口 | 拟议范围与影响 |
|---|---|
| 普通桌面 HTTP 听写 | 启用短帧输入和停顿分段，后台按段请求；可选 LLM 整理仍在整次停录后的原位置执行 |
| 桌面 Realtime | 保留短帧上传，增加本地停顿 commit；前缀、单响应、清理及确认流程不变 |
| HTTP `convert` | 保持固定 30 秒/23 MiB 的 `WavChunkReader` 行为 |
| Realtime `convert` / `OfflineDictation` | 显式选择固定分段，保留连续 24kHz 重采样与 EOF 尾段 |
| `prompt-lab record` | 保持现有固定块 HTTP 采集，不启用停顿分段；完整会话 WAV、原始转写侧车和归档队列行为不变 |
| HTTP、Realtime prompt-lab 评测及 Realtime 场景评测 | 保持固定分段，不改变数据集/报告格式、评分版本或 `input_layout`；不把这些评测当成本 feature 在线切点的验证 |

这些入口确实共享代码：Realtime 离线转换使用 `Recording::new()`，评测使用 `for_evaluation`；原始采集强制 HTTP，归档在 UI 线程向容量 2 的队列做阻塞发送。保留原始采集的固定块，可以避免把发送频率提高到每秒五次而新增归档背压，也不需要归档合块器。[离线转换](../../src/dictation/offline.rs#L47-L85)、[场景评测](../../src/dictation/evaluation.rs#L172-L204)、[后端选择](../../src/core/recognition.rs#L26-L41)、[归档发送](../../src/prompt_lab/capture.rs#L155-L176)、[归档完整 WAV](../../src/prompt_lab/capture.rs#L240-L270)。

不包括语义断句、语气分析、服务端 VAD、自动停录/自动注入、窗口定位或气泡留白、并发响应、新识别后端、新公共配置、版本号变更、依赖/模型升级，以及修改他人的服务端。已有 30 秒 HTTP 收敛期限和 90 秒 Realtime 响应期限与音频段长上限是不同概念，本次不调大它们。[HTTP 收敛](../../src/core/orchestrator.rs#L26-L27)、[Realtime 响应期限](../../src/dictation/transport.rs#L42)。

### 拟议通断判定与用户行为

**最小策略：本段出现过人声证据后，等待持续的低人声概率；短暂停顿继续积累在原段中。** 先使用与现有保守门控一致的 0.3 概率阈值：概率达到阈值便记录本段出现过人声，并清零连续安静计数；低于阈值才累计安静帧。只有本段出现过人声，安静时间才可能触发停顿切段。概率不等于已知真实声学标签。

| 参数 | 建议初值/候选值 | 地位 |
|---|---|---|
| 段长、字节上限 | 30 秒 / 23 MiB，按现有容量计算取小值 | 用户约束/既有安全策略，不可因等待停顿延长 |
| VAD 分析帧 | 16kHz 的 512 样本，即 32ms | 复用现有模型接口 |
| 人声概率阈值 | 先用 0.3；是否需要双阈值仅由对照证据决定 | 初始工程选择，尚未测量在线效果 |
| 持续停顿 | 先用 800ms，即连续 25 个低概率帧；对照 1200ms，必要时再试 600ms | 待语料校准，不是用户已经确定的值 |
| 源音频分析推进 | 约 20ms 一步，跨传输包保留不足一步的输入 | 内部实现初值；消除 200ms 包边界对判定的任意影响 |

非 32ms 整倍数的候选值向上取整到完整分析帧，并报告实际时长；例如 1200ms 对照实际需要 38 帧，即 1216ms，不能按传输包舍入。

本版不加“临近 30 秒改用 300ms 停顿”、最低有声时长、10 秒空闲轮转、自适应噪声学习或回溯最优切点。前两项可能违背保护换气、短词的目标，其余机制会增加状态和验证成本；需要证据后再决定。800ms 以上的句内思考停顿仍可能切段，这属于声学检测的能力边界，不承诺所有句子都完整保留在一个识别请求里。

| 情况 | 拟议行为 |
|---|---|
| 8 秒讲话，短暂停顿 300ms，再讲话 | 不因该停顿切段；也不重置 30 秒计数 |
| 8 秒讲话，随后持续停顿 | 连续安静达到约 800ms 后尽快形成切点，通常在约 9 秒附近提交；录音和托盘录音态继续 |
| 无停顿的 65 秒输入 | 30/30/5 秒；最后 5 秒由停录提交 |
| 29.5 秒才停顿 | 800ms 尚未满足便到 30 秒，优先在 30 秒硬切；不缩短阈值来抢切 |
| 刚好 30 秒停录 | 只提交已满的一段，不再发空尾段；硬切、停顿和 stop 同时发生只关闭一次 |
| 停顿尚未达阈值便手动停录 | 不再等待，提交包含未满停顿的完整尾段 |
| 有效讲话不足 100ms 或只剩一个样本 | 不按“太短”直接丢弃；交给已有整段门控。Realtime 接受时沿用补静音规则，补零不计作新增源音频 |
| 从未讲话的纯静音、段后长静音 | 不以每 800ms 的定时器反复请求；没有人声证据的开放段等上限或 stop 整理。纯数字静音经 HTTP 门控不发请求；Realtime 可已上传 PCM，但应 clear，不能 commit/create response 生成文字 |
| 端点 VAD 初始化/重采样/推理失败 | 本次录音剩余部分退回固定上限分段，保留原样本，记一次无语音内容的警告；不把未知结果当静音裁掉 |
| 已有请求尚未返回 | 后续输入有界排队，空闲后依次分段/提交；不承诺在繁忙后端上仍能按墙钟即时提交 |

切点取检测确认时推进到的源采样游标，保留已收到的整段静音，不回溯裁到“最后一个有声样本”。处理当前输入前先求出切点，再让各传输适配器逐段消费：下一段的样本不能先 append 到 Realtime 当前段再尝试撤回。重采样延迟可能让切点略晚，不能声称固定延迟必然只移动静音；通过边界语音语料检查首尾识别质量。暂不为此增加音频重叠或文字去重。

### 技术设计与受影响模块

#### 1. audio 拥有增量 VAD 与切点策略

保留批量存在性过滤的公共行为：每次/每声道从全新状态开始，短输入保守放行，原阈值和失败放行不变。将 `Detector` 的单帧推理从“逐帧扫描直到发现人声”的循环中分离出来，批量检测和增量检测复用同一帧推理。现有函数内的模型 OnceLock 移到模块内共享，全局仍只加载一份现有模型，推理锁只保护模型调用；不得持锁重采样、组段或等待网络。录音的递归状态不放进全局对象，批量调用也不能污染它。

拟议接口（下面均为计划声明，不是已实现代码）：

```rust
// src/audio/vad.rs；复用现有 Detector，不新建模型/错误抽象。
impl Detector {
    fn predict_frame(
        &mut self,
        frame: &[f32; 512],
        state: &mut [f32; 256],
        context: &mut [f32; 64],
    ) -> anyhow::Result<f32>;
}

pub(super) struct StreamingVad {
    resampler: PcmResampler,       // 当前输入采样率 -> 16kHz，仅分析副本
    pending: VecDeque<i16>,        // 不足 512 样本的 16kHz 分析尾部
    state: [f32; 256],             // 本次录音的 Silero 递归状态
    context: [f32; 64],            // 本次录音的上一分析帧尾部
}

impl StreamingVad {
    pub(super) fn new(source_rate: u32) -> anyhow::Result<Self>;
    pub(super) fn push(&mut self, pcm: &[i16]) -> anyhow::Result<Vec<f32>>;
}
```

`push` 返回本次新完成的分析帧概率，不能重新扫描历史音频。i16 分析副本以 `f32::from(sample) / 32768.0` 归一化后才进模型；沿用现有有限值、概率范围、递归状态形状校验，异常触发固定分段降级。状态跨 200ms 包及分段连续，只有新录音或故障退回固定分段才丢弃。stop 不通过补造一长串静音来凑停顿阈值；所有源尾样本由 stop 的强制收尾规则处理。

已有 `PcmResampler` 硬编码输出 24kHz，不能直接用作 16kHz 分析器。拟将它的源头接口改为 `PcmResampler::new(input_rate: u32, output_rate: u32) -> anyhow::Result<Self>`，增加 `output_rate: u32` 字段，保留 `rate()` 的输入采样率含义，现有 Realtime 调用显式传 24000。不另造第二个相同的流式滤波/延迟补偿实现，也不修改发送给 STT 的音频采样率、增益或声道。先用已有分包/整包及尾部测试证明 24kHz 路径逐样本不变。[现有状态、计数和刷新](../../src/audio.rs#L45-L119)。

新增 `src/audio/segment.rs` 承担共享切点策略，不拥有网络、UI、会话身份或整个会话的 PCM：

```rust
pub(crate) enum Segmentation { Fixed, PauseAware }
pub(crate) enum CutReason { Pause, Limit, Stop }

pub(crate) struct Cut {
    pub(crate) offset: usize,     // 当前 push 输入中的结束偏移，不含后一个样本
    pub(crate) reason: CutReason, // 日志和验收区分停顿、容量、停录
}

pub(crate) struct Segmenter {
    max_samples: usize,           // 调用方按现有容量策略得到的本域硬上限
    open_samples: usize,          // 当前开放段已经消费的源样本数
    step_samples: usize,          // 约 20ms 的原始采样推进容量，至少一个样本
    step: Vec<i16>,               // 不足一次推进的分析输入，不是另存整段
    detector: Option<StreamingVad>, // None 表示 Fixed 或本次录音已降级
    speech_seen: bool,            // 当前段是否已有 >=0.3 的人声证据
    quiet_frames: usize,          // 当前连续低概率的 32ms 帧数
}

impl Segmenter {
    pub(crate) fn new(rate: u32, max_samples: usize, mode: Segmentation) -> Self;
    pub(crate) fn push(&mut self, pcm: &[i16]) -> Vec<Cut>;
    pub(crate) fn finish(&mut self) -> Option<CutReason>;
}
```

`push` 的返回偏移严格递增、位于本次输入内；无切点返回空数组。调用方据此消费各连续切片及剩余部分，源样本不会被分析缓冲重复输出。硬上限按收到的样本数精确计算，不受分析步长、VAD 帧长、滤波尾部或通知延迟延长；需要时在一步内部先硬切。停顿切点只在当前已推进游标形成，不要求回收上一包已经上传的样本。切段后重置段长和本段人声/停顿计数，保留流式滤波、分析残余及递归状态；`finish` 只在非空开放段返回 Stop，并清零段长，再次调用不得重复收尾。Fixed 模式跳过分析。

30 秒容量只保留音频模块这一处来源：HTTP 在原生单声道 i16 域调用已有 `max_frames_per_chunk`，Realtime 在 24kHz 单声道 i16 域使用同一容量计算，不再另写一个可漂移的 `30 * 24000`。23 MiB 与极端格式的既有规则继续有效；不增加可配置的停顿参数或通用策略框架。

#### 2. HTTP 复用现有 worker，先切段再调用现有 Transcriber

普通桌面 HTTP 开启已有的 200ms 录音帧及 60 秒 PCM 缓冲，保留 `enable_streaming_frames` 方法名，更新只提 Realtime 的注释/溢出诊断。原始采集保留固定块。回调仍不编码/推理/等待网络；winit 继续只取 WAV 信封并 `try_send`，流式 VAD、解码、组段和段 WAV 编码都在已有 HTTP worker 里做。

- `RecognitionConfig::Http` 增加私有 `segmentation: Segmentation`，由已有 `raw_capture` 入口选择 Fixed / PauseAware，不进入持久配置。普通 Delivery 和 Capture 的 recorder 模式必须与它一致。
- 保留 `SessionOrchestrator::new(transcriber, config)` 的完整块默认契约。新增 crate 内入口 `pub(crate) fn new_live_http(transcriber: Arc<dyn Transcriber>, config: OrchestratorConfig, notify: impl Fn(SessionId) + Send + Sync + 'static) -> Self`；它选择帧队列和停顿分段，不包装第二个转写服务。
- `RecognitionConfig::build(self, notify: impl Fn(SessionId) + Send + Sync + 'static) -> anyhow::Result<SessionOrchestrator>` 在 proxy 创建后组装上述路径；Fixed 和 Realtime 仍使用原入口。`SessionBackend::Http` 保存 `segmentation` 及可选的 `Arc<dyn Fn(SessionId) + Send + Sync>` 通知，创建每次录音的 worker 时传递；回调仅用于唤醒，不携带语音或文本。
- `WorkerMsg` 保留已有 `Chunk { index, chunk }` 给 Fixed，增加 `Frame { chunk: WavChunk }` 给 Live。worker 在首帧建立原生采样率的 `Segmenter`，保留一份最长 30 秒的当前段 PCM；之后采样率必须一致。Frame 路径不按 200ms 包分配识别段号。
- worker 在实际封段时分配连续的 `index: usize`，先发送新增的 `WorkerEvent::SegmentQueued { index }` 注册段，再沿用 UploadStarted/Completed。新增 `InputDrained` 表示 EOF 后所有已接受输入、尾段及请求均已处理；新增 `InputFailed { error: String }` 报告解码等不能继续保持输入完整的故障。它们走当前会话自己的 result 通道。
- `HttpSession` 增加 `segmentation: Segmentation`、`input_drained: bool` 和 `failure: Option<String>`；复用现有段状态、顺序合并与取消标志。Fixed 的注册、容量和失败语义不改；Live 不能因为当前段表为空或暂时全为 terminal 就提前结束，必须收到 InputDrained。输入 EOF 后才确定没有段时，再走 NoChunks/空文本路径。
- `SessionError` 增加 `InputFailure { error: String, partial_text: String }`：前者说明输入故障，后者保留故障前已完成的文字。尚未形成识别段的输入故障不能冒用一个虚构的失败段号。`finish_transcription` 将它映射为有/无部分文字两种警告预览，仍无学习确认能力；Fixed 采集不产生此错误，采集分支只补齐枚举匹配。
- HTTP 仍只有一个网络请求在途，重试、静音门控、增益及 panic 隔离沿用 `ApiTranscriber`。请求挂起期间不分析下一包；后续 PCM 排队，返回后按输入顺序补算。无需额外线程、完成段队列或并发识别。

#### 3. Realtime 接入同一 Segmenter，保持客户端拥有 commit

`Recording` 新增 `segmenter: Segmenter`；`Recording::new(mode: Segmentation) -> Self` 要求构造点显式选择。桌面 worker 传 PauseAware；`OfflineDictation` 传 Fixed；`for_evaluation(id: String, at: u64) -> Self` 签名不变但内部明确 Fixed。这些选择有负对照测试，不能靠默认构造“恰好没影响离线”。

`push_pcm` 先取得当前输入的切点，然后依次执行现有 begin/append；到一个切点才 `finish_segment`，完成清理后开始下一段，不能把整包或跨切点余下的 PCM 先 append 进去。`finish` 先刷新原有 24kHz 源重采样尾部并经过同一 `push_pcm`，再用 `Segmenter::finish` 关闭剩余非空段。

保持 `turn_detection:null`、冻结稳定前缀、前缀确认先于音频、关联 ID、一个段一次 commit/结果、静音 clear、短尾补零、结果校验先于交付、音频和文本项删除确认、录音内临时学习及最终确认反馈。停顿不会形成新录音身份，也不能把同一录音的多个段算成多次独立词汇支持。[当前生命期](../../src/dictation/engine.rs#L238-L317)、[段结果与学习](../../src/dictation/engine.rs#L451-L495)。

#### 4. 有界积压、失败、停止与取消

| 层次 | 拟议容量/执行约束 | 满载处理 |
|---|---|---|
| Live 录音器尚未取走的原生 PCM | 两条桌面路径都用现有 60 秒容量 | sticky 故障；不继续接受样本并假装完整 |
| HTTP Live 输入通道 | 300 个正常 200ms WAV 帧，约 60 秒；只接收录音器此模式产出的短帧 | 首次 Full/Disconnected 记故障，拒绝此后的输入，唤醒 UI 停录 |
| HTTP 当前段/请求 | 单 worker、一个最多 30 秒的当前段或在途段，无第二个待完成段队列 | 网络慢时输入通道积压，不另开请求或扩大段长 |
| Realtime 输入通道及当前段 | 现有 300 帧、当前段最多 30 秒；只允许一个响应 | 保留现有显式故障、取消和连接退役语义 |
| 分析缓冲 | 不足一分析步/滤波块/512 样本帧的残余，逐步消费 | 不保存整次录音的 VAD 概率或无界分析副本 |
| Fixed 原始采集 | 现有完整块及容量 2 的识别/归档通道 | 本次不修改其语义 |

300 帧等于约 60 秒依赖录音器确实只产出 200ms 包，不能对任意 30 秒 WAV 入参都这么换算。Live 是 crate 内的封闭入口，只接短帧录音器；停录的残余也按相同短帧容量切开，离线完整块不得进入该队列。由生产输入边界限制载荷，不在 UI 新增 WAV 全量解码；worker 校验解码/格式一致性。按生产输入粗略计，未完成音频不超过录音缓冲 60 秒 + 通道 60 秒 + 当前段 30 秒；在 192kHz 单声道 i16 下约 57.6 MB PCM，加上有界 WAV、滤波和瞬时复制开销。最终文字/已完成段元数据随用户口述长度增长，属于结果，不是未完成音频积压。

HTTP Live 的首次输入故障锁定一个连续前缀；不再把之后的帧越过缺口接进该段。`recording_error` / `fail_session` 扩展到 HTTP，worker 故障用上述通知唤醒现有 `AudioChunkAvailable` 处理。UI 在取块循环之前和之后都检查故障，即使没有可取块也能停止；正常 stop 先停设备，再按序送残余、关闭输入，后台刷新尾段。不得仅在 `while let Some(chunk)` 内检查故障，避免没有新音频时漏报。[现有检查位置](../../src/ui/listener/event_loop.rs#L458-L485)、[stop 路径](../../src/ui/listener/event_loop.rs#L657-L678)。

故障与主动取消分别处理：

- 正常 stop：处理所有成功入队的帧和尾部，关闭输入即 EOF。队列已满导致尾段不能接受时必须返回明确的不完整结果；“手动停录提交尾段”不覆盖容量已失败的场景，不能把它报告为正常完成。
- HTTP 输入满、worker 退出、设备/编码等完整性故障：停止新输入，处理已接受的连续前缀；按现有 30 秒收敛期限交付 InputFailure 与已完成段。若 worker 在某帧解码失败，已入队的后续帧也不能越过该缺口继续；报告故障后退出，不能等不到 InputDrained 却当成功。单段 HTTP 请求失败仍记录段号并允许后续段继续，最后为 PartialFailure。
- HTTP 收敛：deadline 覆盖尚未分段的输入、尾段和网络请求，不在开始最后请求时重置。没有 InputDrained 即使已有段均完成也不能成功；超时保留已完成文字，未排空输入至少记作一个未完成工作项，不能伪报“0 项仍待处理”。相应将 `ConvergenceTimeout` 的显示从精确“chunks”改为未完成音频工作项，Fixed 的数量行为不变。超时后置取消标志，丢弃尚未处理的输入，不让已无人接收结果的 worker 继续启动分段请求；在途调用结束后退出。不为了提前提交修改现有超时值。
- Realtime 故障：保留已验证结果供部分失败预览，放弃本录音记忆更新，清理/退役连接，不重放状态不明的音频；不试图继续提交未知尾部。沿用现有故障取消与用户取消的区分。
- 主动 Esc/取消：停止录音，丢弃待处理帧、开放段和未提交尾部，撤销会话所有候选文字的交付；与故障竞态时也以用户取消的交付撤销为准。无法撤回已经进入 `ApiTranscriber` 调用的请求及其既有重试；返回值仅由该会话通道接收并丢弃，不能影响新 SessionId。置位后不启动新的分段调用，后台网络等待仍受已有请求超时/重试约束。
- 只有正常确认交付才写历史/记忆；分段、临时结果、部分失败及取消都不能绕过原确认能力。[现有错误预览](../../src/ui/listener/event_loop.rs#L871-L948)。

### 候选策略与取舍

| 方案 | 判断 |
|---|---|
| 固定 30 秒或一律改成更短固定时间 | 最简单的基线，但不能按用户停顿提前提交，或会无差别增加碎段 |
| 纯 RMS/能量 + 持续低能量 | 计算便宜；噪声与轻声依赖阈值，现有能量门控只回答是否值得识别。作为简化消融，暂不作为声学端点的默认方案 |
| 增量 Silero + 单阈值 + 持续停顿（推荐） | 复用已打包模型和保守阈值，只增加逐录音状态与计数；不增加声学依赖。仍需真实语料校准 |
| 双阈值滞回、最低有声时长、临近上限缩短停顿阈值 | 可以比较，但未有本项目收益证据；可能漏掉短词的提前处理、增加换气切段。本次先不采用 |
| 每 200ms 对整个增长中的 WAV 重跑已有 VAD | 批量调用会从头找“有没有人声”，不会识别结尾停顿，还重复计算，不采用 |
| 服务端 `server_vad` / `semantic_vad` | 不能覆盖普通 HTTP，也不假设兼容服务具备；会改变客户端 commit 所有权，不采用 |
| 回溯切到最后有声点、增加前后音频重叠 | 需要 Realtime 暂存/重传与防重复策略；当前样本守恒方案不需要，不采用 |

HTTP 额外分段线程与单 worker 的比较：

- **分段线程与网络分离：** 网络请求挂起时也能实时计算切点，代价是多一线程、帧与段两级队列、取消/EOF 协调，以及流式与整段 VAD 的模型锁竞争。请求仍串行等待，计算出切点不代表能更早发下一请求。
- **同一 worker 处理短帧（推荐）：** 后端空闲时可在同样停顿处提交；繁忙时暂停检测、输入有界排队，恢复后顺序补算。与现有 Realtime 结构一致。代价是补算耗时计入排队延迟，且积压可能超过停录后的原收敛期限。

只有测得补算/控制消息延迟不达标，或另行批准并发请求时，才重新评估分离线程；不能未经验证先承诺“60 秒音频一定亚秒补算完”。

### 成本、风险与缓解

- **短段的质量与成本：** 段数、HTTP 请求数、Realtime 前缀/响应开销及限流概率可能增加；中文专有词或改口可能因跨段缺少声学上下文而退化。比较 800/1200ms 和固定基线，保留有意义的重复，不做模糊文字去重。
- **声学误差：** 风扇、人声背景、音乐、轻声和拖长尾音会影响判断；长停顿不能证明句子已结束。VAD 故障只取消提前切段，不新增裁剪/静音丢弃。既有整段门控仍可能拒绝低能量讲话，属于明确保留的限制。[能量阈值](../../src/audio/signal.rs#L7-L9)。
- **边界质量：** 只切连续区间能够证明不漏采样、不重采样，不能证明识别模型不漏字；停顿阈值附近突然恢复说话、词跨 30 秒必须进入质量验收。不要借“保护尾音”加入可导致重复音频的默认重叠。
- **吞吐与内存：** HTTP 首次采用短帧和录音器容量，属于行为变化；慢后端下会明确停止而不是持续积累。报告各层容量、实际峰值和网络在途数，不通过改大队列或超时掩盖无法持续消费。
- **CPU/控制消息：** 增量推理运行在后台，但 Realtime runtime 为单线程，批量 VAD 的全局模型锁也可能带来停顿。测试每 200ms 输入的总 CPU 时间、追赶速度、心跳和取消响应；不先假设需要额外线程池。
- **共用重采样器：** 增加目标采样率可能影响旧的 24kHz 尾部/长度；先保行为验证，再接入端点策略，不把声音效果或包装依赖升级混进来。
- **评测盲区：** 既有固定分段 prompt-lab 成绩不能证明桌面提前分段的质量。使用同一语料驱动生产 Live 入口做独立对照；不偷换报告含义，也不把人工参考文字发送给识别服务。

### 实现顺序、交付与回滚

只有用户明确批准方案后才开始以下实现/测试；保持一个 feature record、一个 bookmark、一个 PR，不创建替代实现 PR。

1. 保行为拆出单帧推理，并让已有流式重采样器支持显式 16kHz/24kHz 目标；原批量过滤及 24kHz 路径先通过不变性验证。
2. 增加共享 Segmenter 的样本计数、持续停顿和退回固定策略；用确定性概率序列验证切点，不先依赖真实模型碰巧产生的概率。
3. 接入 Realtime，所有桌面/离线/评测构造点显式选模式；证明 begin/append/commit/clear/delete、尾部和稳定前缀不变。
4. 接入 HTTP Live 短帧和结果注册/排空标记，保留 Fixed 输入；扩展故障通知和 stop 收敛；完成满队列、取消及 EOF 竞态验证。
5. 组合验证两条桌面路径、原始采集/离线负对照与预览确认；用同一语料做消融、性能和服务联调，按结果确定停顿初值。
6. 同步 `docs/usage.md`、`docs/dictation.md`、audio/core/dictation 架构说明及 changelog；本轮仅计划与索引，不把未实现功能写成当前使用方式。

不迁移配置、数据集或历史，不增加用户开关。验证不达标则不发布此功能；需要回滚时将两条 Live 入口退回原固定分段，并保留原始音频/历史。模式枚举用于区分实际入口与验证，不是公开的运行时配置系统。发布、部署和版本 tag 不属于本次计划任务。

### 验证与消融策略

**下面全部是批准后的验证计划，不是本轮已执行的测试。** 永久测试优先扩展已有 audio/orchestrator/dictation 文件，通过输出段、实际请求音频、预览/错误结果验证契约；性能探针和语料对照驱动可临时使用，完成后移除，不保存一套重复测试框架。

#### 确定性边界与生命周期

| 要验证的风险 | 能区分正确/错误实现的输入与标准 |
|---|---|
| 把换气当停顿 | 相同非对称语音夹 200/400/600ms 低概率帧，不提前切；达到 800ms 的第 25 个低概率帧才具备停顿条件；中间插一帧人声证据应重新计时 |
| 包边界改变结果 | 同一 PCM/概率序列按单样本、317 样本、200ms 及不规则片段输入，转换为绝对偏移后的切点一致；端点在分析推进游标，允许的量化规则事先固定 |
| 切点把后段音频先上传 | 讲话 A、足够停顿、讲话 B；检查 HTTP 每次请求和 Realtime 每次 committed PCM 的连续区间，不只检查调用次数；B 的样本不得出现在 A 段 |
| 30 秒 off-by-one | 在 16/44.1/48kHz 原生域及 24kHz Realtime 域，验证 29.99/30.00/30.01 秒，以及上限样本数减一/等于/加一；恰好上限无空尾，超过上限只有精确余数。例如 44.1kHz 的 30.01 秒尾为 441 样本 |
| 分析等待越过硬上限 | 29.5 秒停、29.9 秒停、VAD 帧横跨上限、一次 push 跨多个上限；未满足停顿阈值时仍必须在上限样本数处精确切开，不等完整 VAD 帧 |
| stop 重复提交/漏尾 | 800ms 停顿前 stop、切点后立即 stop、30 秒硬切同时 stop、空输入、可听的一个样本尾、<100ms 短词；源样本全保留且只收尾一次，Realtime 补零与源样本分别计数 |
| 重采样漂移 | 各源采样率的分包/整包输出相同；24kHz flush 后累计长度等于原有 \(\lfloor n \times 24000 / r \rfloor\)（\(n\) 为源样本数，\(r\) 为源采样率），恰好 30 秒不能容忍“多出若干样本”的未知尾段 |
| VAD 失败丢音或串状态 | 初始化失败、处理中途错误、批量门控插在流式分析之间、新 SessionId；失败后切点退回固定，所有源样本保留，批量/流式状态互不污染 |
| 纯静音产生幻觉请求 | 初始纯静音、讲话后的长静音、静音跨多个上限：HTTP 无实际 STT 请求，Realtime 无 commit/response.create；允许已有 append/clear 生命周期，段表不能导致空尾请求 |
| 有序与重复口述 | 不同长度的 A、B、A 段，以及分三次重复同一短语；通过最终文本验证顺序及重复保留，错误/晚到结果不能覆盖 terminal 快照 |
| 满载、EOF 与未注册段 | 用可控阻塞而非长 sleep 的慢后端填满 Live 通道，再分别释放/stop；满后不接跨缺口帧，输入未 Drained 不得提前成功。校验当前段、排队 PCM、在途请求及结果都在计划上界内 |
| 超时后的后台泄漏 | 有已完成段、在途请求及多段待处理音频时耗尽收敛期限；返回快照后释放在途调用，确认不再启动下一段或继续交付迟到结果 |
| 取消与故障混淆 | 已完成一段，再在下段输入/响应/清理期间分别取消、故障；取消无交付/历史/记忆，故障只有已验证部分文字和警告。旧 SessionId 的故障通知、录音就绪及迟到结果不影响新录音 |
| UI 故障检查只依赖新音频 | worker 故障后不再送帧；通知仍唤醒 UI，取块为空也能停止并完成错误预览，不可卡住录音态 |
| 模式泄漏 | 两种 convert、两种 prompt-lab 回归、Realtime 场景及原始采集使用相同音频时，固定段区间/音频载荷/输出 WAV 与基线一致；保留原评分/报告身份 |
| 未确认便自动投递 | 停顿后继续录音、停录后最终文字到达、Esc、编辑后 Enter；未确认时目标输入与历史/记忆不变，确认后只交付/保存一次 |

概率注入只用于确定性策略验证，不作为“真实 VAD 准确”的证据；把帧概率与有编号/非对称 PCM 配对，期望切点从输入时长和规则独立计算。新测试应能检出固定分段未改、丢掉末帧、重叠预录、把帧当段编号或 finish 过早等实际错误。

#### 真实模型、语料与延迟

- 用已有 [3 秒 Silero fixture](../../tests/fixtures/audio/README.md) 拼接不同长短的数字静音，并加入确定性噪声和可听短尾，驱动真实捆绑模型与生产两条 Live 路径；它只能证明集成，不代表中文或本人麦克风效果。
- 使用获准的中文/中英混说及专有词语料，覆盖轻声、尾音、快语速、句内思考、风扇、背景人声、长连续口述、29–30 秒附近恢复讲话。人工标注“短停顿/可接受提交停顿”及音频时间，不将语义句号当作 VAD 已知标签；参考文字只用于本地评分。
- 记录四个独立时机：采集时间轴上的停声、实际处理/切点判定、网络请求开始或 commit、结果完成。分别报告检测/输入排队、请求排队、结果和 stop 后收敛延迟，不能混成一个“识别延迟”。
- 初始拟议门槛：确定性样本守恒、长度、顺序和尾部全通过；空闲模拟后端下，以 800ms 初值测得停声至实际提交 p95 ≤ 1.2 秒；≤600ms 已标注换气不误切；≥1.2 秒可接受停顿切段召回 ≥90%。这些门槛尚未由本轮语料证实，也不保证任意背景声音下达到。
- 同一模型、提示词、温度和音频顺序对照固定基线；本地 WER、专有名词及语义复核沿用现有定义，报告每样本新增删除/重复。拟要求微平均 WER 不恶化超过 1 个百分点、专名不恶化，且边界样本没有可归因于裁剪/重复的字词损失；小语料结论必须注明覆盖不足。
- 量测每 200ms 音频的 VAD/组段总耗时、积压追赶速度、峰值内存、API/commit 数和取消/心跳响应。初始 CPU 预算为每包 p99 ≤20ms、持续消费速度快于音频到达；参考机器、实际数据和慢后端结果在实现记录填写，不能用英文 fixture 或模拟快响应替代真实服务质量验收。

#### 消融与执行时机

1. 固定分段 vs 800ms vs 1200ms：同一音频、服务、提示词和顺序，比较停顿提交、短停顿误切、请求数量、stop 等待和识别质量，选择最保守且有可见收益的值。
2. 增量 Silero vs 仅能量+同等停顿：检查噪声、轻声、换气的差异；若简单方案达到同一质量/延迟门槛，再删去无需保留的复杂性，不能只比较无噪声输入。
3. 段间保留递归状态 vs 每包/每段重置（临时错误变体）：检出边界概率与误切退化；无收益也要说明是否仅为满足模型连续输入契约而保留。
4. 当前共享推进粒度 vs 直接按 200ms 包判断：比较包形状不变性和临界停顿，确认该少量缓冲状态是否必要。双阈值或回溯保护只有在前述结果明确失败时才提出追加方案，不为消融先把它们都做进产品。

上述对照在实现后、自动代码审查前执行，按结果精简后重跑相关检查。常规检查：`cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`；Windows 执行相应 `--features windows-app` 构建、测试和 Clippy。两平台真麦克风/预览交互分别记录；没有 Windows 实机证据时明确保留限制，不把 macOS 或交叉编译写成 Windows 运行通过。[项目验证约定](../../CONTRIBUTING.md#质量检查)。

### 开放问题、批准边界与暂缓事项

**需要随方案确认的行为选择：**

- “发送识别”仅为音频提交，停顿不会自动停录、自动结束预览或向目标应用输入。
- 先仅改变两条桌面听写路径；离线/评测和原始采集维持固定分段，在线分段效果由专项回放验证。
- 接受 HTTP Live 新增有界短帧积压及首次溢出停录/不完整预览的行为；不接受无界缓存、自动丢帧或扩大 30 秒。

**实现期校准，不要求用户先给精确数字：** 800ms 与 1200ms 的取舍、0.3 阈值的噪声/轻声表现、20ms 推进是否有实际必要、两平台 CPU/追赶预算和现有 30 秒收敛期限下可承受的后端速度。若上述门槛不能同时满足，记录证据并收缩/修订方案，不能声称 VAD 已理解句意或擅自延长段长。

**暂缓：** 语义/语调端点、服务端自动 turn detection、噪声自适应、临近上限另设阈值、预录音重叠、并行请求、桌面 HTTP 增量文字、在线分段专用的持久 prompt-lab 模式，以及用户可调的策略配置。

### 本轮调查与验证状态

本轮已核读两条桌面路径、原始采集、离线/评测复用点及工作流材料，并再次 fetch 确认 v0.5.3 基线未变。本地 Claude Code 顾问使用 `claude-opus-5-5` 生成方案草案；初始化及响应模型均核对为该模型，tools/mcp_servers 为空，进程退出 0、最终结果无错误。本文保留单 worker、有界短帧、共享切点和离线模式隔离等建议，删去尚无证据支持的近上限短停顿、最低有声时长、空闲轮转和回溯 guard，并用实读源码补齐采集及精确尾长度的边界。顾问建议不等于运行验证。

本次只新增本 feature record 和文档索引，未修改产品代码、测试、配置、个人记忆或数据集；不运行/宣称产品功能、音频质量或跨平台验收已通过。计划批准前不开始实现。

## 2. PR 变更时间线

### 1. 2026-11-10 — 共享声学端点与精确音频边界

**触发与范围：** 实现上述用户要求，同时保留整段人声门控和离线/原始采集契约。

**已实现：** `audio/vad.rs` 拆出共享单帧推理，模型仍只加载一份；批量过滤每声道重置状态，`StreamingVad` 每录音拥有独立重采样、递归状态、上下文与不足 512 样本的尾部。锁只围绕推理，不覆盖重采样、组段或网络。`PcmResampler::new(input_rate, output_rate)` 支持分析用 16kHz，旧 Realtime 调用显式保持 24kHz。

新增 `audio/segment.rs` 实现 `Segmentation`、`CutReason`、`Cut` 与 `Segmenter`。采用 0.3 人声阈值、连续 25 个 32ms 低概率帧（800ms）、约 20ms 原生样本推进。段内先见人声再计停顿；恢复人声清零静音计数。切段保留 VAD/滤波连续状态，只重置段计数；初始化/分析失败后本次录音退回固定上限。硬容量优先，stop 非空尾只关闭一次；不裁剪、不重叠、不按文字去重。

Realtime 在 append 前求切点，按连续切片逐段提交；30 秒容量改用已有 `max_frames_per_chunk`，删除独立的 `SEGMENT_SAMPLES`。桌面使用 PauseAware，`offline` 和 `for_evaluation` 显式 Fixed，`turn_detection:null` 及前缀、清理、补零、学习确认不变。

**取舍与状态：** 已实现最小单阈值方案；没有增加双阈值、最低有声时长、回溯、服务端 VAD 或公共配置。阈值是工程初值，真实语料准确率未验收。

**验证：** 扩展现有重采样分包/整包/尾长度测试；确定性概率测试覆盖换气、恢复讲话、包形状、四种采样率的 30 秒 ±1 样本及 29.99/30/30.01 秒、多上限、重复 stop 和推理降级。真实模型测试覆盖英文 fixture 的 600ms/2s 停顿及 16/44.1/48kHz 的分包不变性；交错批量 VAD 不污染流式状态。本地 WebSocket 协议测试检查 committed PCM 拼接与源 PCM 逐样本一致，停录前已经得到第一段，Fixed 负对照只产生一段，重复口述保留；30 秒纯静音和单样本静音尾都 clear 而非请求文字。

来源：[切点实现与回归](../../src/audio/segment.rs)、[VAD](../../src/audio/vad.rs)、[Realtime 协议测试](../../src/dictation/engine/tests.rs)、[真实模型回归日志](../../.amp/in/artifacts/endpoint-model-regression.log)、[静音边界日志](../../.amp/in/artifacts/realtime-silence-boundary.log)。

### 2. 2026-11-10 — HTTP 短帧、排空与输入完整性

**背景与方案比较：** 普通 HTTP 原先只接完整块。保持单 worker 可以避免分段线程及两级音频队列；请求繁忙时接受延迟检测，输入有界等待。实现该方案，不增加并发请求。

**已实现：** 普通桌面 HTTP 与 Realtime 都启用现有 200ms recorder 帧和 60 秒 PCM 容量，raw capture 保持 Fixed。HTTP Live 队列容量 300，worker 校验每帧最多 200ms 及稳定格式，解码、VAD、组段和 STT 在同一串行 worker 上执行。`HttpAudio` 只保留一个最长 30 秒的开放段，实际封段后才发 `SegmentQueued` 分配段号；`InputDrained` 证明 EOF/尾段完成，不能以“当前没有注册段”提前成功。Fixed 完整块入口和容量 2 保留。

**相对初始方案的简化决定（已采用）：** 不新增重复的 `InputFailed` event 和 session `failure` 字段。用一份共享 `HttpInputStatus { failure, notify }` 保存首个完整性故障并唤醒 UI；`recording_error` 与最终收集读取同一来源。输入断开后不再接受跨缺口帧，避免两份错误状态漂移。

队列满时已接受的连续前缀继续排空；解码/格式失败时停止消费，后续已排队音频也不能跨过缺口。有效前缀尾段仍尝试提交。`InputFailure { error, partial_text }` 显示“录音不完整”，无文字时仅错误气泡，有文字时可核对后确认；没有学习确认能力。单段 STT 失败允许后续段继续，仍走原 PartialFailure。UI 取帧前即检查故障，worker 通知能在没有新音频时触发停录。

收敛 deadline 仍为停录后 30 秒，覆盖排队与未注册尾部；超时撤销后续请求，未排空输入至少算一个待完成工作项。正常 stop 关闭发送端并处理尾段，容量失败不能伪报完整成功。模型、网络重试和音量预处理的原行为不变。

**验证：** 可控通道阻塞网络测试检查提前请求、EOF 未注册尾部、满 300 帧后拒绝后续输入、连续样本完整拼接、无新音频的故障唤醒、故障前缀及单段失败后继续。真实 `RecognitionConfig` + `ApiTranscriber` 组装的 31 秒纯静音测试确认本地监听端没有 HTTP 连接；一个非零样本尾用 mock 确认不被分段层丢弃（不声称该样本一定通过原能量门控）。原始采集在 Realtime 配置开/关时均走 Fixed。

来源：[HTTP worker 与契约测试](../../src/core/orchestrator.rs)、[入口选择](../../src/core/recognition.rs)、[UI 故障检查与警告](../../src/ui/listener/event_loop.rs)、[本地全量检查](../../.amp/in/artifacts/validation-macos.log)。

### 3. 2026-11-10 — 停录后 Esc 取消仍能撤销 HTTP 后续请求

**发现：** 停录会将 HTTP session 从 orchestrator 脱离。此前预览取消只撤销投递，无法到达 detached worker，积压的下一段仍会开始识别。新增 `cancel_after_stop_reaches_the_detached_http_worker` 在未修复时实际失败，并能区分旧/新 SessionId。

**已实现决定：** 复用已有 `Progress.cancelled: CancellationToken`，HTTP 和 Realtime 的桌面 Delivery 都创建 Progress；HTTP 只使用取消，不发布增量文字。HTTP 不再另存 AtomicBool。Esc、超时和 abort 都能阻止后续段调用，在途同步 API 及其原重试仍不能撤回。旧录音的迟到结果不影响新录音；文字、历史和记忆继续受原预览确认门禁约束。

**结果：** 同一回归在修复后通过；主动取消/零 deadline 场景在释放阻塞请求后均没有第二次请求。错误警告不能获得学习能力，预览相关原测试继续通过。

来源：[失败证据](../../.amp/in/cancel-red.log)、[修复后证据](../../.amp/in/cancel-green.log)、[完整回归](../../.amp/in/artifacts/validation-macos.log)。

### 4. 2026-11-10 — 声学策略与推进粒度消融

**已执行设置：** macOS arm64、本仓库 debug 构建、捆绑 Silero 英文 3 秒 fixture，先重采样为 16/44.1/48kHz；相同源音频依次拼接 200/400/600/1200/2000ms 数字静音及第二遍 fixture。另用 `(((i * 7919 % 101) - 50) * 30)` 的 i16 确定性噪声替换 2 秒静音，并将“fixture + 2 秒静音”重复八次形成 40 秒输入。没有网络、人工参考文字或个人音频。

临时探针逐包运行相同 `Segmenter`，比较 Fixed、生产增量 Silero、用分析帧均方能量 >0.00001 代替 VAD 的变体、每包重建分析器、在含切点的包之后重建分析器（近似每段重置）、以传输包大小代替 20ms 推进。每次只替换一个机制；800ms 与 1216ms 两轮仅替换 `PAUSE_FRAMES` 的 25/38。执行命令：`cargo test --locked --lib endpoint_ablation_probe -- --ignored --nocapture --test-threads=1`。临时探针和 include 已删除，正式参数恢复 25；其输入生成方式及替换规则如上，常规测试保留模型/包形状回归。

| 对照 | 已观察结果与决定 |
|---|---|
| Fixed vs 800ms | Fixed 在这些短录音中都等 stop；800ms 对 200/400/600ms 停顿不切，对 1200/2000ms 的第一切点为 3820ms（16kHz）或 3840ms（44.1/48kHz）。满足提前提交的本地集成目标，不能当作中文准确率。 |
| 800ms vs 1216ms | 后者第一切点为 4240/4260ms，已超过 3s + 1.2s 停顿；前者在该停顿内形成切点，且本组短停顿未误切，故暂保留 800ms。没有证据保证所有讲话/轻声都适用。 |
| Silero vs 能量 | 在上述确定性噪声间隔，能量变体没有提前切点，Silero 仍切；简单能量方案未满足同组需求，不替换现有模型。噪声合成样本不能代表所有风扇/背景人声。 |
| 20ms vs 包大小推进 | 48kHz 下生产策略对 317 样本与 200ms 包都在 3840ms 切；包大小推进分别为 3830/4000ms。保留少量跨包步长残余，避免任意包形状改变边界。 |
| 保留状态 vs 重置 | 每包重置在 44.1/48kHz 的 2s 停顿切点延后到 3980ms；近似段重置在后续 40s 切点有约 20ms 差异，但没有识别质量结论。保留跨段状态符合连续模型输入契约，也避免新增重置逻辑。 |

**耗时观测：** 生产增量变体的 200ms 包 p99 最大观测 6.832ms；40s 的 44.1/48kHz 输入分别约 1057/993ms 处理完。只量了此机器上分段/重采样调用耗时，未覆盖网络、UI 事件延迟、进程峰值内存、心跳或 Windows；不能据此宣称整体延迟或全部性能门槛通过。

**仍未验收：** 中文/混说、轻声/尾音、真实噪声、词跨 30 秒的模型质量；真人标注的停顿召回及提交延迟 p95；真实服务 WER/专名/费用、进程峰值及慢服务持续吞吐；Windows 实机。未调用个人配置的识别 API，未采集麦克风。发布前需要这些证据，不用固定分段 prompt-lab 历史成绩替代。

来源：[800ms 原始日志](../../.amp/in/artifacts/segment-ablation-800.log)、[1216ms 原始日志](../../.amp/in/artifacts/segment-ablation-1216.log)。

### 5. 2026-11-10 — 本地组合验证与交付限制

**已执行并通过：** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、`cargo clippy --locked --all-targets -- -D warnings`；恢复临时探针/fixture 后，另执行 `cargo test --locked --features integration` 与 `cargo clippy --locked --features integration --all-targets -- -D warnings`。原有忽略项未启用，vendored rdev 的既有依赖警告仍在，不修改其代码或压制警告。[默认检查日志](../../.amp/in/artifacts/validation-macos.log)、[integration 检查日志](../../.amp/in/artifacts/validation-integration.log)。

**原生 UI：** 临时将现有 `preview_demo` 的两种失败输入替换为 InputFailure 的空文本/部分文本，仍经生产 `finish_transcription` 及确认等待器；渲染 fixture 不读取配置、不录音、不联网、不捕获目标、不注入或写历史。执行 `cargo run --locked --features integration --bin viberwhisper-preview -- --artifacts .amp/in/artifacts/incomplete-preview` 退出 0，原生错误态检查拒绝 Enter 并用 Esc 关闭。正常预览、带部分文字的不完整警告及空文字错误气泡均已逐张查看，无裁切；部分文字截图的“无法获取输入目标”来自这个无目标 fixture。临时 demo 改动已全部恢复，最终 diff 不含该文件；截图只证明呈现，取消/样本守恒由上述执行测试证明。

[正常预览](../../.amp/in/artifacts/incomplete-preview/ready.png)、[部分结果](../../.amp/in/artifacts/incomplete-preview/incomplete-partial.png)、[空结果错误](../../.amp/in/artifacts/incomplete-preview/failed.png)、[原生检查日志](../../.amp/in/artifacts/native-preview.log)。

**Windows 范围：** 常规 `cargo check --locked --target x86_64-pc-windows-gnu --features windows-app --all-targets` 未通过：ort-sys rc.9 不提供该目标的预编译 ONNX Runtime，未到产品类型检查阶段。核读其 build.rs 后仅用 `DOCS_RS=1` 跳过 ORT 链接准备，Windows all-targets check 与同目标 Clippy `-D warnings` 均通过；这是 Rust 类型/静态检查，不是可链接 exe、原生测试或打包验证。随后清理本次 ort-sys Windows 检查产物，避免复用跳过链接的构建缓存。不改依赖、不下载另一套运行时来掩盖限制。

[常规交叉检查阻断](../../.amp/in/artifacts/validation-windows-cross.log)、[Windows 类型检查](../../.amp/in/artifacts/validation-windows-types.log)、[Windows Clippy 类型检查](../../.amp/in/artifacts/validation-windows-clippy-types.log)。

**交付状态：** 仅独立 JJ workspace 本地修改，文档与 changelog 已同步；未设置交付 bookmark、推送、开 PR、合并、发版或部署，没有改主工作区/其他 feature。未执行发布前的自动代码审阅门禁，发布前仍须在同一 feature PR 跑门禁并完成第 4 条列出的质量/实机验收；本地通过不代表已可发布。

### 6. 2026-11-10 — 与最新诊断日志基线组合验证

**背景与已完成集成：** 送审时 `master@origin` 已包含诊断日志功能，故将本 feature 变基到上述 7aa37aef。只有文档索引的并行追加冲突，保留诊断日志及停顿分段两个入口；产品代码自动合并，未改变既有诊断日志、依赖或工作流。初始方案的原 v0.5.3 调查仍保留，不将变基后的事实改写进初始结论。

**已执行结果：** 新基线上 `cargo fmt --check`、`cargo build --locked`、默认测试、`--features diagnostic-logs` 测试、`--features integration` 测试及 `cargo clippy --locked --all-targets --all-features -- -D warnings` 均通过。[送审前组合检查日志](../../.amp/in/artifacts/validation-before-pr.log)。第 4、5 条的真实语料、服务及 Windows 运行限制仍然有效，Draft PR 不代表已满足发布验收。

### 7. 2026-11-10 — 修复 stable CI 的测试解码 lint

**触发与已核实原因：** [PR #173](https://github.com/b1indsight/viberwhisper/pull/173) 首轮 [macOS CI](https://github.com/b1indsight/viberwhisper/actions/runs/42806243198/job/129795986737) 和 [Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42806243198/job/129795986358) 均在新增 Realtime 样本连续性测试的 `chunks_exact(2)` 处触发 `clippy::chunks_exact_to_as_chunks`；前置构建和测试步骤通过。CI 使用 Rust 1.99.1，前述本地检查使用 1.94。额外安装独立的 1.99.1 工具链后，在修改前用 CI 的 diagnostic-logs Clippy 命令复现相同失败，没有改变默认工具链或工作流。

**已修复与边界：** 测试 PCM 解码改用 `as_chunks::<2>().0.iter()`，直接读取二字节数组；源音频拼接、字节序及逐样本断言不变。不增加 lint 豁免、依赖或运行机制，不修改产品分段策略。本次为测试写法兼容修复，无新增机制需要消融。

**验证与状态：** Rust 1.99.1 的 `cargo fmt --check`、CI 原命令 `cargo clippy --locked --all-targets --features diagnostic-logs -- -D warnings`、all-targets/all-features Clippy 和 diagnostic-logs 全量测试均通过；原 1.94 工具链上的该 Realtime 样本连续性回归也通过。Windows 新提交的 CI 结果仍待确认，以上本地证据不替代 Windows 运行验收；第 4 条真实语料与服务验收限制不变，PR 继续保留 Draft。

来源：[同版本复现](../../.amp/in/artifacts/ci-clippy-1.99.1-red.log)、[修复后验证](../../.amp/in/artifacts/ci-clippy-1.99.1-green.log)。
