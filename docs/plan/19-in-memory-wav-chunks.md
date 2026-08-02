# 内存 WAV Chunk 流水线

## 状态

**已按批准计划实现，待最终 review。**

本文档修订 `05-long-audio-streaming.md` 和
`06-end-to-end-stream-recognition.md` 中“chunk 以临时 WAV 文件路径传递”的既有设计。

## 依赖

本实现的实时内存上限以 PR #83 的 STT 请求约束为前提：单次请求 5 秒、最多三次重试，
完整请求与退避窗口约 27 秒。经用户批准，实现 change 直接叠在 #83 的 head 上，未复制
其代码，也未在本 PR 中合并该依赖。

## 背景

当前系统已经有两条 chunk 生产路径：

1. 实时录音把单声道 `i16` PCM 累积到内存，达到阈值后写成临时 WAV。
2. 离线转换用 `WavReader` 顺序读取本地 WAV，但先把所有分片写成 `TmpChunk` 文件。

两条路径随后都把文件路径交给 `ApiTranscriber`。转写器重新读取文件，把完整 WAV
字节放进 multipart `file` part，再发给 OpenAI-compatible STT endpoint。远端只接收 WAV
字节，不使用本地路径。

这形成了不必要的中间过程：

```text
PCM / source WAV
  -> encode temporary WAV file
  -> enqueue path
  -> read the same file into memory
  -> multipart upload
  -> delete temporary file
```

实时录音默认每 30 秒产生一片，单次 STT 请求固定为 5 秒，最多三次重试时完整请求与
退避窗口约为 27 秒。正常约束下 STT 消费速度不应持续落后于录音速度，因此不需要用
磁盘作为 backlog spill。离线输入也可以逐片读取、逐片转写，无需先物化全部临时文件。

此外，chunk 限额到样本数的换算目前分别存在于 `recorder.rs` 和 `splitter.rs`：两处都
减去 44 字节 WAV header，再对 duration limit 和 size limit 取最小值，但无效 size 的
错误语义并不一致。

## 结论与边界

本计划把 chunk 定义为：

> 内存中一段完整、可独立解码和转写的 WAV payload。

文件输入和录音输入只是 `WavChunk` 的两种流式生产方式。转写器只消费一个已经构造
完成的 `WavChunk`，不再负责文件切片、路径读取或多片文本合并。

```text
Recording PCM -- AudioRecorder -----+
                                    +--> WavChunk --> Transcriber --> Result<String>
Local WAV ----- WavChunkReader -----+
```

## 目标

1. 定义唯一的内存 `WavChunk` payload，保证其内容是一段完整 WAV。
2. 将 chunk 限额换算收口到 `audio::max_frames_per_chunk` 模块级函数。
3. 实时录音达到阈值时直接编码内存 WAV，不创建 session 临时目录或 chunk 文件。
4. 离线 WAV 逐片读取、逐片生成内存 WAV、逐片转写，内存上限保持为一个 chunk。
5. `max_size_bytes` 换算出的容量小于等于 0.5 秒时不产生 chunk，避免发送无意义请求。
6. 将 `Transcriber` 收窄为单 chunk 转写接口，输入 `WavChunk`，输出现有
   `Result<String, TranscribeError>`。
7. 保持 session 状态、顺序合并、partial failure、timeout 和 retry 的用户可见语义。
8. 删除只为中间 chunk 文件服务的写盘、路径移交和清理逻辑。

## 非目标

- 不做静音检测、语义切句或 token 级流式 ASR。
- 不改变 Hold、Toggle、tray 或 hotkey 的状态机语义。
- 不增加并行 STT worker，也不引入 async runtime。
- 不实现进程崩溃后的 chunk 恢复；重启后不会继续旧 session。
- 不改变 WAV 之外的输入格式支持。
- 不改变语言感知文本合并规则。
- 不在本次重构中迁移持久化配置字段或重命名 `chunking.max_retries`。
- 不把 session id、上传状态、转写文本或 retry policy 塞入 `WavChunk`。

## 核心类型

### `WavChunk`

位置：`src/audio/chunk.rs`，由 `src/audio/mod.rs` re-export。

```rust
#[derive(Clone)]
pub struct WavChunk {
    wav_bytes: Arc<[u8]>,
}

impl WavChunk {
    pub(crate) fn bytes(&self) -> &[u8];
    pub(crate) fn len(&self) -> usize;
}
```

设计约束：

