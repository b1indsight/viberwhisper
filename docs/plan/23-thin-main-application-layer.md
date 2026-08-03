# 23 - 薄入口与轻量应用层

## 状态

已实现。`src/main.rs` 已收敛为三行委托入口，library 模块根和轻量 application/listener
边界已经落地，现有 115 个测试保持通过。

## 背景

当前 `src/main.rs` 超过 800 行，同时承担模块注册、日志初始化、CLI 分发、配置访问、Local
服务生命周期、常驻 listener、Session Effect 执行、离线转写和测试。它实际上已经是一个未命名的
应用层，而不是进程入口。

第一版计划按每条命令拆分多个 application 文件并移动 `runtime_config`，虽然边界完整，但对当前代码量
略显重型。本修订只分离真正复杂且独立变化的 listener，其余较短工作流继续放在一个应用入口模块中。

## 目标

1. 将 `src/main.rs` 收敛为只调用 `viberwhisper::run()` 的薄入口。
2. 新增 `src/lib.rs` 作为唯一 Rust 模块根，并只公开应用启动函数。
3. 新增轻量 `application` 模块承载日志初始化、CLI 分发以及 config/local/convert 工作流。
4. 单独提取复杂的 listener loop、输入归一化和 Session Effect 执行。
5. 保持 CLI、日志、录音、Session 路由、Local 服务、转写、后处理和文本注入行为不变。
6. 保留现有测试覆盖，不为纯文件移动增加低价值抽象或测试。

## 非目标

- 不移动 `src/runtime_config.rs`；它已经是清晰、独立的应用级配置装配模块。
- 不为 config、local、convert 分别创建文件；只有出现独立增长需求时再拆。
- 不改变 `RecordingSessionMachine`、`SessionOrchestrator`、`AudioRecorder` 或 `SessionId` 的 API/语义。
- 不改变 20ms listener 间隔、事件处理顺序、托盘状态、启动回滚或停止收敛逻辑。
- 不改变 CLI 命令、参数、输出文字、配置 schema、默认值或错误降级策略。
- 不引入 async runtime、event bus、service locator、依赖注入框架或新的运行时 trait 层。
- 不设计新的通用错误体系；继续使用当前 `Result<_, Box<dyn Error>>` 边界。
- 不修改 Python server、依赖、打包或 GitHub Actions。

## 目标结构

```text
src/
  main.rs                  — 进程入口，只调用 viberwhisper::run()
  lib.rs                   — 唯一模块根，只公开 application::run
  application/
    mod.rs                 — 日志、CLI 分发、config/local/convert 和共享 helper
    listener.rs            — listener loop、输入归一化、Effect 执行和结果交付
  runtime_config.rs        — 保持原位
  audio/
  core/
  input/
  local/
  platform/
  postprocess/
  session.rs
  text.rs
  transcriber/
```

这个结构只增加一个必要的 library 边界和一个 listener 文件。`application/mod.rs` 预计承载约 300 行
较短、低耦合的命令工作流；`listener.rs` 承载约 400 行需要独立审阅的实时流程。以后只在其中一部分
形成新的独立变化轴时继续拆分。

## 入口边界

最终 `src/main.rs` 只保留：

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    viberwhisper::run()
}
```

所有 `mod` 声明移动到 `src/lib.rs`。内部模块保持 crate 私有，library 只公开一个稳定入口：

```rust
pub use application::run;
```

`application::run()` 保持当前顺序：

1. 初始化 `tracing_subscriber` 和默认 `viberwhisper=info` filter；
2. 使用 Clap 解析 `Cli`；
3. 根据 `Commands` 分发到 listener、config、local 或 convert；
4. 原样向 `main()` 返回错误。

日志初始化仍是应用入口唯一产生的全局副作用，不移动到静态对象或模块构造过程。

## `application::mod` 职责

从现有 `main.rs` 移入：

- `run()` 与 CLI dispatch；
- `handle_config()`；
- `handle_local()`、`LocalServiceGuard`、安装准备和 Local backend 启动；
- `handle_convert()`；
- `load_config()`、`config_context()` 和 packaged server 文件定位 helper。

这些函数虽然服务不同命令，但总计规模有限，共享 ConfigStore 和 Local backend 生命周期，并且都只从
顶层 CLI 分发调用。先放在同一模块比建立多个只包含单个函数的文件更直接。

## `application::listener` 职责

从现有 `main.rs` 移入：

- `run_listener()` 与 `run_listener_with_config()`；
- `RecordingInput` 与 `normalize_recording_input()`；
- `drive_session()`；
- `finish_transcription()`。

迁移只调整模块路径和可见性，保留：

- Tray action、Hotkey event、ready chunk 的轮询顺序；
- 20ms sleep 与 heartbeat；
- source-specific 输入止于应用边界的规则；
- recorder/orchestrator 原子启动和失败回滚；
- stop-time 尾片顺序、转写收敛、partial text 注入和 shutdown 清理。

`application::mod` 通过窄函数调用 listener，不接触 listener 内部状态；listener 可以调用同一父模块中的
配置加载和 Local backend helper，不增加 facade 或 trait。

## 依赖方向

```text
main.rs
  -> viberwhisper::run
       -> application::run
            -> application::listener
            -> runtime_config
            -> core / audio / input / local / platform / postprocess / transcriber
