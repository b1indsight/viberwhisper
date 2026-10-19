# 60 - Prompt-lab 扫描与评测语言简化 — Feature record

**Stage:** Published for review; local validation and standard workflow review passed; hosted CI pending.
**PR:** [#155](https://github.com/b1indsight/viberwhisper/pull/155)，`refactor/prompt-lab-scan-language` → `master`。

## 1. Initial plan

### Summary, scope and success criteria

用户在[架构审阅线程](https://ampcode.com/threads/T-01a153fd-0936-75aa-a1ea-de312a192fd2)
中选择先实现建议 2、3：数据集校验与评分共用一次扫描，删除 `EvaluationRequest`
中重复的 `language`。不修改增量后处理或文字投递接口，不新增依赖或配置、数据集、报告 schema。

- 私有扫描同时返回原有校验报告与按文件名排序的有效 ready 样本；`validate` 取报告，
  `scoring_snapshot` 用同批样本计算原有 digest。保留诊断、计数、孤立音频和链接边界。
- `list_samples` 保持独立的遇错即失败行为，不强行并入容错校验扫描。
- HTTP 与 Realtime 评测只从 `request.stt.language` 取得合并语言，报告仍保存同一份 STT 元数据。

### Design decisions and validation

复用现有校验循环，避免新建泛化扫描框架或缓存。只读校验会暂时收集 ready 样本后丢弃，
以换取两个消费者共享完整校验规则；不增加控制参数或第二条扫描路径。
数据集继续按单进程所有权约定使用，不扩展并发文件替换防护。本次不宣称总耗时或 I/O 减半。

使用现有行为测试与临时前后对照，不为纯重构增加永久测试。验收包括混合数据集诊断、
ready 顺序与 digest 相同，中文/非中文 HTTP 和 Realtime 多段合并与报告语言一致，
以及 fmt、测试、build、Clippy。本次删除重复状态和扫描，没有新增需要消融的实质机制。

## 2. PR change timeline

### [001] — 2026-10-19 — 共享校验扫描，统一语言来源

- **Implemented changes:** `dataset.rs` 的私有 `scan` 返回校验报告和 ready 样本；评分删除
  第二次枚举、解析及音频校验。`regression.rs` 删除请求的独立语言字段及相等校验，
  两条执行路径使用 STT 快照中的语言。CLI 构造和既有测试辅助构造同步删除重复赋值。
  更新架构说明与 changelog。基于已合并的 PR #154 创建隔离工作区，主工作区未改动。
- **Evidence / validation:** `cargo fmt --check`、`cargo test --locked --offline --all-targets
  --features integration`（313 通过、2 忽略）、`cargo build --locked --offline`、
  `cargo clippy --locked --offline --all-targets --features integration -- -D warnings` 均通过。
  既有 rdev 依赖 FFI/static-mut 警告仍在。实际 CLI 前后对照使用同一混合夹具，含 ready、
  pending、损坏 JSON、摘要不匹配、非 JSON、外部 sidecar 链接和孤立音频，诊断完全一致；
  ready IDs、顺序和评分 digest 一致。4 种语言（zh、zh-CN、en、未指定）的两分块 HTTP
  评测经 32 次回环模拟请求，去掉运行 ID/时间后报告完全一致。临时 Realtime 探针通过真实
  evaluate 入口处理相同 4 种语言、每次两段，检查合并文本及报告重新读取/回放，全部通过；
  探针接入口已移除，不保留在生产或测试代码中。
- **Limits / pending:** 未访问个人配置、录音或真实远端 STT。Windows 原生运行未执行。
  已调用本地 Claude Code Opus 5.5 复审；初始化确认 tools/MCP 为空，但随后返回
  `rate_limit`，进程退出 1、最终 `is_error=true`，没有模型审阅结果。CLI 提示新加坡时间
  22:40 重置会话额度；未切换模型或计费渠道。
- **Final review:** 用户随后指定收尾审阅使用当前仓库工作流，只有明确要求时才使用 Claude。
  已运行 `plan-first-workflow/scripts/review.sh @`，通过独立只读 Codex 审阅，退出 0，
  结构化结果为 `findings: []`。该审阅未运行测试或修改源码；测试证据来自上述本地验证。
  发布前对[最终代码提交](https://github.com/b1indsight/viberwhisper/commit/c26293d5025af0a786c5ae60b279258bcd334987)
  再次运行同一工作流，退出 0、无 findings；fmt、build、313 项测试与 Clippy 再次通过。
  随后推送同一个 bookmark 并创建非草稿 PR #155。审阅后仅补录本文档中的发布链接及结果，
  未改动源码；GitHub macOS/Windows CI 尚待完成，未执行合并。
