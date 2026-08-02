# 21 - 显式且轻量的录音状态转换

## 状态

已实现。统一 Session 路由门、显式转换表、单一状态写入口、统一拒绝日志和
`Recovering` 清理均已完成；聚焦测试、完整测试、格式检查和 Clippy 全部通过。

## 目标

将 `RecordingSessionMachine` 收敛为一个显式、有限、单入口的运行时状态机，并保持实现
简单：

1. 状态只能沿代码中明确列出的有限路径转换，其他事件统一进入异常处理。
2. `handle(SessionEvent)` 是外部改变状态的唯一入口。
3. 不引入 typestate、每状态事件类型或复杂的转换结果层。
4. 被拒绝的事件只做轻量日志记录，不影响正常流程。

本次重构保持现有录音生命周期、Effect 顺序和用户行为不变。

## 非目标

- 使用 typestate 泛型替换运行时 `RecordingState`。
- 为每个状态定义独立的事件枚举和 `TryFrom<SessionEvent>` 转换。
- 增加 `SessionTransition`、`TransitionOutcome` 或 `IgnoreReason` 等公共结果类型。
- 修改 Hold、Toggle 或托盘控制语义。
- 修改 Recorder、Orchestrator、转写、后处理或文本注入行为。
- 增加异步清理确认协议、指标、持久化诊断或新配置项。

## 设计原则

### 1. 保留一个外部状态写入口

`RecordingSessionMachine` 对外继续只暴露：

```rust
pub fn state(&self) -> &RecordingState;

pub fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect>;
```

`state()` 只提供只读观察；所有状态更新都由 `handle(event)` 完成。状态字段保持私有，
不增加公开 setter，也不让 `main.rs`、热键、托盘、Recorder 或 Orchestrator 直接修改
状态。

`handle` 的返回类型保持不变，因此 `drive_session` 和 Effect 执行循环不需要增加新的
协议或分支。

### 2. 使用一个私有显式转换表

状态机内部增加一个私有、无日志副作用的转换函数。它消费当前状态和一个事件，只在
匹配明确允许的路径时返回下一状态和 Effect：

```rust
struct Transition {
    next: RecordingState,
    effects: Vec<SessionEffect>,
}

fn transition(
    state: RecordingState,
    event: SessionEvent,
    next_session_id: &mut u64,
) -> Option<Transition> {
    match (state, event) {
        // 每个允许路径都是一个显式 match arm。

        _ => None,
    }
}
```

`RecordingState` 只包含可复制的小值字段，可派生 `Copy`。`transition` 因此可以取得当前
状态的值，而只有 `handle` 会把成功转换产生的 `next` 写回 `self.state`。

`handle` 在进入转换表前，统一比较当前活动 Session ID 与事件摘要中的路由 Session
ID。两者都存在且不相等时直接进入通用拒绝出口；匹配后，转换表只负责状态、阶段和事件
种类，不在每个分支重复 Session ID guard。`RecorderAlreadyRecording` 等多 ID 事件使用
requested ID 路由，observed active ID 只用于生成下层清理 Effect。

不增加独立的 `accepts(event)` 预检查，因为那会与真正的转换表重复维护同一套规则。
一个 match 同时负责“是否允许”和“允许后如何转换”。

### 3. 明确列出所有允许路径

私有转换表只包含以下生命周期路径：

| 当前状态 | 接受事件 | 下一状态 |
| --- | --- | --- |
| `Idle` | `Start` / `Toggle` | `Starting(Recorder)` |
| `Starting(Recorder)` | 匹配 ID 的 `RecorderStarted` | `Starting(Orchestrator)` |
| `Starting(Recorder)` | 匹配 ID 的 Recorder 启动失败 | `Idle`，并执行现有清理 Effect |
| `Starting(Orchestrator)` | 匹配 ID 的 `OrchestratorStarted` | `Recording` |
| `Starting(Orchestrator)` | 匹配 ID 的 Orchestrator 启动失败 | `Idle`，并执行现有清理 Effect |
| `Recording` | 匹配 ID 的 `ChunkReady` | 保持 `Recording` 并提交 chunk |
| `Recording` | 当前模式允许的 `Stop` / `Toggle` | `Stopping(Recorder)` |
| `Stopping(Recorder)` | 匹配 ID 的 `RecorderStopped` / `RecorderNotRecording` | `Stopping(Orchestrator)` |
| `Stopping(Recorder)` | 匹配 ID 的 `RecorderStillRecording` | 回到 `Recording` |
| `Stopping(Orchestrator)` | 匹配 ID 的 `OrchestratorFinished` | `Idle` |
| 除 `ShuttingDown` 外的任意状态 | `ShutdownRequested` | `ShuttingDown` |