- `WavChunk` 只持有编码完成的 WAV bytes。
- 不保存路径；multipart filename 固定使用 `audio.wav`。
- 不保存 index。顺序是 producer/orchestrator 的流元数据，不是音频 payload 的属性。
- 不保存 `SessionId`。同一个 payload 类型同时服务录音和离线输入。
- 使用 `Arc<[u8]>` 让 channel 传递、事件派发和 retry 复用同一份只读内容。
- 构造函数保持 crate 内可见，避免任意字节绕过 WAV 编码边界。

multipart retry 为每次请求创建新的 `Cursor<Arc<[u8]>>`，再使用
`Part::reader_with_length`。每次 retry 只重建 request body reader，不复制完整 payload。

### 模块级 chunk 容量计算

位置：`src/audio/chunk.rs`，由 `src/audio/mod.rs` re-export。

不保留 `ChunkLimits` value object。duration/size 是实际消费者配置中的两个标量，唯一的
换算入口是模块级纯函数：

```rust
pub(crate) fn max_frames_per_chunk(
    max_duration_secs: u32,
    max_size_bytes: u64,
    output_spec: WavSpec,
) -> Result<Option<u64>, ChunkError>;
```

语义：

- duration limit 按 `duration * output_spec.sample_rate` 换算为完整 frame 数。
- size limit 先从 `output_spec` 计算
  `bytes_per_frame = channels * ceil(bits_per_sample / 8)`，再按
  `(max_size_bytes - encoded_header_bytes) / bytes_per_frame` 换算。
- `encoded_header_bytes` 必须与实际输出 `WavSpec` 一致；不能对多声道或高位深 WAV
  一律假设为 44 字节。
- 非零 size 连一个 header 加完整 frame 都容不下，或者按 size 最多容纳的 frames 时长
  小于等于 0.5 秒时，返回 `Ok(Some(0))`。
- 其余情况下，两个有效限额取最小值。
- 两个限额都为 `0` 时返回 `Ok(None)`，表示不主动切片。
- 格式参数为零或算术溢出时仍返回结构化错误。
- 返回值始终以完整 frame 为单位，producer 不会在多声道 frame 中间切片。
- recorder 固定传 mono、16-bit 输出格式；文件 reader 使用它实际准备编码的输出格式。

`Ok(Some(0))` 不表示构造零长度 `WavChunk`：recorder 的 ready/stop 路径不返回 chunk，
`WavChunkReader` 的 iterator 直接结束，transcriber 不收到请求。它只处理
`max_size_bytes` 无法容纳超过 0.5 秒音频的情况，不改变正常 duration 切片、队列容量或
最终 tail 的既有语义。`Ok(None)` 仍只表示两个限额都关闭。

这会统一当前 recorder 静默把过小 size 当作禁用、splitter 却返回错误的差异。

配置投影保持扁平：

```rust
struct AudioConfig {
    mic_gain: f32,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
}

struct ConvertConfig {
    backend: BackendConfig,
    language: Option<String>,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
}
```

`runtime_config` 直接从 `ChunkingSection` 把两个字段复制到对应 workflow config；不创建
只负责搬运数据的中间对象。`max_retries` 仍单独进入 `TranscriberConfig`。

## 两种 producer

两条路径统一产出 `WavChunk`，但不抽象 `ChunkProducer` trait：实时录音是事件驱动的
push 模型，本地文件是按需推进的 pull iterator，强行统一调用接口只会引入适配层。

模块职责保持精简：

```text
audio/chunk.rs      WavChunk、ChunkError、内存 WAV 编码、容量换算
audio/recorder.rs   AudioRecorder，实时产生 WavChunk
audio/wav_file.rs   WavChunkReader，通过 iterator 产生 WavChunk
```

### 1. 实时录音 producer

保留音频 callback 的实时约束：callback 只负责下混、增益、追加 PCM 和更新完整 chunk
计数，不执行 WAV 编码、不等待 channel，也不执行网络调用。

主事件循环调用 `take_ready_chunk` 时：

1. 从共享 PCM buffer 复制一个完整 chunk 的样本。
2. 在内存 `Cursor<Vec<u8>>` 上用 `WavWriter` 编码单声道 16-bit WAV。
3. 编码成功后从 recorder buffer drain 对应样本。
4. 返回 `ReadyChunk { session_id, chunk: WavChunk }`。
5. 停止录音时，同样把完整剩余片和最终 tail 编码成 `WavChunk`。

