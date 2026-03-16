# 全流程 Stream 识别

## 背景

PR #26（`05-long-audio-streaming.md`）已落地以下底层能力：

| 能力 | 所在模块 |
|------|----------|
| WAV 离线分片（`split_wav`） | `src/audio/splitter.rs` |
| Toggle 录音达阈值封片落盘 | `src/audio/recorder.rs` |
| 单片指数退避重试 | `src/transcriber/api.rs` |
| 多语言文本合并（`merge_texts`） | `src/transcriber/api.rs` |
| 三个分片配置项 | `src/core/config.rs` |

然而，**会话（session）级的统一调度层**尚未建立：分片的生成、上传队列、结果收集、错误传播等逻辑散布在 `recorder.rs` 与 `api.rs` 中，Hold 与 Toggle 两种模式也没有共享统一的会话生命周期模型。

本文档的目标是：在不重复已有底层机制的前提下，设计并实现一套**完整的端到端 stream 识别链路**，将两种录音模式的会话生命周期、chunk 状态机、结果收敛、错误传播全部收口到独立的 orchestrator 模块中。

## 当前边界（PR #26 已提供）

以下能力已实现，本方案**不重复设计**，仅将其视为基础构件调用：

- **`split_wav(path, max_duration_secs, max_size_bytes) → Vec<TmpChunk>`**：将已有 WAV 文件切为若干临时分片，`TmpChunk` 自动通过 `Drop` 清理。
- **`TmpChunk`**：封装分片路径与序号，作用域结束即删除临时文件。
- **`transcribe_chunk_with_retry(&chunk) → Result<String>`**：对单片执行指数退避重试，4xx 不重试，5xx 最多 `max_retries` 次。
- **`merge_texts(results, language) → String`**：语言感知拼接，中文无空格，其他语言以空格分隔。
- **录音达阈值封片**：`AudioRecorder` 在录音过程中按 `max_chunk_duration_secs` / `max_chunk_size_bytes` 自动封片落盘。

## 目标

1. 引入 **`SessionOrchestrator`**，统一 Hold / Toggle 两种模式的会话生命周期管理，消除 `recorder.rs` 与 `main.rs` 中的重复调度逻辑。
2. 定义 **Chunk 状态机**，让每个分片的完整流转路径可观测、可测试。
3. 明确**结果收敛协议**：停止录音后如何等待后台上传、超时处理、有序合并。
4. 统一**错误传播语义**：分片上传失败时，是立即中断会话还是继续收集其他分片，并在最终输出时明确报告。
5. 为 orchestrator 补充**完整测试与验收标准**，使端到端路径可在 CI 中验证（网络部分使用 mock）。

## 非目标

- **真正的增量式实时字幕**：结果仍以整次会话结束后统一输出为边界，不做 token 级 / 句级流式 UI 更新。
- **修改 `Transcriber` trait 签名**：`transcribe(&self, wav_path: &str)` 保持不变，orchestrator 在 trait 之上构建。
- **并发分片上传**：仍维持串行上传，不引入额外异步运行时（`reqwest::blocking` 不变）。
- **重新实现底层分片 / 重试 / 合并**：这些逻辑已在 PR #26 中实现，orchestrator 直接复用。
- **修改 Hold/Toggle 热键检测逻辑**：`hotkey.rs` 的 `HotkeySource` / `HotkeyEvent` 接口不变。

## TODO

- [x] 明确后台上传线程与主线程之间的同步原语选型：使用 `std::sync::mpsc`（上传 worker 通过 `Sender<TmpChunk>` 接收分片，主线程通过 `Receiver` 驱动收敛等待）
- [x] 确认停止录音时等待收敛的超时上限（默认 `convergence_timeout_secs = 30`）
- [ ] 评估 `SessionOrchestrator` 是否需要持有 `Arc<dyn Transcriber>` 以便测试注入
- [ ] 补充日志 / 进度可观测性（分片序号、上传耗时、重试次数）

## 架构方案

### 全流程总览

