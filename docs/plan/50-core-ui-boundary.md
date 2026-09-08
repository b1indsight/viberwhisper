# 合并 application 与 core，分离桌面交互

## 状态与范围

用户已批准精简方案；实现已在同一 bookmark 和 PR #133 上完成，本机验证结果记录
在文末，独立评审与托管 CI 结果汇总在 PR 中。

基于 `master` 的 `4b899ce8`（音频接口简化，PR #132）。延续计划 23 的薄进程入口、
计划 28 的主线程事件循环，以及计划 41 的首次启动向导；本计划调整它们的模块归属。

将 `application` 的启动、命令执行和会话编排合入 `core`，把桌面交互独立为 `ui`。
草案将“交互”解释为配置对话框、热键捕获与验证、托盘交互和 winit 事件适配。
CLI 参数定义、命令分发与终端输出继续由 `core` 管理。

实施约束：尽量少新增代码，以移动现有实现、修改引用和必要的可见性调整为主。
允许桌面交互模块保留现有业务调用的串联代码，避免为目录分层增加中间接口。

本次以保持行为的结构重构为目标。现有录音、转写、历史、配置保存与退出策略保持一致；
setup 超时、离线转写并发和后处理策略调整留待单独变更。

## 目标结构

沿用 Rust 2018 模块文件布局，保留一个 crate：

```text
src/
  lib.rs                         # 模块声明；从 core 导出 run / run_desktop
  core.rs                        # 模块声明、启动、日志、命令分发及 config / convert 执行
  core/
    cli.rs                       # 现有 CLI 参数与命令定义
    config.rs + config/          # 现有配置类型和持久化
    orchestrator.rs              # 现有转写编排
    recording_session.rs         # 现有唯一录音状态机
    listener.rs                  # 移入既有 RecordingConfig / ListenerConfig 及构造逻辑
    prompt_lab.rs                # prompt-lab 命令执行
  ui.rs                          # 桌面交互模块声明
  ui/
    listener.rs                  # 既有监听器启动、服务组装和平台动作映射
    listener/
      event_loop.rs              # 既有事件处理、会话操作与后台完成通知
    setup.rs                     # 完整配置向导、原生对话框与测试录音流程
    setup/
      hotkey.rs                  # 热键 helper 子进程、协议与生命周期
  input/                         # 既有热键映射、托盘组件和 TextTyper 接口
  platform/                      # 既有 macOS / Windows 平台实现
```

删除 `src/application.rs` 和 `src/application/`。`src/main.rs` 与 Windows GUI 入口
继续调用 `viberwhisper::run` / `viberwhisper::run_desktop`，无需感知内部目录变化。
`src/prompt_lab/` 仍是数据集与评分领域实现，`core::prompt_lab` 只负责命令执行。

## 职责与依赖边界

### 启动与组装

`core::run` / `run_desktop` 是组装入口，可以直接调用 `ui` 启动桌面交互。
CLI-only 命令不创建原生事件循环。热键 helper 识别仍先于日志和 CLI 初始化；
Windows 桌面输出准备与原生启动错误提示保留入口对既有 `platform` 能力的调用，
无需增加仅转发一次调用的 `ui` 包装函数。

`ui` 直接调用已有业务接口。`core::config`、`orchestrator`、`recording_session`
及录音配置类型不依赖 `ui`、winit 或原生平台对象。入口负责连接各模块，不为
追求整个目录的单向依赖增加注入框架。

### 监听器

- 将已有 `RecordingConfig` / `ListenerConfig` 及其构造逻辑移入 `core::listener`，
  供配置检查、CLI、setup 和桌面监听器复用，不创建新的配置类型或转换层。
- 将其余监听器启动代码和 `event_loop.rs` 按现有结构移入 `ui::listener`。
  保留现有 `AppEvent`、`ListenerApplication`、事件代理与资源所有权，直接使用
  `core` 中的状态机和 orchestrator，以及既有录音器、后处理、历史与采集能力。
- 录音启动回滚、尾块提交、收尾线程和文本投递保持现有调用链，不单独建立
  `core::listener` 执行器，也不新增托盘动作协议、完成通知接口或平行的事件类型。
- 沿用 `RecordingSessionMachine`、`SessionEvent`、`SessionEffect` 和 `TextTyper`。
  本次不新增 trait、事件总线、服务容器或转发包装层。