`RecorderStopOutcome::Stopped` 的 payload 从 `Vec<String>` 改成
`Vec<WavChunk>`。短录音仍产生一个 chunk，完整边界片和尾片顺序保持不变。

实时 worker 输入 channel 继续有界，但容量从当前 64 收紧为常量 2：一个正常周期内只会
出现当前 in-flight chunk 和停止时紧随其后的 tail。queue full 继续成为显式 chunk failure，
不回退到磁盘 spill，也不静默丢弃。

### 2. 本地 WAV producer

在 `audio/wav_file.rs` 中用 `WavChunkReader` 取代
`split_wav -> Vec<TmpChunk>`：

```rust
pub struct WavChunkReader {
    // owns WavReader and per-stream counters
}

impl WavChunkReader {
    pub fn open(
        path: &Path,
        max_chunk_duration_secs: u32,
        max_chunk_size_bytes: u64,
    ) -> Result<Self, ChunkError>;
    pub fn chunks(&mut self) -> WavChunks<'_>;
    pub fn total_chunks(&self) -> usize;

    fn read_next_chunk(&mut self) -> Result<Option<WavChunk>, ChunkError>;
}

pub struct WavChunks<'a> {
    reader: &'a mut WavChunkReader,
    finished: bool,
}

impl<'a> Iterator for WavChunks<'a> {
    type Item = Result<WavChunk, ChunkError>;
}
```

行为：

1. `open` 读取 WAV header/spec，调用共享 `max_frames_per_chunk` 计算单片 frame 数。
2. 根据 `reader.len()` 预先计算总片数，仅用于进度日志；不再设置任意的 chunk 数量上限。
3. `chunks()` 返回借用 reader 的迭代器；每次 `Iterator::next` 最多读取一个 chunk 的
   samples，并直接写入内存 `WavWriter`。单片读取细节由私有 `read_next_chunk` 承担。
4. 调用方转写并释放当前 chunk 后才推进迭代器，自然形成 pull-based backpressure。
5. 当两个 limits 都禁用时，整个源 WAV 作为一个 chunk 流式读入内存；不再用“空 Vec
   表示无需切片”的特殊协议。
6. iterator 在 EOF 或第一次读取错误后进入 `finished`，不会在返回错误后继续读取不确定
   状态的输入。

`WavChunkReader` 保留源 WAV 的 channels、sample rate、bits per sample 和 sample format。
任何大小的源文件都只需要一个 chunk 大小的工作内存，不创建中间文件。

## Transcriber 边界

trait 改为只接受单个 chunk：

```rust
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError>;
}
```

`ApiTranscriber` 的职责收窄为：

1. 从 `WavChunk` 创建 multipart WAV part。
2. 添加 model、temperature、language、prompt 和 authentication。
3. 发送单次请求并解析 `{ "text": ... }`。
4. 对 network/5xx 执行现有 retry；4xx 立即返回。

以下职责移出 `ApiTranscriber`：

- `split_wav` 调用。
- chunk duration/size 字段和容量换算。
- 文件路径读取与 filename 派生。
- 多 chunk 循环和 `merge_texts`。

`TranscriberConfig` 继续持有 endpoint、auth、model、transcription options 和
`max_retries`；不再持有 chunk duration/size 字段。

## 调度与结果合并

### 实时 session

`SessionEvent::ChunkReady`、`SessionEffect::SubmitChunk` 和 `WorkerMsg::Chunk` 将 `path`
替换为 `WavChunk`。orchestrator 保持现有行为：

- `on_chunk_ready` 按接收顺序分配 index。
- `ChunkEntry` 继续只保存 index 和 `ChunkState`，不长期保存音频 payload。
- worker 在调用 `transcribe` 期间拥有 queued `WavChunk`。
- 完成、失败、timeout 和 abort 后通过 Rust 所有权自然释放 payload。
- `ChunkState`、`WorkerEvent`、partial text 和有序合并语义不变。

删除 orphan、stale、queue rejection、worker completion 和 cancellation 路径中的临时
文件删除操作；这些路径只需 drop 尚未接管或已经接管的 `WavChunk`。

### 离线 convert

`handle_convert` 负责消费 `WavChunkReader`：

```text
for chunk in reader.chunks()
  -> chunk?
  -> transcriber.transcribe(&chunk)
  -> collect text in iterator order
merge_texts(texts, language)
  -> post-process
  -> output
```

为避免把切片和合并重新塞回 transcriber，`runtime_config::resolve_convert` 返回的
`ConvertConfig` 直接向 convert 暴露 duration、size 和 language。listener 的
`AudioConfig` 直接持有相同的两个配置标量，`OrchestratorConfig` 继续持有 language。