```text
热键事件
  │
  ▼
SessionOrchestrator::start_session(mode: SessionMode)
  ├─ 创建 Session（id, mode, start_time, chunk_queue, result_store）
  └─ 通知 AudioRecorder 开始录音

录音进行中
  AudioRecorder → 封片落盘 → SessionOrchestrator::on_chunk_ready(chunk)
                                └─ 将 chunk 状态置为 Flushed
                                └─ 提交到后台上传队列（串行 worker）
                                     └─ transcribe_chunk_with_retry(chunk)
                                          ├─ 成功 → result_store[chunk.index] = text
                                          │          chunk 状态 → Transcribed
                                          └─ 失败 → chunk 状态 → Failed(err)
                                                    session_error = Some(err)  ← 仅记录，不中断

热键结束（Hold 松开 / Toggle 再次触发）
  SessionOrchestrator::stop_session()
  ├─ 通知 AudioRecorder 停止录音
  ├─ 录音线程 flush 尾片 → on_chunk_ready(tail_chunk)
  └─ wait_for_convergence(timeout)
       ├─ 等待所有已提交 chunk 达到终态（Transcribed | Failed）
       ├─ 超时 → 将未完成 chunk 标记为 Failed(Timeout)
       └─ collect_results()
            ├─ 按 chunk.index 顺序过滤 Transcribed 结果
            ├─ 存在 Failed chunk → 返回 Err（附带失败详情 + 已成功分片索引）
            └─ 全部成功 → merge_texts(results, language) → Ok(String)

最终结果
  └─ main.rs / run_listener → TextTyper::type_text(result) 或打印错误
```

### Session 生命周期

两种模式共享同一个 `SessionOrchestrator`，差异仅在触发时机：

```text
Hold 模式                          Toggle 模式
─────────────────────────────────  ──────────────────────────────────────
KeyDown(Hold)                      KeyDown(Toggle) [第一次]
  └─ start_session(Hold)             └─ start_session(Toggle)

  … 录音 + 封片 + 后台上传 …          … 录音 + 封片 + 后台上传 …

KeyUp(Hold)                        KeyDown(Toggle) [第二次]
  └─ stop_session()                  └─ stop_session()

  wait_for_convergence()             wait_for_convergence()
  collect_results()                  collect_results()
  → 输出文本                          → 输出文本
```

`stop_session()` 是同步阻塞调用（含超时），确保主线程在输出前拿到完整结果。

### Chunk 状态机

每个 chunk 在 `SessionOrchestrator` 内部经历以下状态流转：

```text
                  ┌──────────────────────────────────────────┐
                  │                                          │
  录音中 ──封片──▶ Flushed ──提交队列──▶ Uploading ──成功──▶ Transcribed
                                         │    ▲
                                         │    │ 网络错误（非 API 内部）且未超时 → 重试
                                         │    └────────────────────────────────┘
                                    重试耗尽（5xx）/ 4xx / 超时后网络仍失败
                                         │
                                         ▼
                                       Failed(TranscribeError)
                                         ▲
                  wait_for_convergence 超时
                                         │
                              Uploading / Flushed ──▶ Failed(Timeout)
```

状态定义：

```rust
pub enum ChunkState {
    Flushed,
    Uploading { attempt: u32 },
    Transcribed(String),
    Failed(TranscribeError),
}

pub enum TranscribeError {
    Api { status: u16, body: String },
    Network(String),
    Timeout,
}
```

### 结果收敛协议

**收敛等待**：`wait_for_convergence` 轮询（或通过 `mpsc` 通知）所有已知 chunk 是否进入终态。

```rust
fn wait_for_convergence(&self, timeout: Duration) -> Result<(), ConvergenceError> {
    let deadline = Instant::now() + timeout;
    loop {
        if self.all_chunks_terminal() { return Ok(()); }
        if Instant::now() >= deadline {
            self.mark_pending_as_timeout();
            return Err(ConvergenceError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
```

**有序收集**：`collect_results` 按 `chunk.index` 升序排列，确保拼接顺序与录音时序一致：

```rust
fn collect_results(&self) -> Result<String, SessionError> {
    let mut texts = Vec::new();
    let mut errors = Vec::new();
    for chunk in self.chunks_ordered() {
        match &chunk.state {
            ChunkState::Transcribed(t) => texts.push(t.clone()),
            ChunkState::Failed(e) => errors.push((chunk.index, e.clone())),
            _ => unreachable!("all chunks must be terminal after convergence"),
        }
    }
    if errors.is_empty() {
        Ok(merge_texts(&texts, self.language.as_deref()))
    } else {
        Err(SessionError::PartialFailure { errors, partial_text: merge_texts(&texts, self.language.as_deref()) })
    }
}
```

**错误语义**：`PartialFailure` 同时携带 `partial_text`（已成功分片的拼接结果）和 `errors`（失败分片的详情），上层决定是丢弃部分结果还是输出并提示用户。

### 错误传播策略

