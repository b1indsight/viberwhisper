# 待办

本轮 core 审阅的八项改进已实现，详见 [实施计划与验证记录](docs/plan/47-core-session-simplification.md)。

- [x] `src/core/recording_session.rs`：将会话 ID 分配行为收回到 `RecordingSessionMachine`，让 `transition` 使用确定的 ID 计算状态和动作，不再接收或修改 `&mut next_session_id`。仅在接受新会话创建时分配 ID；保留启动失败后不复用 ID、拒绝旧会话事件的行为。
- [x] `src/core/orchestrator.rs`：将仅有 `ActiveSession` 分支的 `SessionStartError` 枚举简化为结构体，保留 `requested`、`active` 字段以及 `Display`、`Error` 实现；同步简化调用方的字段访问，保持启动失败行为不变。
- [x] `src/core/orchestrator.rs`：将 `ActiveSessionInner.chunks` 重命名为 `chunk_entries`，明确其保存的是分块处理记录；同步统一指代这些记录的局部变量和辅助函数参数名称，与音频数据 `WavChunk` 区分。
- [x] `src/core/orchestrator.rs`：调整 `on_chunk_ready` 为先 `try_send`，再根据发送结果确定 `Flushed` 或 `Failed` 状态并一次性 `push` 分块记录，省去发送失败后查找并修改记录的步骤；发送失败的块仍须登记，记录加入后再消费 worker 事件。
- [x] `src/core/orchestrator.rs`：保留 `on_chunk_ready` 的通知入口语义，将返回值简化为 `()`，不再返回分块序号或新增提交错误类型。会话路由不匹配时在内部记录日志并拒绝该块，入队失败时在内部记录日志及失败的分块记录，最终转写结果和分块失败由 `finish_session` 汇总返回；同步移除上游仅用于重复日志的错误处理，并调整依赖返回值的测试。
- [x] 简化会话结果汇总及相关文本合并方法的 `language` 参数：按值传递 `Option<String>`，需要保留配置的调用处使用 `self.language.clone()`，同步调整相关接口和调用。优先保持参数传递直观一致，接受少量配置字符串复制，避免为节省这点复制而在调用链中逐层转换为 `Option<&str>`。
- [x] `src/core/orchestrator.rs`：删除仅有一个调用点的 `drain_worker_events`，将非阻塞消费 worker 事件的循环内联到 `on_chunk_ready`；保留录音期间及时更新分块状态的行为，以及与 `finish_session` 共用的 `apply_worker_event`。
- [x] `src/core/orchestrator.rs`：将仅有一个调用点的 `begin_upload` 和 `record_worker_result` 内联到 `apply_worker_event` 的对应事件分支，移除 `begin_upload` 未被使用的布尔返回值；保留仅允许 `Flushed` 进入 `Uploading`、终态不被迟到完成事件覆盖的约束及相关日志。
