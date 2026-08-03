# Rust 2018 风格模块入口迁移

## 状态

已实现。九个模块入口已迁移为同级 `<module>.rs`，当前结构文档已同步。

## 背景

迁移前，仓库有九个目录模块以 `mod.rs` 作为入口。Rust 2018 及后续 edition 支持把
`foo/mod.rs` 写成同级的 `foo.rs`，同时继续把子模块放在 `foo/` 目录中。项目已使用
Rust 2024，因此不需要兼容旧 edition 的查找规则。

本次迁移统一采用新式布局，消除仓库中的 `mod.rs`，但不改变模块名、可见性、公开 API、
条件编译或运行时行为。

## 目标与边界

目标：

1. 把 `src/` 下所有九个 `mod.rs` 迁移为对应的同级 `<module>.rs`。
2. 保留每个入口文件的内容和子模块声明，仅改变文件系统位置。
3. 同步描述当前源码结构的仓库文档。
4. 通过编译、测试、lint 和文件搜索确认迁移完整。

不在范围内：

- 不拆分较大的模块入口文件，也不调整其职责。
- 不改变 `mod` / `pub mod` 声明、re-export、类型可见性或调用路径。
- 不重写历史计划中的旧路径；这些文档保留当时的设计与实现记录。
- 不改变配置、依赖、CLI、用户行为或发布产物。

## 目标布局

入口文件按以下方式一一迁移：

| 当前路径 | 目标路径 |
|---|---|
| `src/application/mod.rs` | `src/application.rs` |
| `src/audio/mod.rs` | `src/audio.rs` |
| `src/core/mod.rs` | `src/core.rs` |
| `src/core/config/mod.rs` | `src/core/config.rs` |
| `src/input/mod.rs` | `src/input.rs` |
| `src/local/mod.rs` | `src/local.rs` |
| `src/platform/mod.rs` | `src/platform.rs` |
| `src/postprocess/mod.rs` | `src/postprocess.rs` |
| `src/transcriber/mod.rs` | `src/transcriber.rs` |

例如，`src/core.rs` 继续声明 `config`、`orchestrator` 和 `recording_session`；其中
`src/core/config.rs` 继续声明 `document`、`fields` 和 `store`。Rust 允许 `core.rs` 与
`core/` 并存，也允许 `core/config.rs` 与 `core/config/` 并存，因此嵌套模块无需更改
Rust 路径。

## 实施顺序

1. 记录迁移前的九个 `mod.rs` 清单，防止遗漏嵌套入口。
2. 将每个入口文件机械迁移到上表中的目标路径，不修改文件内容。
3. 搜索源码和当前结构文档，确认不再把现行入口描述为 `mod.rs`。
4. 更新 `AGENTS.md` 的项目结构，以及直接展示入口文件的 architecture 文档。
5. 运行格式、编译、测试和 lint 验证；由 GitHub Actions 再验证 Windows 构建与测试。

## 测试策略

这是纯模块文件布局迁移，不增加行为测试。Rust 编译器会解析完整模块树，现有测试会覆盖
模块引用和公开接口未被破坏：

- `rg --files -g 'mod.rs'` 必须无输出；
- `cargo fmt --check`；
- `cargo build`；
- `cargo test`；
- `cargo clippy -- -D warnings`；
- PR 上现有 macOS 与 Windows GitHub Actions 必须通过。

## 文档影响

实施阶段预计更新：

- `AGENTS.md`：把项目结构中的现行模块入口改为新路径；
- `docs/architecture/core.md`：把 config facade 从 `mod.rs` 改为 `config.rs`；
- `docs/architecture/postprocess.md`：把模块入口从 `postprocess/mod.rs` 改为
  `postprocess.rs`；
- 本计划：实施完成后把状态更新为已实现，并记录验证结果或实质偏差。

`docs/plan/` 下已有计划中的 `mod.rs` 路径不会批量改写，因为这些文件是历史决策记录，
不是当前源码结构清单。`changelog` 也无需更新：本次不改变用户可见行为、配置、接口或发布
流程。

## 验收标准

- 仓库中不存在 `mod.rs`。
- 九个模块入口均位于对应的 `<module>.rs`，模块内容和 Rust 路径保持不变。
- 当前结构文档不再把现行入口描述为 `mod.rs`。
- macOS 本地验证与 PR 的跨平台 CI 全部通过。

## 实施结果

九个入口均按目标布局完成迁移。由于 `include_str!` 相对声明宏的源文件解析路径，
`src/core/config.rs` 中四处测试 fixture 路径由 `../../../config.example.json` 调整为
`../../config.example.json`；fixture 和测试语义没有变化。除此之外，模块入口内容、
Rust 模块路径和公开接口均保持不变。

本地验证结果：

- `rg --files -g 'mod.rs'`：无输出；
- `cargo fmt --check`：通过；
- `cargo build`：通过；
- `cargo test`：115 个测试通过；
- `cargo clippy -- -D warnings`：通过。

Windows 构建与测试由本 PR 的 GitHub Actions 验证。