| 错误来源 | 当前行为（PR #26） | 本方案调整 |
|----------|-------------------|-----------|
| 单片上传失败（网络错误，非 API 内部错误） | 立即中断，不处理后续分片 | 在超时范围内重试（`convergence_timeout_secs` 窗口内）；超时前未能成功则标记为 `Failed(Network)`，继续处理其他分片 |
| 单片上传失败（5xx API 错误，重试耗尽） | 立即中断，不处理后续分片 | 记录 `Failed(Api { status, body })`，继续处理其他分片，收敛时汇总报告 |
| 单片 4xx（客户端错误） | 立即中断 | 同上，标记 `Failed(Api { status, body })`，不重试，继续 |
| 收敛超时 | 不存在（无超时机制） | 标记未完成分片为 `Failed(Timeout)`，返回 `ConvergenceError::Timeout` |
| 所有分片成功 | `Ok(merged_text)` | 不变 |
| 部分分片失败 | 不存在（短路失败） | `Err(SessionError::PartialFailure { ... })` |

**设计理由**：长录音场景下单片偶发失败不应丢弃用户前几分钟的转录结果。网络层面的瞬时抖动（非 API 内部错误）应在 `convergence_timeout_secs` 窗口内进行重试，给网络恢复机会；API 4xx / 5xx 耗尽重试后则直接标记失败。整体改为"尽力收集 + 收敛时汇总"的策略，上层可根据错误详情决定是否重新上传失败分片（后续扩展点）。

### Orchestrator 抽离

将以下逻辑从 `recorder.rs` / `main.rs` 移入独立模块 `src/core/orchestrator.rs`：

- Session 创建与生命周期管理
- Chunk 状态追踪（`chunks: Vec<ChunkEntry>`）
- 后台上传 worker 线程（单线程串行队列）
- 收敛等待与结果收集
- 错误聚合

`recorder.rs` 只负责采集 PCM 和按阈值封片落盘，通过回调或 channel 将 `TmpChunk` 交给 orchestrator。

`main.rs` 只负责响应热键事件，调用 `orchestrator.start_session()` / `orchestrator.stop_session()`，并将最终结果传给 `TextTyper`。

```
main.rs
  └─ run_listener()
       ├─ HotkeyEvent::Pressed(Hold/Toggle) → orchestrator.start_session(mode)
       └─ HotkeyEvent::Released(Hold) / HotkeyEvent::Pressed(Toggle 第二次)
            → result = orchestrator.stop_session()
            → match result { Ok(text) → typer.type_text(text), Err(e) → log/notify }

src/core/orchestrator.rs
  SessionOrchestrator
  ├─ start_session(mode) → SessionId
  ├─ on_chunk_ready(chunk: TmpChunk)    ← AudioRecorder 通过 channel 推送
  ├─ stop_session() → Result<String, SessionError>
  └─ (private) upload_worker_loop()    ← 独立线程，串行处理队列

src/audio/recorder.rs
  AudioRecorder
  ├─ start_recording(chunk_tx: Sender<TmpChunk>)
  └─ stop_recording() → flush 尾片 → chunk_tx.send(tail_chunk)
```

## 数据类型设计

### `Session`（`src/core/orchestrator.rs` 内部）

```rust
struct Session {
    id: u64,
    mode: SessionMode,
    started_at: Instant,
    chunks: Vec<ChunkEntry>,
    language: Option<String>,
}

struct ChunkEntry {
    index: usize,
    state: ChunkState,
}

pub enum SessionMode {
    Hold,
    Toggle,
}
```

### `SessionError`

```rust
pub enum SessionError {
    NoChunks,
    PartialFailure {
        errors: Vec<(usize, TranscribeError)>,
        partial_text: String,
    },
    ConvergenceTimeout {
        pending_count: usize,
        partial_text: String,
    },
}

impl fmt::Display for SessionError { ... }
```

### `SessionOrchestrator` 公开接口

```rust
pub struct SessionOrchestrator { ... }

impl SessionOrchestrator {
    pub fn new(transcriber: Arc<dyn Transcriber>, config: &AppConfig) -> Self;
    pub fn start_session(&self, mode: SessionMode);
    pub fn stop_session(&self) -> Result<String, SessionError>;
    // (内部) on_chunk_ready 通过 channel 异步处理
}
```

`transcriber: Arc<dyn Transcriber>` 使测试时可注入 `MockTranscriber`，不依赖真实 HTTP。

## 模块改动点

