# Session 内聚的 Chunk 结果生命周期

## 状态

**已实现。**

`SessionOrchestrator` 的 chunk 状态与转录结果所有权调整已按本文档实现。

## 背景

实现前，`SessionOrchestrator::start_session` 为每个 session 创建一个 worker 线程，同时创建：

```rust
Arc<Mutex<Vec<ChunkEntry>>>
```

`ActiveSessionInner` 和 worker 各持有一份 `Arc`。worker 通过修改这个共享对象，把 `ChunkEntry` 从 `Flushed` 更新到 `Uploading`，再写入 `Transcribed(String)` 或 `Failed(TranscribeError)`；`finish_session` 则轮询同一个共享对象并收集结果。

这导致 `chunks` 的生命周期不再由 session 单独决定：

- 正常完成时，session 和 worker 在 `join` 后共同释放 `chunks`。
- 收敛超时或 abort 时，orchestrator 会取走并丢弃活动 session，但 detached worker 仍持有 `Arc<Mutex<Vec<ChunkEntry>>>`。
- worker 可以在 session 已经结束后继续读取或写入旧 session 的 chunk 状态。

虽然 Rust 类型系统保证该共享访问没有数据竞争，但 session 不再是 chunk 状态与结果的唯一 owner，生命周期边界不清晰。

## 目标

1. `Vec<ChunkEntry>` 只存放在活动 session 内，由 session 独占。
2. worker 不持有、不读取、不修改 session 的 `chunks`。
3. worker 使用消息返回 chunk 处理状态和转录结果。
4. session 结束时立即释放自己的 `chunks`；迟到的 worker 结果不能延长其生命周期，也不能写入新 session。
5. 保持现有公开接口、串行转写、结果顺序、部分失败与收敛超时语义。

## 非目标

- 不修改 `Transcriber` trait 或 `transcribe(&self, path)` 的同步调用方式。
- 不实现正在执行的 HTTP 转写请求的即时取消。
- 不在本计划中消除 timeout/abort 后可能继续运行的 detached worker。
- 不修改 `AudioRecorder` 的文件读取、切片或录音生命周期。
- 不修改 `RecordingSessionMachine`、热键、托盘或文本注入流程。
- 不引入 Tokio、async runtime、线程池或并行 chunk 转写。

及时取消和 worker 关闭屏障需要单独计划。本计划只收紧 `chunks` 的所有权边界。

## 实现前流程

```text
ActiveSessionInner                       worker
        │                                  │
        └──── Arc<Mutex<Vec<ChunkEntry>>> ─┘
                         │
                  worker 原地修改状态
                         │
                  finish_session 轮询
```

实现前的 worker 签名：

```rust
fn worker_loop(
    rx: mpsc::Receiver<WorkerMsg>,
    chunks: Arc<Mutex<Vec<ChunkEntry>>>,
    transcriber: Arc<dyn Transcriber>,
    cancelled: Arc<AtomicBool>,
)
```

`chunks` 同时承担状态跟踪和转录结果返回，worker 的返回值为 `()`。

## 目标所有权模型

```text
ActiveSessionInner                         worker
┌──────────────────────────┐        ┌──────────────────────┐
│ chunks: Vec<ChunkEntry>  │        │ transcribe(path)     │
│ result_rx                │◀───────│ result_tx.send(...)  │
│ chunk_tx                 │───────▶│ WorkerMsg::Chunk     │
└──────────────────────────┘        └──────────────────────┘
          唯一 owner                     不持有 chunks
```

### Session 内部状态

```rust
struct ActiveSessionInner {
    session_id: SessionId,
    mode: SessionMode,
    chunks: Vec<ChunkEntry>,
    chunk_tx: mpsc::SyncSender<WorkerMsg>,
    result_rx: mpsc::Receiver<WorkerEvent>,
    worker: thread::JoinHandle<()>,
    next_index: usize,
    cancelled: Arc<AtomicBool>,
}
```

`Arc<AtomicBool>` 暂时保留为现有 cooperative cancellation 信号，但不再承载 chunk 状态或转录结果。

### Worker 输出事件

```rust
enum WorkerEvent {
    UploadStarted {
        index: usize,
    },
    Completed {
        index: usize,
        result: Result<String, TranscribeError>,
    },
}
```

worker 只负责：

1. 从 `WorkerMsg` 接收 `{ index, path }`。
2. 检查现有 cancellation flag。
3. 发送 `UploadStarted { index }`。
4. 调用 `transcriber.transcribe(&path)`。
5. 清理 worker 已接管的临时文件。
6. 发送 `Completed { index, result }`。
7. 输入 channel 关闭后退出。