## 文件改动

| 文件 | 计划变更 |
|---|---|
| `src/audio/mod.rs` | 声明并 re-export `chunk`、`recorder` 和 `wav_file` 的公共音频接口 |
| `src/audio/chunk.rs` | 定义 `WavChunk`、`ChunkError`、内存 WAV 编码和模块级 `max_frames_per_chunk` |
| `src/audio/recorder.rs` | 内存编码实时 chunk；移除 session 临时文件、写盘和路径所有权逻辑 |
| `src/audio/wav_file.rs` | 新增 `WavChunkReader::chunks()` pull iterator，接替离线文件 producer 职责 |
| `src/audio/splitter.rs` | 删除；`TmpChunk`、临时输出文件和旧 `split_wav` 接口不再保留 |
| `src/transcriber/api.rs` | trait 接受 `&WavChunk`；multipart 直接读取内存；移除切片/合并职责 |
| `src/core/orchestrator.rs` | worker channel 改为传递 `WavChunk`；移除文件清理分支 |
| `src/core/recording_session.rs` | event/effect payload 从路径改为 `WavChunk` |
| `src/runtime_config.rs` | `AudioConfig`/`ConvertConfig` 直接携带 duration/size；transcriber 不再携带这些字段 |
| `src/main.rs` | 适配实时 event 和离线 pull loop |
| `docs/architecture/audio.md` | 描述两个内存 producer 与有界内存模型 |
| `docs/architecture/transcriber.md` | 描述单 chunk 输入契约和职责收窄 |
| `docs/architecture/core.md` | 更新 orchestrator worker payload 与清理语义 |
| `docs/plan/05-long-audio-streaming.md` | 标记文件型 chunk 设计被本计划后续修订 |
| `docs/plan/06-end-to-end-stream-recognition.md` | 更新 worker 输入但保留 session-owned 结果模型 |
| `docs/plan/16-session-owned-chunk-results.md` | 记录文件清理验收项被内存所有权替代 |
| `changelog` | 记录实时和离线 chunk 改为内存流式传递 |

不新增外部 crate；编码继续使用 `hound`，共享 payload 使用标准库 `Arc<[u8]>`。

## TDD 实施顺序

计划批准后，所有阶段先写最小失败测试，再实现使其通过。同一 feature 继续使用本计划的
bookmark 和 PR。

### Phase 1：共享 chunk 与限额换算

先增加代表性测试：

- duration/size 同时存在时返回较小的样本限额，并正确考虑 channels。
- 两个限额都禁用时返回 `None`。
- 非零 size 不大于 WAV header 时返回 `Some(0)`。
- size 换算容量小于等于 0.5 秒时返回 `Some(0)`，超过 0.5 秒时返回对应 frame 数。
- producer 收到 `Some(0)` 后不构造 `WavChunk`，也不调用 transcriber。
- 最终 tail 可以短于 0.5 秒，并仍编码为合法 `WavChunk`。
- recorder 使用的 mono i16 样本可编码成能被 `WavReader` 重新打开的 `WavChunk`。

然后实现 `WavChunk`、内存 WAV encoder 和模块级 frame limit 换算，删除
`ChunkLimits`、`ChunkLimits::from_section` 以及 recorder/splitter 的重复公式。

### Phase 2：单 chunk transcriber

先改 mock HTTP 测试，使其直接构造内存 `WavChunk`，验证：

- multipart 收到完整且可解码的 WAV bytes。
- 4xx 不 retry，5xx/network 保持既有 retry 次数。
- retry 使用同一 payload，调用方不依赖文件继续存在。

然后修改 trait 和 `ApiTranscriber`，移除文件读取、内部 splitter 与内部文本合并。

### Phase 3：离线流式 reader

用小型合成 WAV 增加测试：

- 短输入只产出一个 chunk。
- 长输入逐片产出，所有 chunk 样本总数等于源文件且 spec 保持一致。
- 创建 `chunks()` iterator 不会预先物化 chunk；每次 `next()` 只产生一片。
- 超过 100 片的输入仍逐片读取到 EOF，不因片数被拒绝。
- 转写结果按读取顺序使用现有 `merge_texts` 合并。

然后在 `audio/wav_file.rs` 实现 `WavChunkReader`，删除 `audio/splitter.rs`，并迁移
`handle_convert`。

### Phase 4：实时录音与 orchestrator