| 文件 | 变更类型 | 具体说明 |
|------|----------|----------|
| `src/core/orchestrator.rs` | **新增** | `SessionOrchestrator`、`Session`、`ChunkEntry`、`ChunkState`、`SessionError` 全部定义与实现 |
| `src/core/mod.rs` | **修改** | `pub mod orchestrator;` + `pub use orchestrator::SessionOrchestrator;` |
| `src/audio/recorder.rs` | **修改** | `start_recording` 接受 `chunk_tx: Sender<TmpChunk>` 参数；移除原有的内联调度逻辑 |
| `src/main.rs` | **修改** | `run_listener` 持有 `SessionOrchestrator`；热键事件映射到 `start_session` / `stop_session`；移除原有内联分片转写逻辑 |
| `src/core/config.rs` | **修改** | 新增 `convergence_timeout_secs: u64`（默认 30），更新 `get_field`/`set_field`/`apply_json` |
| `src/transcriber/mod.rs` | **不变** | `Transcriber` trait 签名不变 |
| `src/transcriber/api.rs` | **不变** | `transcribe_chunk_with_retry`、`merge_texts` 不变；orchestrator 直接调用 |
| `docs/architecture/core.md` | **修改** | 补充 `SessionOrchestrator` 及其与 recorder / transcriber 的协作关系 |

## 配置项

在 `AppConfig` 中新增以下字段（向后兼容，旧配置不声明时使用默认值）：

```json
{
  "convergence_timeout_secs": 30
}
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `convergence_timeout_secs` | `u64` | `30` | `stop_session` 等待所有后台分片完成的最长时间（秒）。超时后未完成分片标记为 `Failed(Timeout)`，返回部分结果 |

Rust 端定义：

```rust
#[serde(default = "default_convergence_timeout")]
pub convergence_timeout_secs: u64,
// fn default_convergence_timeout() -> u64 { 30 }
```

## 边界与错误处理

| 情况 | 处理方式 |
|------|----------|
| `stop_session` 时无任何 chunk（极短录音未触发封片） | `Err(SessionError::NoChunks)`；调用方决定是否静默忽略 |
| 所有分片成功 | `Ok(merged_text)` |
| 部分分片失败 | `Err(SessionError::PartialFailure { errors, partial_text })` |
| 收敛超时 | 标记未完成分片为 `Failed(Timeout)`，返回 `Err(SessionError::ConvergenceTimeout { ... })`，携带已完成分片的 `partial_text` |
| 后台 worker 线程 panic | 主线程通过 `JoinHandle` 检测；worker 死亡时 orchestrator 将所有 Flushed / Uploading chunk 标记为 `Failed(Network("worker panicked"))` |
| 会话重入（前一会话未结束，热键再次触发） | `start_session` 检测到活跃会话时调用 `stop_session` 强制终止前一会话，再开新会话 |
| `max_chunk_duration_secs = 0` 且 `max_chunk_size_bytes = 0` | 录音过程中无封片，停止录音时仅 flush 尾片，退化为单片路径 |

## 测试计划

### `src/core/orchestrator.rs` 单元测试

全部使用 `MockTranscriber`，无网络依赖，可在 CI 中稳定运行。

| 测试名 | 验证内容 |
|--------|----------|
| `test_single_chunk_success` | 单片会话正常完成，返回 `Ok(text)` |
| `test_multi_chunk_ordered_merge` | 多片会话按 index 顺序合并，而非到达顺序 |
| `test_partial_failure_returns_error_with_partial_text` | 部分分片失败时返回 `PartialFailure`，`partial_text` 包含成功分片内容 |
| `test_convergence_timeout` | mock transcriber 阻塞超过 `convergence_timeout_secs` 时返回 `ConvergenceTimeout` |
| `test_no_chunks_returns_error` | 极短录音无封片时 `stop_session` 返回 `NoChunks` |
| `test_hold_and_toggle_same_lifecycle` | Hold / Toggle 两种模式复用同一 `start_session` / `stop_session` 路径 |
| `test_session_reentry_terminates_previous` | 前一会话未结束时再次 `start_session` 正确终止旧会话 |
| `test_worker_panic_marks_chunks_failed` | worker 线程 panic 后所有未完成 chunk 标记为 `Failed` |

### `src/audio/recorder.rs` 修改后测试

| 测试名 | 验证内容 |
|--------|----------|
| `test_chunk_tx_receives_flushed_chunk` | 达到封片阈值时 `chunk_tx` 收到正确的 `TmpChunk` |
| `test_stop_recording_flushes_tail` | 停止录音时尾片通过 `chunk_tx` 发送 |
| `test_short_recording_single_tail_chunk` | 短录音（未触发中间封片）只发送一个尾片 |

### `src/core/config.rs` 配置测试

| 测试名 | 验证内容 |
|--------|----------|
| `test_default_convergence_timeout` | 默认值为 30 |
| `test_apply_json_convergence_timeout` | 从 JSON 正确反序列化 |
| `test_backward_compat_missing_convergence_timeout` | 旧配置缺少该字段时使用默认值 |
| `test_get_set_convergence_timeout` | `get_field`/`set_field` 正常工作 |

### 端到端集成测试

`test_end_to_end_stream_recognition`（需要 `TRANSCRIPTION_API_KEY`，CI 中通过 `#[ignore]` 跳过，本地验证使用）：