`result_tx.send(...)` 失败只表示 session 已不再接收状态或结果，不能让 worker 提前退出。worker 必须继续消费剩余的 `WorkerMsg`：已取消的消息直接清理文件，未取消的消息仍按既定所有权完成处理与文件清理。每个由 worker 成功接收的路径都必须在 worker 退出前释放，结果 channel 是否存活不影响该清理责任。

worker 不再执行以下操作：

- 锁定 session 数据。
- 查找或修改 `ChunkEntry`。
- 判断 chunk 是否已经被 session 标记为 terminal。
- 将迟到结果写回共享状态。

### Session 应用事件

新增私有辅助函数：

```rust
fn apply_worker_event(
    chunks: &mut [ChunkEntry],
    event: WorkerEvent,
);
```

状态转换全部在 session 一侧完成：

```text
on_chunk_ready                WorkerEvent::UploadStarted
      │                                  │
      ▼                                  ▼
   Flushed ──────────────────────────▶ Uploading
                                          │
                              WorkerEvent::Completed
                                          │
                          ┌───────────────┴──────────────┐
                          ▼                              ▼
                    Transcribed(text)               Failed(error)
```

如果 session 已经将 entry 标记为 `Failed(Timeout)`，迟到的 `Completed` 事件会被忽略。若 session 已经销毁，worker 向已断开的 `result_tx` 发送失败，结果随事件一起被释放。

## 方法行为

### `start_session`

1. 创建有界的 `WorkerMsg` channel。
2. 创建 `WorkerEvent` channel。
3. 在 `ActiveSessionInner` 中直接创建 `Vec<ChunkEntry>`。
4. 将 `result_tx` move 进 worker。
5. worker 不再获得 `chunks` 的引用、锁或 `Arc` clone。

### `on_chunk_ready`

1. 校验 `SessionId`。
2. 将新的 `ChunkEntry { index, state: Flushed }` 直接写入当前 session 的 `Vec`。
3. 非阻塞提交 `WorkerMsg::Chunk`。
4. 队列满或断开时，由 session 直接把对应 entry 标记为 `Failed` 并清理未交付的文件。
5. 在返回前非阻塞 drain 已到达的 `WorkerEvent`，使长录音期间的结果及时归档到 session。

### `finish_session`

1. 校验并取出当前 session，使其成为该调用栈上的独占值。
2. 关闭 chunk sender。
3. 使用 `result_rx.recv_timeout` 等待并应用 worker 事件，而不是轮询共享 `Mutex`。
4. 所有 entry 到达终态后 `join` worker，按 index 汇总结果。
5. 达到收敛超时后，将未完成 entry 标记为 `Failed(Timeout)`，返回当前 partial text。
6. session 离开作用域时，`Vec<ChunkEntry>` 与其中的文本、错误一起释放。

超时后 worker 可能继续完成当前同步转写，但它只持有 `result_tx`。session 的 receiver 已释放，迟到事件发送失败后立即释放，不会保留或修改旧 `chunks`。

### `abort_session`

1. 校验并取出当前 session。
2. 设置现有 cancellation flag。
3. 关闭输入与结果 receiver。
4. session 离开作用域时立即释放 `chunks`。
5. worker 若仍在同步转写，只能得到一次 result-channel disconnected，不能访问旧 session 状态。

本计划保持 `abort_session` 当前的非阻塞返回行为；worker 的强制收敛不在本次范围。

## 文件改动

实际实现范围：

| 文件 | 变更 |
|---|---|
| `src/core/orchestrator.rs` | `chunks` 改为 session 独占 `Vec`；增加 worker result channel；改写 worker、finish、abort 与相关辅助函数和测试 |
| `docs/architecture/core.md` | 记录 session 独占 chunk 状态、worker 只返回事件的结构 |
| `docs/plan/16-session-owned-chunk-results.md` | 更新状态与实际设计差异 |
| `changelog` | 追加 session-owned chunk result lifecycle 条目 |

不计划修改 `src/main.rs`、`src/core/recording_session.rs`、`src/audio/` 或 `src/transcriber/`。

## 实际实现说明

- 使用标准库无界 `mpsc::channel` 作为每个 session 独立的 worker result channel。
- `on_chunk_ready` 在返回前通过 `try_recv` 排空已到达事件；`finish_session` 通过 `recv_timeout` 等待剩余事件。
- result channel 意外断开时，session 将所有非终态 entry 标记为 `Failed(Network)`，随后 join worker 并返回 partial failure，不再等待完整 convergence timeout。
- timeout 与 abort 仍保持非阻塞 detached-worker 语义；result receiver 断开不会阻止 worker 清理已接管的临时文件。
- 按批准决定，本次没有增加 timeout 后取消排队转写/API 调用的行为。

## TDD 实施顺序

严格按测试先行：

### Phase 1：定义所有权与事件测试

先增加或调整测试，验证：

