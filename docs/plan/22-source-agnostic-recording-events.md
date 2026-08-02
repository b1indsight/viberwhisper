# 22 - 来源无关的录音事件与 Session 级生命周期

## 状态

已实现。输入归一化、来源无关的 Session 状态/事件、复合启停 Effect、无 mode 的
Orchestrator API、聚焦测试和架构文档均已完成。

## 背景

当前录音状态机同时承载了两类细节：

1. Hold 热键、Toggle 热键和托盘点击的来源与交互语义；
2. Recorder 与 Orchestrator 各自的启动、停止阶段和完成事件。

这些细节使同一个用户意图表现为 `Start`、`Stop`、`Toggle` 等不同控制事件，并让
`RecordingState` 持久化 `source`、`mode`、`StartPhase` 和 `StopPhase`。但输入来源并不
改变录音 Session 的核心生命周期；`SessionMode` 在 Orchestrator 中也只用于启动日志，
不影响转写行为。

本计划将平台输入先归一化为来源无关的开始/停止请求，再把 Recorder 与 Orchestrator
的有序协作封装为 Session 级 Effect。状态机继续是唯一决定生命周期转换的组件，但不再
知道哪个输入设备触发了请求，也不再逐步建模下层组件的同步调用。

## 目标

1. 核心状态机只接收 `StartRequested` 和 `StopRequested` 两种用户意图。
2. Hold、Toggle 和托盘来源只保留在输入集成层，不进入 `SessionEvent` 或
   `RecordingState`。
3. 不同输入可以停止彼此启动的当前录音，核心层不做来源匹配。
4. 将 Recorder 与 Orchestrator 的同步启动流程合并为一个原子的 Session 启动 Effect。
5. 将停止录音、提交尾片、等待转写和最终文本处理合并为一个 Session 停止 Effect。
6. 删除只用于同步下层步骤的 mode、source 和 phase 状态，同时保留 Session ID 路由、
   失败恢复、chunk 顺序和关闭清理。

## 非目标

- 不实现完全事件驱动主循环；该工作由 GitHub issue #89 跟踪。
- 不改变 20ms 主循环间隔、热键配置、托盘防抖或原生事件泵。
- 不改变音频 chunk 大小、转写重试、收敛超时、后处理或文本注入策略。
- 不移除 Session ID，也不放宽 Recorder/Orchestrator 的 Session 路由检查。
- 不引入 async runtime、通用 event bus、typestate 或仅为测试而增加的运行时 trait 层。

## 设计

### 1. 输入来源止步于集成层

`HotkeyEvent`、`HotkeySource` 和 `TrayAction` 继续由 `src/input/` 表达原始输入。`main.rs`
增加一个私有、无副作用的归一化函数，根据当前稳定状态将原始交互转换为核心请求：

| 原始输入 | `Idle` | `Recording` | 其他状态 |
| --- | --- | --- | --- |
| Hold 按下 | `StartRequested` | 丢弃 | 丢弃 |
| Hold 释放 | 丢弃 | `StopRequested` | 丢弃 |
| Toggle 按下 | `StartRequested` | `StopRequested` | 丢弃 |
| 托盘点击 | `StartRequested` | `StopRequested` | 丢弃 |

Toggle 键释放仍由现有热键 mapper 丢弃。按键重复和托盘双击仍分别由现有输入层逻辑
去重。`ShutdownRequested` 保持独立，不参与开始/停止归一化。

归一化发生在事件从 `HotkeyManager` 或 `TrayManager` 取出时，使用状态机的只读
`state()` 快照；输入模块不持有或复制录音状态。归一化后，来源不再写入核心事件、状态
或 Effect。

这一规则有一个明确后果：来源无关的 Hold release 会停止当时正在运行的任何 Session，
即使该 Session 由 Toggle 或托盘启动。这是允许不同输入互相停止的预期行为。

### 2. 精简核心状态

状态收敛为：

```rust
pub enum RecordingState {
    Idle,
    Starting { session_id: SessionId },
    Recording { session_id: SessionId },
    Stopping { session_id: SessionId },
    ShuttingDown { session_id: Option<SessionId> },
}
```

`Starting` 和 `Stopping` 保留，因为它们表达“请求已接受，但 Session 级操作尚未完成”的
真实业务状态。删除以下只描述同步实现步骤的类型和字段：

- `SessionMode`
- `ControlSource`
- `ControlAction`
- `ControlEvent`
- `StartPhase`
- `StopPhase`
- 状态中的 `mode`、`source` 和 `phase`

### 3. 使用 Session 级事件

核心事件收敛为：