1. 构造 > 30 秒的合成语音 WAV（使用 `hound` 生成静音 + tone，代替真实麦克风）
2. 通过 `SessionOrchestrator` 走完整个 Toggle 会话路径
3. 验证返回文本长度 > 0，且无错误

## 验收标准

以下标准为本方案 PR 合并的门槛：

- [ ] `cargo test` 全绿（包含所有新增单元测试）
- [ ] 所有新增单元测试不依赖网络或麦克风，可在 CI 中稳定运行
- [ ] Hold 模式：按下热键 → 录音开始；松开热键 → 等待收敛 → 文本输出；短录音（< `max_chunk_duration_secs`）行为与 PR #26 前完全一致
- [ ] Toggle 模式：第一次热键 → 录音开始；第二次热键 → 等待收敛 → 文本输出；与 Hold 模式共享同一 orchestrator 路径
- [ ] 单片失败不丢弃其他片的结果：`PartialFailure` 错误中包含已成功分片的合并文本
- [ ] 收敛超时可配置（`convergence_timeout_secs`），超时后返回已完成部分结果而非静默丢弃
- [ ] `src/core/orchestrator.rs` 与 `src/audio/recorder.rs` 各自职责清晰，无循环依赖
- [ ] `main.rs` 中无内联的分片调度逻辑（全部委托给 `SessionOrchestrator`）
- [ ] `docs/architecture/core.md` 更新以反映 orchestrator 模块

## 分阶段落地

### Phase 1 — 数据类型与接口定义（无副作用）

1. 新增 `src/core/orchestrator.rs`，只定义类型：`SessionMode`、`ChunkState`、`TranscribeError`、`SessionError`、`SessionOrchestrator`（空实现，方法均 `todo!()`）
2. 更新 `src/core/mod.rs` 导出
3. 新增配置字段 `convergence_timeout_secs`（含默认值、serde、`get_field`/`set_field`）

**验收**：`cargo build` 成功；`cargo test core::config` 全绿。

### Phase 2 — Orchestrator 核心逻辑

1. 实现 `start_session` / `on_chunk_ready` / `wait_for_convergence` / `collect_results`
2. 实现后台 worker 线程（`mpsc::channel` + `JoinHandle`）
3. 写全部 orchestrator 单元测试（使用 `MockTranscriber`）

**验收**：`cargo test core::orchestrator` 全绿；所有测试不需要网络。

### Phase 3 — Recorder 解耦

1. `AudioRecorder::start_recording` 增加 `chunk_tx: Sender<TmpChunk>` 参数
2. 移除 `recorder.rs` 中原有的内联分片调度逻辑
3. 写 recorder 修改后单元测试（验证 `chunk_tx` 接收行为）

**验收**：`cargo test audio::recorder` 全绿；短录音行为与改动前一致（通过现有测试回归）。

### Phase 4 — Main.rs 接入

1. `run_listener` 中实例化 `SessionOrchestrator`，持有 `Arc<dyn Transcriber>`
2. 热键事件映射到 `start_session` / `stop_session`
3. 移除 `run_listener` 中原有的内联分片转写逻辑

**验收**：`cargo run` 功能正常；Hold 和 Toggle 两种模式手动测试通过（短录音 + 长录音）。

### Phase 5 — 文档与收尾

1. 更新 `docs/architecture/core.md`，补充 `SessionOrchestrator` 模块说明
2. 更新 `docs/README.md`，纳入本文档
3. 本文档（`06-end-to-end-stream-recognition.md`）状态由计划标记为 IN PROGRESS
4. 更新 `changelog`

**验收**：`cargo test` 全绿；PR review 通过。

## 后续扩展方向

- **增量式实时字幕**：orchestrator 在每个分片 `Transcribed` 时立即通过回调通知 UI，而非等待收敛。当前接口预留了 `on_chunk_transcribed` 扩展点位置。
- **失败分片重传**：收敛后对 `PartialFailure` 中的失败分片可选择性重试，无需重录。
- **并发上传**：worker 改为固定大小线程池，`collect_results` 维持有序合并；仅在串行延迟成为瓶颈后评估。
- **会话历史持久化**：将每次会话的 chunk 路径、转录文本、错误信息写入本地 SQLite，方便回溯和二次编辑。