- worker 完成结果通过 `WorkerEvent` 返回。
- `ChunkEntry` 状态只由 session 侧的事件应用函数修改。
- 多 chunk 结果按 index 汇总，不受完成顺序影响。
- worker 不需要 `Arc<Mutex<Vec<ChunkEntry>>>`。

测试失败后再定义 `WorkerEvent`、result channel 和事件应用函数。

### Phase 2：迁移正常完成路径

先增加测试，验证：

- 单 chunk 和多 chunk 正常完成。
- 部分失败保留 partial text。
- worker panic/断开时，未完成 entry 得到确定的失败结果。
- 正常完成后 worker 已 join，session chunks 随 session 释放。

然后将 `start_session`、`on_chunk_ready` 和 `finish_session` 迁移到消息返回模型。

### Phase 3：迁移 timeout 与 abort

先增加测试，验证：

- timeout 后迟到结果不能修改已经返回的 session 结果。
- timeout/abort 后 session 的 `chunks` 不再被 worker 持有。
- timeout/abort 关闭 result receiver 后，worker 仍继续排空输入队列并清理其中的真实临时文件和空 session 目录。
- result channel 断开导致 `UploadStarted` 或 `Completed` 发送失败时，worker 不会提前退出或跳过后续路径清理。
- 输入队列 full/disconnected 时，由提交方清理未交付的真实临时文件和空 session 目录。
- timeout/abort 后可以启动新 session，旧 worker 结果不会路由到新 session。

然后移除 worker 对 `chunks` 的所有共享引用和相关锁操作。

### Phase 4：文档与全量验证

实现完成后：

1. 更新 `docs/architecture/core.md`。
2. 将本文档状态改为已完成并记录实际差异。
3. 更新 `changelog`。
4. 运行格式化、单元测试和全量检查。

## 测试清单

| 测试 | 验证内容 |
|---|---|
| `worker_reports_result_without_shared_chunks` | worker 通过事件返回结果，不接受共享 chunk store |
| `session_applies_worker_state_transitions` | `Flushed → Uploading → Transcribed/Failed` 只在 session 侧发生 |
| `multi_chunk_results_remain_index_ordered` | 完成顺序不影响最终文本顺序 |
| `partial_failure_preserves_successful_text` | 消息返回模型保持现有 partial failure 语义 |
| `timeout_drops_late_worker_result` | timeout 后迟到事件无法修改已结束 session |
| `abort_releases_session_chunks` | abort 后 session-owned chunks 立即释放 |
| `old_worker_result_cannot_reach_new_session` | 新旧 session 使用不同 result channel |
| `queue_full_deletes_rejected_file_and_session_dir` | 使用真实文件填满有界输入队列；session 标记失败并删除未交付文件及空目录 |
| `disconnected_input_deletes_rejected_file_and_session_dir` | 输入 channel 已断开时，提交方删除未交付的真实文件及空目录 |
| `timeout_result_disconnect_drains_and_cleans_queued_files` | timeout 释放 result receiver 后，迟到结果发送失败不阻止 worker 排空并清理队列中的真实文件 |
| `abort_result_disconnect_drains_and_cleans_queued_files` | abort 释放 result receiver 并设置取消后，worker 跳过转写但清理所有已接管文件及空 session 目录 |
| `worker_disconnect_marks_pending_entries_failed` | worker 异常退出不会让 finish 永久等待 |
| 现有 orchestrator 回归测试 | SessionId、文件清理、NoChunks、timeout、panic 与 partial text 行为保持 |

## 验收标准

- [x] `ActiveSessionInner::chunks` 是普通 `Vec<ChunkEntry>`，不是 `Arc` 或 `Mutex`。
- [x] `worker_loop` 参数中不存在 `chunks`。
- [x] worker 不读取或修改任何 `ChunkEntry`。
- [x] 所有 worker 结果通过 session 专属 result channel 返回。
- [x] timeout/abort 后 worker 无法持有或修改旧 session 的 chunk 状态。
- [x] 迟到结果不会进入新 session。
- [x] 输入或结果 channel 断开不会泄漏任何已提交、排队或被拒绝的临时文件及空 session 目录。
- [x] worker 发送结果失败后仍履行其已接管路径的清理责任。
- [x] 现有 `SessionOrchestrator` 公开接口和用户可见结果语义不变。
- [x] 实现遵循 TDD，新增测试先失败、实现后通过。
- [x] `cargo fmt --check`、`cargo check`、`cargo test` 通过。

## 后续计划

以下问题明确留待独立设计：

1. 将同步 `Transcriber::transcribe` 改造成可取消调用。
2. timeout/abort 后等待 worker 的关闭确认，禁止 detached worker。
3. 将 `RecordingSessionMachine` 的 `Idle` 转换绑定到 `SessionClosed`。

这些工作与本计划的 `chunks` 所有权调整解耦，避免一次 PR 同时改变数据所有权、线程取消和顶层状态机。
