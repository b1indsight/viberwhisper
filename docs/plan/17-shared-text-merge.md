# 共享转录文本合并工具

## 状态

**已实现。**

`merge_texts` 已集中到 `src/text.rs`，离线 WAV 分片转录与 session orchestrator
复用同一实现。一条代表性测试验证非中文片段使用空格连接。

## 背景

目前有两条分块转录路径需要把按顺序产生的文本片段合并为最终文本：

- `src/transcriber/api.rs` 的离线 WAV 分片转录路径。
- `src/core/orchestrator.rs` 的录音 session 收敛、部分失败和超时路径。

两个模块分别定义了私有的 `merge_texts`。两份实现只有局部变量名不同，行为相同，测试也有重复。未来修改语言分隔规则时，可能只更新其中一处，导致两条转录路径产生不同结果。

## 目标

1. 在 `src/text.rs` 中提供唯一的转录文本合并实现。
2. 让离线分片转录和 session orchestrator 复用该实现。
3. 集中维护合并规则的单元测试。
4. 保持现有输出、错误处理和模块公开 API 不变。

## 非目标

- 不改变中文或其他语言的分隔规则。
- 不增加片段去空白、标点修复、语言标准化或大小写标准化。
- 不改变分片顺序、转录请求、重试、收敛超时或部分失败语义。
- 不把该函数公开为 crate 外部 API。
- 不在本次重构中引入通用 `util` 模块或新的依赖。

## 设计

在 crate 根模块声明新的私有模块：

```rust
mod text;
```

`src/text.rs` 提供 crate 内共享函数：

```rust
pub(crate) fn merge_texts(texts: &[String], language: Option<&str>) -> String;
```

函数保持当前规则：

1. `language` 为 `Some(lang)` 且 `lang.starts_with("zh")` 时，不插入分隔符。
2. 其他语言以及 `None` 使用单个空格分隔。
3. 合并前过滤 `String::is_empty()` 为真的片段。
4. 保持输入片段原始顺序。

这意味着判断继续区分大小写；例如 `zh-CN` 使用空分隔符，而 `ZH-CN` 仍使用空格。只包含空白的字符串不视为空片段。本次重构不修正或扩展这些既有语义。

调用方统一使用：

```rust
use crate::text::merge_texts;
```

`src/transcriber/api.rs` 和 `src/core/orchestrator.rs` 中的本地实现将被删除。共享函数维持 `pub(crate)` 可见性，避免形成不必要的外部接口。

## 文件改动

| 文件 | 计划变更 |
|---|---|
| `src/main.rs` | 声明 `mod text;` |
| `src/text.rs` | 新增共享 `merge_texts` 及集中单元测试 |
| `src/transcriber/api.rs` | 导入共享函数，删除本地实现和重复的纯函数测试 |
| `src/core/orchestrator.rs` | 导入共享函数，删除本地实现和重复的纯函数测试 |
| `docs/architecture/transcriber.md` | 将语言感知合并逻辑的位置更新为 `src/text.rs` |
| `docs/plan/06-end-to-end-stream-recognition.md` | 记录 orchestrator 复用共享文本模块的后续调整 |
| `docs/plan/17-shared-text-merge.md` | 实现完成后更新状态和实际差异 |
| `changelog` | 记录合并逻辑去重 |

## TDD 实施顺序

计划批准后严格按以下顺序实施：

### Phase 1：共享模块测试

先创建 `src/text.rs` 并编写一条单元测试，验证非中文片段以一个空格连接。

此时先运行目标测试并确认因为共享实现尚未完成而失败，再实现函数使测试通过。

### Phase 2：迁移调用方

1. 在 `src/main.rs` 注册 `text` 模块。
2. 将两个调用方改为导入共享函数。
3. 删除两个本地 `merge_texts` 实现及重复的纯函数测试。
4. 运行 transcriber 和 orchestrator 的现有测试，确认两条业务路径行为不变。

### Phase 3：文档与验证

1. 更新架构文档、相关既有计划文档、本计划状态和 `changelog`。
2. 运行 `cargo fmt --check`。
3. 运行 `cargo check`。
4. 运行 `cargo test`。
5. 检查最终 `jj diff`，将实现推送到本计划使用的同一个 bookmark 和 draft PR。

## 实际实现说明

- 首先添加 `src/text.rs` 的规则测试并运行目标测试，确认因共享函数尚不存在而编译失败。
- 实现 crate 内可见的 `merge_texts` 后，将两个调用方迁移到共享函数。
- 保留一条非中文空格连接测试，避免为这个小型辅助函数维护重复用例。
- 删除 `api.rs` 与 `orchestrator.rs` 中重复的函数定义和纯函数测试。

## 验收标准

- [x] crate 中只存在一个 `merge_texts` 函数定义。
- [x] 离线 WAV 分片路径使用 `crate::text::merge_texts`。
- [x] session 正常完成、部分失败和超时路径使用同一个共享函数。
- [x] 现有语言分隔、空字符串过滤和顺序语义保持不变。
- [x] 共享函数仅在 crate 内可见。
- [x] 合并规则测试集中在 `src/text.rs`。
- [x] `cargo fmt --check`、`cargo check` 和 `cargo test` 全部通过。
- [x] 架构文档、计划状态和 `changelog` 与实现一致。

## 风险与回滚

本次变更不修改业务算法，主要风险是遗漏某个调用点或模块声明。编译检查和两条路径的现有测试可以覆盖这些问题。若迁移导致回归，可在同一 PR 中恢复调用方的本地实现；不涉及配置、数据格式或持久化迁移。