session ID 不匹配由统一路由门拒绝；阶段不匹配、当前模式不接受的控制、重复控制及关闭
后的任何事件不会出现在允许路径中，并统一落入最后的 `_ => None`。两层拒绝都回到
`handle` 的同一个异常出口。

### 4. 删除没有持久语义的 `Recovering` 状态

当前 `RecorderAlreadyRecording` 分支先把状态设为 `Recovering`，随后在同一次
`handle()` 调用内立即改回 `Idle`，外部永远观察不到 `Recovering`。这不是一个真实的
生命周期状态，反而让有限转换路径更难理解。

本次重构删除 `RecordingState::Recovering`。对应异常仍然从 `Starting` 直接转换到
`Idle`，并保持当前 `CancelRecorder`、`AbortOrchestrator` 和托盘重置 Effect 的顺序。
这只去除不可观察的中间赋值，不改变运行行为。

### 5. 一个通用的轻量异常处理出口

`handle` 在调用私有转换函数前，从事件取得一个不包含载荷的摘要；摘要中的可选 ID
同时用于统一 Session 路由：

```rust
fn summary(&self) -> (&'static str, Option<SessionId>);
```

转换成功时，`handle` 写入下一状态并返回 Effect。转换返回 `None` 时，`handle` 保持状态
不变，输出一条 `debug!` 日志，然后返回空 Effect：

```rust
debug!(
    state = ?self.state,
    event,
    session_id = ?session_id,
    "Recording session event rejected",
);
```

摘要只记录事件名称和可选 session ID，不格式化 `WavChunk`、错误正文或其他大载荷。
所有异常路径共用这一处逻辑，不增加原因枚举，也不让调用方处理被拒绝事件。

已接受但没有 Effect 的转换，例如 `OrchestratorFinished -> Idle`，仍通过
`Some(Transition { effects: Vec::new(), ... })` 表示，因此不会被错误地记录为异常。

## 文件改动

| 文件 | 计划改动 |
| --- | --- |
| `src/core/recording_session.rs` | 增加统一 Session 路由门、私有显式转换表和事件摘要，删除无持久语义的 `Recovering`，统一记录被拒绝事件，并更新测试。 |
| `docs/architecture/core.md` | 记录单入口、显式允许路径和统一异常出口。 |
| `docs/plan/21-lightweight-recording-transitions.md` | 记录获批设计，并在实现后更新状态。 |
| `changelog` | 记录不改变行为的状态机结构简化。 |

`src/main.rs`、依赖和配置均不计划修改。

## 测试驱动的实现顺序

获得明确的计划批准后：

1. 先增加聚焦测试，证明允许路径能够转换到预期状态并保持现有 Effect 顺序。
2. 增加拒绝路径测试，证明 session ID 不匹配、阶段不匹配、不接受的控制和关闭后事件
   都保持原状态且不产生 Effect。
3. 增加回归测试，证明已接受但没有 Effect 的 `OrchestratorFinished` 能正常进入
   `Idle`，不会与拒绝路径混淆。
4. 实现统一 Session 路由、私有 `Transition`、显式 `(state, event)` 转换表和通用异常出口，使测试通过。
5. 删除不可观察的 `Recovering` 状态，并保持对应清理 Effect 不变。
6. 更新架构文档、本计划状态和 `changelog`。
7. 运行格式检查、聚焦测试、完整测试、拒绝警告的 Clippy，以及最终 diff 检查。

日志本身不做脆弱的文本匹配测试；测试状态和 Effect 结果即可证明异常出口不会改变业务
行为。

## 验证方式

```bash
cargo fmt --check
cargo test core::recording_session::tests
cargo test
cargo clippy -- -D warnings
```

测试保持确定性并独立于平台，不访问麦克风、托盘、网络或真实计时。

## 验收标准

- `handle(SessionEvent)` 是外部改变录音状态的唯一方法。
- Session ID 只在进入转换表前统一比较一次，多 ID 事件明确选择路由 ID。
- 每一条允许的状态转换都在一个私有 `(state, event)` match 中显式列出。
- 未列出的路径统一保持状态不变、返回空 Effect，并产生一条轻量 debug 日志。
- 日志只包含当前状态、事件名称和可选 session ID，不包含音频或错误大载荷。
- `Recovering` 不再作为不可观察的伪状态存在。
- 已有允许路径、Effect 顺序和用户行为保持不变。
- 不增加公共转换结果类型、不修改 `drive_session`，也不引入 typestate。
- 聚焦测试、完整测试、格式检查和 Clippy 全部通过。
