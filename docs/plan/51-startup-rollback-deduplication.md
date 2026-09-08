# 启动回滚去重

## 状态与范围

计划草案，等待批准后在同一 bookmark 和 PR 上实现。
基于 `master` 的 `d7f479b6`，此前的模块迁移已在 PR #133 合入。

只简化 `src/ui/listener/event_loop.rs` 中 `ListenerApplication::start_session`。
以减少嵌套和重复代码为目标，保持启动、失败恢复和会话路由行为。

## 实现方式

1. 录音器启动失败或已有录音时，执行原有处理后提前返回；成功后顺序执行
   orchestrator 启动和输出启动，展开目前的嵌套 `match`。
2. 最多提取两个私有方法，分别复用取消录音器、终止 orchestrator 的操作与日志。
   各启动失败分支继续明确传入相应 `SessionId`，capture 的取消保持原有位置。
3. 公共 debug 日志可统一文案，保留资源操作结果、会话 ID 和分支的失败原因日志。
   资源清理顺序不变，不为维持细微的 debug 文案或输出时序增加参数。

现有分支的行为约束如下。表中的“本次”与“已有”会话可能具有不同 ID：

| 场景 | 资源清理顺序 | 返回事件 |
| --- | --- | --- |
| 录音器启动失败 | 不增加上层清理 | 对本次请求返回 `SessionStartFailed` |
| orchestrator 已有会话 | 取消本次录音器 → 取消已有 capture → 终止已有 orchestrator | 对本次请求返回 `SessionStartFailed` |
| capture 启动失败 | 取消本次录音器 → 终止本次 orchestrator；不增加 capture 取消 | 对本次请求返回 `SessionStartFailed` |
| 录音器已有会话 | 取消已有 capture → 取消已有录音器 → 终止同 ID 的 orchestrator | 对本次请求返回 `SessionStartFailed` |
| 全部启动成功 | 不清理 | 返回 `SessionStarted` |

不引入统一的 `rollback(session_id)`、回滚配置对象、RAII guard、新 trait 或状态机。
停止和退出流程、底层资源接口、错误类型与启动顺序保持现有实现。
预计源码净减少十几到二十来行，最终以实际 diff 为准，不为行数压缩可读代码。

## 文件与实施顺序

1. 记录现有测试基线，按上表核对当前失败分支与会话 ID 来源。
2. 在 `src/ui/listener/event_loop.rs` 完成局部重构，逐分支对照资源操作与返回事件。
3. 更新 `changelog` 和本计划的实施状态，在同一 bookmark / PR 上提交实现。

## 验证

- 复用现有监听器、录音状态机、录音器与 orchestrator 测试，重点检查启动失败恢复、
  会话 ID 不匹配时的保护及取消行为。
- 不为本次去重创建原生录音器 mock 框架，不增加只验证包装方法调用的测试。
  如果实施发现必须改变可观察行为，先补对应的最小回归测试并明确该变化。
- 对照上表审阅全部分支，确保没有把“本次请求 ID”与“已有资源 ID”合并。
- 运行 `cargo fmt --check`、`cargo build --locked`、`cargo test --locked`、
  `cargo clippy --locked -- -D warnings`，并通过现有 macOS / Windows CI。
- 代码推送通过仓库独立评审门禁；记录验证结果和源码实际净变化。