```rust
pub enum SessionEvent {
    StartRequested,
    StopRequested,
    SessionStarted { session_id: SessionId },
    SessionStartFailed { session_id: SessionId, error: String },
    ChunkReady { session_id: SessionId, chunk: WavChunk },
    SessionStopped { session_id: SessionId },
    SessionStopFailed { session_id: SessionId, error: String },
    ShutdownRequested,
}
```

`SessionStarted` 表示 Recorder 和 Orchestrator 都已成功启动。`SessionStopped` 表示录音已
停止、尾片已提交、Orchestrator 已完成收敛，并且最终文本结果已经交给现有后处理/注入
路径。转写为空或转写失败仍按现有规则记录并结束 Session，不将其误报为 recorder 停止
失败。

事件摘要继续为带 Session ID 的结果事件执行统一路由。旧 Session 的完成、失败或 chunk
仍不能改变当前 Session。

### 4. 使用 Session 级 Effect

启动和停止 Effect 合并为：

```rust
pub enum SessionEffect {
    StartSession { session_id: SessionId },
    StopSession { session_id: SessionId },
    SubmitChunk { session_id: SessionId, chunk: WavChunk },
    CancelRecorder { session_id: SessionId },
    AbortOrchestrator { session_id: SessionId },
    SetTrayRecording(bool),
    ReadyToExit,
}
```

删除 `StartRecorder`、`StartOrchestrator`、`StopRecorder` 和 `FinishOrchestrator`。实时
`ChunkReady` 仍通过 `SubmitChunk` Effect 进入 Orchestrator；只有 Session 启停内部的
尾片提交由复合 Effect 按固定顺序完成。

`drive_session` 仍只执行状态机批准的 Effect，并把一个 Session 级结果事件放回本地
队列。它不自行决定是否允许开始或停止。

### 5. 原子启动与失败回滚

`StartSession` 按以下固定顺序执行：

1. 调用 `recorder.start_recording(session_id)`；
2. Recorder 成功后调用 `orchestrator.start_session(session_id)`；
3. 两者均成功时产生 `SessionStarted`；
4. Recorder 已在运行时，清理其报告的 active Session 及对应 Orchestrator；
5. Orchestrator 启动失败时，取消刚启动的 Recorder，并清理错误报告的 active
   Orchestrator；
6. 任一失败路径完成回滚后产生 `SessionStartFailed`。

因此 `SessionStarted` 是全有或全无的 Session 建立结果。状态机无需处理
`RecorderStarted`、`RecorderAlreadyRecording`、`OrchestratorStarted` 等组件级事件。
错误日志继续保留具体失败阶段，公共事件只需要 Session ID 和错误正文。

### 6. 原子停止与现有结果语义

状态机接受 `StopRequested` 时立即进入 `Stopping`，先发出
`SetTrayRecording(false)`，再发出 `StopSession`。这样托盘在停止和转写收敛期间保持
空闲显示；如果 recorder 报告仍在录音，`SessionStopFailed` 会让状态机回到
`Recording` 并恢复红色托盘状态。

`StopSession` 按以下固定顺序执行：

1. 调用 `recorder.stop_recording(session_id)`；
2. 对 `Stopped` 返回的所有尾片按原顺序调用 `orchestrator.on_chunk_ready`；
3. `NotRecording` 视为没有额外尾片，但仍完成当前 Orchestrator；
4. 调用 `orchestrator.finish_session(session_id)`，并通过现有 `finish_transcription`
   完成后处理和文本注入；
5. 完成后产生 `SessionStopped`；
6. 只有 `StillRecording` 产生 `SessionStopFailed`，且不结束 Orchestrator。

停止被接受后托盘会比当前实现最多提前一次同步 `stop_recording` 调用切换为空闲；失败时
会恢复录音显示。除此之外，尾片顺序、warning 日志、转写错误处理和最终文本注入保持
不变。

### 7. 简化转换表

允许路径变为：

| 当前状态 | 事件 | 下一状态 | Effect |
| --- | --- | --- | --- |
| `Idle` | `StartRequested` | `Starting` | `StartSession` |
| `Starting` | 匹配的 `SessionStarted` | `Recording` | 托盘设为录音中 |
| `Starting` | 匹配的 `SessionStartFailed` | `Idle` | 托盘设为空闲 |
| `Recording` | 匹配的 `ChunkReady` | `Recording` | `SubmitChunk` |
| `Recording` | `StopRequested` | `Stopping` | 托盘设为空闲、`StopSession` |
| `Stopping` | 匹配的 `SessionStopped` | `Idle` | 无 |
| `Stopping` | 匹配的 `SessionStopFailed` | `Recording` | 托盘恢复录音中 |
| 非关闭状态 | `ShutdownRequested` | `ShuttingDown` | 取消、终止、重置托盘、退出 |