```

能力模块不反向依赖 `application`。`main.rs` 不再直接依赖业务模块，`lib.rs` 只声明模块图，不承载
CLI 或 listener 分支。

## 测试处理

本次是结构重构，不访问麦克风、托盘、网络或真实 Local 模型，也不为委托函数增加源代码形状测试。

- 输入归一化测试随函数移动到 `application::listener`；
- 当前 mock pipeline 和 orchestrator smoke tests 移到 `application` 测试模块；
- `runtime_config` 及其他模块测试保持原文件和断言不变；
- 当前共 115 个测试不因拆分删除。

library/binary 边界由 `cargo check --all-targets`、`cargo test` 和 Clippy 同时编译验证。

## 实施顺序

计划再次批准后：

1. 运行完整测试，记录最新 `master` 基线。
2. 先将现有 main 集成测试迁到目标 application 测试位置，保持断言不变。
3. 新增 `src/lib.rs` 和 `src/application/mod.rs`，移动模块声明、日志初始化和 CLI dispatch。
4. 将 config/local/convert 工作流及共享 helper 机械移动到 `application/mod.rs`。
5. 新增 `application/listener.rs`，完整移动 listener、输入归一化、Effect driver 和结果处理。
6. 将 `src/main.rs` 收敛为单一委托函数，搜索确认其中不再存在内部模块或业务 helper。
7. 只更新仍把集成职责归给 `main.rs` 的架构文档、项目结构说明和 changelog。
8. 对照原实现逐段审查，确认没有重排条件分支、错误处理、日志或副作用顺序。
9. 运行完整验证和独立代码审查门禁，在同一 bookmark/PR 上推送实现。

## 文件影响范围

| 文件 | 计划变更 |
|---|---|
| `src/main.rs` | 替换为对 library `run()` 的单一调用。 |
| `src/lib.rs` | 新增唯一模块根并公开应用入口。 |
| `src/application/mod.rs` | 新增应用启动、CLI 命令工作流和共享 helper。 |
| `src/application/listener.rs` | 新增 listener、输入归一化、Effect driver 和结果处理。 |
| `docs/architecture/core.md`, `input.md`, `platform.md` | 将过时的 `main.rs` 集成描述更新为 application 路径。 |
| `AGENTS.md`, `changelog` | 更新项目结构和变更记录。 |

`src/runtime_config.rs`、能力模块、`Cargo.toml`、配置、依赖和 Python server 均保持不变。

## 验证方式

```bash
cargo fmt --check
cargo check --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
```

CI 继续负责 macOS 和 Windows target 构建；本地验证不启动 GUI、麦克风、网络或模型。

## 验收标准

- `src/main.rs` 只调用 `viberwhisper::run()`，不含模块声明、CLI match 或业务 helper。
- `src/lib.rs` 是唯一模块根，只向 binary 暴露应用启动函数。
- `application/mod.rs` 承载较短命令工作流，`application/listener.rs` 独立承载实时流程。
- `src/runtime_config.rs` 保持原位且逻辑不变。
- 日志初始化、CLI 分发、用户可见输出和错误传播不变。
- listener 的轮询顺序、20ms 间隔、Session Effect 执行和关闭行为不变。
- Local backend 的安装、启动、复用、释放和状态行为不变。
- convert 的 chunk 顺序、文本合并、后处理降级和输出行为不变。
- 当前测试覆盖得到保留，完整测试、all-target check、格式和拒绝 warning 的 Clippy 全部通过。
- 架构文档不再把业务集成职责描述为 `main.rs` 所有。