- 最终收尾持续在后台执行；状态保持 `Stopping`，直到匹配的完成事件到达。
  退出继续阻止尚未开始的文本投递，并保留不等待已在执行的投递操作的语义。

### 配置向导与验证

- 将 setup 向导及 helper 作为完整桌面交互流程移入 `ui::setup`，继续直接调用
  `core::config` 的读取、字段选择与原子保存能力，以及 `core::listener` 的配置构造。
- 保留现有 `SetupUi` / `SetupVerifier` 及测试替身，保持向导与原生实现现有边界。
  不新增 `core::setup` 与 `ui::setup` 之间的调用协议。
- `NativeVerifier` 保持完整：等待热键、录音、收集音频块、停止和验证不再拆开。
  现有录音器、STT / LLM 接口继续复用，不新增验证录音对象或验证状态机。
- 对话框文本处理、热键协议、读取线程、子进程退出与 Windows 隐藏窗口标志随实现
  一起移动。setup 严格展示验证错误，正常投递保留其既有回退行为。

`input` 和 `platform` 继续提供既有平台组件；`ui` 负责交互组装，不复制其热键、
托盘或文本注入实现。可见性只扩大到跨模块调用实际需要的范围。

## 实施顺序

1. 记录最新基线的测试结果，确定既有测试的目标归属并保留断言。若移动之外的必要
   修改涉及可观察行为，先补最小回归测试；纯移动不新增测试代码。
2. 将启动和命令执行移入 `core`，更新 crate 导出、模块引用和相应单元测试。
3. 将既有监听器配置移入 `core::listener`，其余监听器和事件循环移入 `ui`。
4. 将完整 setup 流程与 helper 移入 `ui`，调整引用；保持保存确认、取消、重试
   和 helper 提前分发的调用顺序。
5. 删除旧 application 路径，检查跨模块可见性和依赖，更新 `AGENTS.md` 模块地图、
   `docs/architecture/core.md`、`platform.md`、`input.md`、`prompt-lab.md` 及相关现行
   文档中的路径，并补充 `docs/architecture/ui.md` 和 `changelog`。历史计划保留原貌。
6. 完成检查与代码评审，在同一 bookmark / PR 上提交实现。

## 验证与验收

- 保留现有 application 范围测试的行为断言，随职责移动到 `core` / `ui`。
- 用已有测试重点验证：托盘与热键输入仍按状态产生动作；旧会话完成通知不能结束
  新会话；退出取消不会等待后台投递；setup 取消不写配置，确认后保存完整候选配置。
  复用已有状态机测试覆盖，不为相同规则重复建测试。
- 复用原有测试与替身，单元测试不启动实际事件循环、麦克风、全局热键或网络服务。
  不新增目录结构断言、静态路径测试或仅为本次移动搭建的测试框架。
- 本机运行 `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、
  `cargo clippy --locked -- -D warnings`。
- 通过现有 macOS 与 Windows CI；Windows 包括 `--features windows-app` 的构建、
  测试以及 `cargo clippy --locked --all-targets --features windows-app -- -D warnings`。
- 验证 CLI 帮助、命令和退出码；具备交互环境时检查首次 setup、热键 helper、托盘
  录音/历史/退出以及 prompt-lab capture。不能自动执行的原生检查明确记录状态。
- 验收时 `src/application*` 已移除；既有业务接口不新增原生 UI 类型；现有公开
  入口、配置格式、命令语义、平台能力和用户可见流程保持兼容。
- 检查 diff 时区分代码移动与实际新增逻辑；新增内容应限于必要的模块接线，
  不以减少表面行数为由压缩现有可读代码或删除有价值的测试。

## 实现进度

- 启动与命令执行移入 `core`；既有录音配置及其测试移入 `core::listener`。
- 桌面监听器、setup 与 helper 移入 `ui`。事件循环和热键 helper 文件内容完全保留，
  沿用既有接口、事件与测试，未增加业务逻辑或运行时抽象。
- Rust 源码净增 16 行，来自模块说明、导入与测试模块结构调整。
- 基线与迁移后完整测试均为 187 项通过；本机格式、构建及 Clippy（警告视为错误）
  检查通过，CLI 主命令与 prompt-lab 帮助均成功退出。
- 独立代码评审及 macOS / Windows 托管 CI 用于最终验收，结果见
  [PR #133](https://github.com/b1indsight/viberwhisper/pull/133)。
- 本次未执行真实麦克风、全局热键、桌面托盘或外部 API 的交互检查。