其余事件保持现有策略：不改变状态、不产生 Effect，并写一条不包含大载荷的 debug
拒绝日志。

### 8. 移除无行为差异的 SessionMode

`SessionOrchestrator::start_session` 改为只接收 `session_id`。删除从
`recording_session` 重导出的 `SessionMode`，并更新调用方与测试。启动日志继续记录
Session ID；不再记录不影响任何 Orchestrator 分支的 mode。

## 文件改动

| 文件 | 计划改动 |
| --- | --- |
| `src/main.rs` | 在输入边界归一化 Hold/Toggle/托盘动作；执行复合的 Session 启停 Effect；更新集成测试。 |
| `src/core/recording_session.rs` | 删除来源、模式和阶段类型；引入来源无关请求、Session 级结果/Effect；简化转换表和单元测试。 |
| `src/core/orchestrator.rs` | 从 `start_session` 删除无行为作用的 `SessionMode` 参数并更新测试。 |
| `docs/architecture/core.md` | 记录来源无关状态机、Session 级 Effect 和简化转换表。 |
| `docs/architecture/input.md` | 记录原始来源止步输入集成层以及状态感知的归一化规则。 |
| `docs/plan/22-source-agnostic-recording-events.md` | 保存获批设计并在实现后更新状态。 |
| `changelog` | 记录事件与状态模型简化。 |

预计不修改 `src/input/hotkey.rs`、`src/input/tray.rs`、`src/audio/recorder.rs`、配置或依赖。
现有输入枚举和下层结构化 outcome 已足够执行计划。

## 测试优先的实现顺序

获得明确批准后：

1. 先更新状态机测试，覆盖来源无关的开始、停止、启动失败、停止失败、关闭和 Session ID
   路由；此时测试应因旧事件/Effect API 而失败或无法编译。
2. 增加纯输入归一化测试，覆盖 Hold/Toggle/托盘在 `Idle`、`Recording` 和过渡状态的映射，
   包括 Hold release 停止任意当前 Session。
3. 实现精简后的状态、事件、Effect 和显式转换表，使聚焦测试通过。
4. 将 `drive_session` 改为执行原子的 `StartSession`/`StopSession`，保持启动失败回滚、尾片
   顺序、warning、转写和文本注入行为。
5. 删除 Orchestrator 的 `SessionMode` 参数并更新所有调用方与测试。
6. 删除过时类型和分支，确认仓库中不再存在核心层 `ControlSource`、`ControlAction`、
   `ControlEvent`、`StartPhase`、`StopPhase` 或 `SessionMode`。
7. 更新架构文档、本计划状态和 changelog。
8. 运行聚焦测试、完整测试、格式检查、拒绝 warning 的 Clippy，并检查最终 diff。

不为实际麦克风、原生托盘或网络调用增加不稳定的集成测试；复合 Effect 使用已有的
Recorder/Orchestrator 结构化结果和各模块现有测试，核心行为由纯状态机及输入归一化测试
覆盖。

## 验证方式

```bash
cargo fmt --check
cargo test core::recording_session::tests
cargo test input_normalization
cargo test
cargo clippy -- -D warnings
```

所有新增测试保持确定性，不访问麦克风、托盘、网络或真实计时。

## 验收标准

- `SessionEvent` 的用户控制面只有来源无关的 `StartRequested` 和 `StopRequested`。
- 输入来源不出现在 `RecordingState`、`SessionEvent` 或 `SessionEffect` 中。
- Hold、Toggle 和托盘在当前状态允许时都归一化到同一开始/停止事件。
- 不同输入可以停止彼此启动的当前 Session；Hold release 不进行来源匹配。
- 不适用于当前状态的原始输入在进入状态机前丢弃。
- 状态机只保留 `Idle`、`Starting`、`Recording`、`Stopping` 和 `ShuttingDown`，且过渡状态
  只携带 Session ID。
- Recorder 和 Orchestrator 只有全部启动成功后才产生 `SessionStarted`；失败会完成回滚并
  返回 `SessionStartFailed`。
- 停止路径保持尾片提交顺序、转写收敛、后处理和文本注入行为；recorder 仍在运行时恢复
  `Recording` 状态和托盘显示。
- Session ID 路由、陈旧 chunk/完成事件拒绝和关闭清理继续有效。
- `SessionMode` 从核心与 Orchestrator API 删除，不引入替代配置。
- 完全事件驱动循环仍留在 #89，本次不扩大范围。
- 聚焦测试、完整测试、格式检查和 Clippy 全部通过。