先迁移现有 recorder/orchestrator 测试并增加最小行为覆盖：

- 达到阈值时返回内存 WAV，buffer 对应样本被释放。
- stop-time 完整片和 tail 按原顺序产生。
- worker 接收内存 chunk 并返回相同的状态转换与有序结果。
- queue full、stale session、abort 和 timeout 会释放 payload 并保持现有失败语义。
- 实时路径不会创建 chunk 文件或 session 临时目录。

然后替换 path-based event/effect/worker payload，并将实时队列容量改为 2。

### Phase 5：删除文件型 chunk 逻辑并更新文档

删除：

- `TmpChunk` 和 Drop 清理。
- recorder 的 `current_session_files`、`session_dir`、`write_chunk`、
  `write_full_recording`、`relinquish_path`。
- orchestrator 的 `remove_chunk_file` 以及所有路径清理测试。
- 只为 chunk 临时文件提供的唯一命名和空 session 目录清理逻辑；若仍有其他调用者则保留
  最小公共部分。

最后更新架构文档、既有 plan 的后续修订说明和 changelog。

## 验收标准

- [x] `WavChunk` 是实时和离线转写唯一的单片输入类型。
- [x] `Transcriber` 不接受文件路径，也不执行切片或多片合并。
- [x] chunk 容量换算只有模块级 `audio::max_frames_per_chunk` 一份实现。
- [x] crate 中不再存在 `ChunkLimits` 或其他只搬运 duration/size 的中间对象。
- [x] `max_size_bytes` 最多容纳 0.5 秒音频时返回 `Some(0)`，producer 不输出 chunk 或发送请求。
- [x] 实时录音不会为转写创建临时 WAV 文件或 session 临时目录。
- [x] 离线长 WAV 不创建 `TmpChunk` 文件，并且一次只持有当前内存 chunk。
- [x] `WavChunkReader` 不设置 chunk 数量上限，并持续读取到 EOF。
- [x] 实时 queue 最多缓存两个等待处理的 payload；满队列显式失败。
- [x] retry 复用同一份 chunk bytes，不重新读文件或复制完整 payload。
- [x] recorder callback 不做 WAV 编码、channel 等待或网络 I/O。
- [x] session 顺序、partial failure、timeout、abort 和文本合并语义保持不变。
- [x] 现有 API/local multipart endpoint 请求格式保持兼容。
- [x] 不引入新的运行时依赖或持久化配置迁移。
- [x] 实现遵循 TDD；测试数量与行为风险成比例，不为简单访问器添加重复测试。
- [x] `cargo fmt --check`、`cargo check`、`cargo test` 全部通过。

## 风险与控制

### 内存上限

默认 44.1 kHz、mono、16-bit、30 秒 chunk 约 2.6 MB。实时 channel 容量 2，加上一个
in-flight chunk 和 recorder 未封片 tail，正常 payload 内存保持在十几 MB 以内。非默认
配置仍受 `max_size_bytes` 限制；queue 不按文件路径无限堆积。

### 同步编码延迟

WAV 编码继续发生在 `take_ready_chunk`/stop 路径，而不是 CPAL callback。编码目标从磁盘
换成内存，减少系统调用；若测试显示主事件循环延迟不可接受，再单独评估 encoder worker，
本计划不预先增加线程。

### retry 生命周期

worker 在 retry 完成前持有 `WavChunk`，每次请求使用新的共享字节 reader。timeout 或 abort
不会强制取消已经进入 blocking HTTP 调用的 worker，这一既有约束不在本计划中改变；调用
结束后 payload 自动释放。

### 离线错误时机

总片数由 WAV header 中的 sample count 和共享限额在 reader 初始化时计算，但只用于进度
日志。内存与磁盘占用都不随总片数累积，因此 reader 不设置 chunk 数量上限并持续读取到
EOF。读取中途遇到损坏样本则停止后续转写并返回错误；已经完成的请求无法回滚，与一般
流式消费语义一致。若 `max_size_bytes` 最多只能容纳 0.5 秒音频，容量函数返回
`Some(0)`，reader 不产出 chunk，也不会把整个输入退化为一个无上限单片。

## 回滚

该改动不修改用户配置或持久化数据。若实现阶段发现某个 endpoint 无法接受内存 multipart
reader，可在同一 PR 内把 multipart 构造改为 request-owned `Vec<u8>`，无需恢复临时文件
模型。完整回滚只需恢复 path-based trait 与两个 producer 的临时文件实现，不涉及数据迁移。
