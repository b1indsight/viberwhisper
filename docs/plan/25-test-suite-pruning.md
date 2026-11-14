# 测试套件价值与确定性整理

## 状态

已实现。20 个多余测试定义已删除，两组同契约测试已合并，保留测试的时间、端口和临时
目录依赖已按本文方案隔离。

## 背景

当前 macOS 基线包含 115 个 Rust 单元测试。Python 测试套件收集 20 个测试，其中 18 个
默认通过，2 个真实模型集成测试按环境变量显式跳过。`server/test_asr_client.py` 是手动
调用本地 ASR 接口的 CLI 工具，不由 pytest 收集，因此不属于自动测试套件。

现有测试全部通过，但逐项对照 `code-principles` 的 Testing Principles 后，发现三类
需要整理的问题：

1. 多个上层测试直接重复模块内已经覆盖的相同行为，或只证明测试替身会返回成功。
2. 一批 Clap 快乐路径测试逐个验证同一种派生解析机制；一个代表性用例已经能够保护
   每类命令形状。
3. 少数测试依赖真实音频设备枚举、主机 Python、固定 TCP 端口或真实退避睡眠，降低了
   确定性和速度；其中弱测试应删除，有独立行为价值的测试应改用受控替身。

本次整理不以覆盖率数字或测试数量为目标。配置安全、持久化、API 协议、音频/WAV
边界、Session 路由、状态机、并发、超时、跨平台命令构造和显式真实模型集成测试均保留。

## 目标与边界

目标：

1. 删除不保护独立行为、被更强测试完全覆盖或依赖不受控运行环境的多余测试。
2. 保留一个代表性快乐路径，同时保留错误边界、可信回归和高风险逻辑的测试。
3. 让仍有价值的测试不依赖固定端口、主机 Python、无必要的真实睡眠或共享临时路径。
4. 保持产品的用户可见行为、公开接口、配置格式和 Python 服务行为不变。

不在范围内：

- 不追求任意覆盖率阈值，也不为被删测试逐一补写等价测试。
- 不重构与测试确定性无关的业务代码。
- 不运行默认跳过且需要本地 Gemma 权重的真实模型测试；继续验证其收集和跳过条件。
- 不把手动 ASR 客户端改造成 pytest 测试。

## 审计结论与删除清单

预计从源码删除 20 个 Rust 测试，并把两组相同契约的测试各合并为一个表驱动测试。
由于 macOS/Windows 平台目录测试互斥，且真实 recorder 构造测试不在 Windows 收集，
macOS 实际减少 19 个删除项、Windows 实际减少 18 个删除项；完成两组合并后，两端预计
均收集 94 个默认 Rust 测试。删除按原因分组如下。

### 重复或被更强测试覆盖

| 模块 | 删除测试 | 保留的证明 |
|---|---|---|
| `application` | `test_full_pipeline_mock` | 该测试只串联两个永远成功的测试替身；真实编排行为由 listener、orchestrator 和 postprocess 测试保护 |
| `application` | `test_orchestrator_integration_single_chunk` | `core::orchestrator::test_single_chunk_success` 覆盖同一路径并断言准确文本 |
| `application` | `test_orchestrator_no_chunks` | `core::orchestrator::test_no_chunks_returns_error` 覆盖相同错误契约 |
| `audio::recorder` | `test_stop_recording_splits_unflushed_ready_chunks` | `stop_time_chunk_catch_up_clears_readiness_and_stops_polling` 使用相同输入并额外验证清理不变量 |
| `core::orchestrator` | `test_multi_chunk_ordered_merge` | `multi_chunk_results_remain_index_ordered` 直接覆盖乱序结果合并；单片端到端测试覆盖 worker/session 路径 |
| `core::orchestrator` | `test_session_lifecycle_is_mode_free` | `test_single_chunk_success` 覆盖相同生命周期；API 不再含 mode 由编译器直接保证 |
| `core::orchestrator` | `test_worker_panic_marks_chunks_failed_via_timeout` | `test_worker_panic_reports_failure` 给出更强、唯一的失败结果断言，避免弱测试接受两种结果 |

### 重复的代表性解析路径

`core::cli` 保留无子命令、带值的 `config set`、`convert` 默认/显式输出以及一个 `local`
子命令测试；删除以下重复的派生解析快乐路径：

- `test_cli_config_list`
- `test_cli_config_path_and_check`
- `test_cli_config_get`
- `test_cli_local_status`

### 只验证简单实现或测试替身

| 模块 | 删除测试 | 原因 |
|---|---|---|
| `input::typer` | `test_mock_typer_succeeds` | 只断言固定返回 `Ok(())` 的测试替身会成功 |
| `local::installer` | `test_dependency_check_script_covers_required_runtime_packages` | 在测试中重复同一个字符串常量的包名清单，只验证实现文本 |
| `local::service` | `test_base_url_uses_loopback_port` | 只验证一行 `format!` |
| `local::service` | `test_pid_file_path_uses_expected_name` | 只验证一行 `Path::join` |
| `platform::macos` | `config_directory_uses_macos_bundle_identifier` | 只验证一行平台目录拼接 |
| `platform::windows` | `config_directory_uses_windows_application_name` | 只验证一行平台目录拼接 |

### 不受控或证明力不足

| 模块 | 删除测试 | 原因 |
|---|---|---|
| `audio::recorder` | `test_recorder_with_config` | 构造器枚举真实音频设备，测试主体却只重复默认值和初始布尔值；Windows 已因 teardown 崩溃而排除 |
| `local::installer` | `test_detect_python_runtime_reports_supported_version` | 调用主机 Python/uv，且结果已经由纯解析与版本边界测试覆盖 |
| `local::service` | `test_health_check_times_out_on_unhealthy_server` | 使用固定端口和真实 150 ms 计时，且仅断言 `Err`，不能证明轮询或超时机制 |

## 合并清单

以下测试保护的契约相同，只是输入变体不同。合并后保留全部断言，并在失败消息中标明输入
case，避免降低诊断能力：

1. `core::cli::test_cli_convert_basic` 与 `test_cli_convert_with_output` 合并为一个表驱动
   测试，覆盖 `output` 缺省与显式传入。
2. `postprocess::llm::test_conservative_session_no_chunks_finish_empty` 与
   `test_preheat_session_no_chunks_finish_empty` 合并为一个表驱动测试，遍历
   `preheat_enabled = false/true`，保护两种 session 模式共享的空输入契约。

Python 的 string/dict response shape 测试继续分开：两条路径对应不同上游响应类型，当前
体量很小，独立名称能提供更直接的失败定位，合并收益不足。

## 保留测试的确定性调整

删除多余测试之外，实施阶段进行以下最小调整：

1. `audio::recorder` 只在真实 cpal stream 存在时执行停止前的 200 ms 回调等待，使内存
   fixture 测试不再真实睡眠；真实录音停止语义不变。
2. `core::orchestrator` 的 partial-failure 测试删除不影响顺序的 10 ms sleeps；timeout
   测试改用已有 Condvar gate 阻塞 worker，并在断言后释放，不再让测试替身睡眠 500 ms。
3. `local::service` 的健康状态测试先绑定系统分配的动态端口，再启动 200 以及 503→200
   响应序列，消除固定端口、bind/请求竞态和真实超时；stale PID 测试使用 `tempfile`
   自动隔离与清理。
4. `transcriber::api` 为私有重试循环注入 sleeper；生产调用仍使用
   `std::thread::sleep`，测试传入 no-op。客户端错误测试以请求次数证明不重试，删除真实
   elapsed-time 阈值断言。

这些调整只隔离外部时间、设备和文件/端口环境，不改变运行时策略值或错误契约。

## 实施顺序

1. 记录 `cargo test -- --list` 与 `pytest --collect-only` 基线，确保删除清单与收集结果一致。
2. 先调整仍需保留的测试 seam 和 fixture：录音停止等待、orchestrator gate、动态健康
   stub、临时目录、STT no-op sleeper。
3. 运行对应模块的聚焦测试，确认保留行为未变。
4. 删除上表 20 个测试及其不再使用的测试 import/helper，再完成两组表驱动合并，避免
   留下 dead code 或陈旧注释。
5. 运行完整 Rust/Python 校验并检查最终测试清单；更新本文状态和实际数量。

## 测试策略

聚焦验证：

```bash
cargo test audio::recorder::tests
cargo test core::orchestrator::tests
cargo test local::service::tests
cargo test transcriber::api::tests
```

完整验证：

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy -- -D warnings
UV_CACHE_DIR=/private/tmp/viberwhisper-uv-cache uv run ruff check server
UV_CACHE_DIR=/private/tmp/viberwhisper-uv-cache uv run pytest
```

验收标准：

- macOS 本地默认 Rust 测试为 94 个并全部通过；Windows 预计同为 94 个并由 PR CI 核对。
- Python 仍收集 20 个测试，默认环境 18 个通过、2 个按既有条件跳过。
- 测试代码不再枚举真实音频设备或探测主机 Python。
- 保留的健康检查测试不使用固定端口；保留的 STT retry 测试不执行真实退避等待。
- 所有删除均能映射到本文的重复、简单实现、不受控环境或弱证明理由。

## 文档影响

规划阶段新增本文并在 `docs/README.md` 登记。实施只整理内部测试及其 seam，不改变用户
行为、配置、CLI 合约、模块职责或已记录架构，因此 README、架构文档、配置示例和
changelog 不需要同步。实施完成后本文将记录最终状态、测试数量和任何偏差。

## 实施结果

- 从 Rust 源码删除 20 个计划内测试定义，并完成 CLI convert 与 LLM empty-session 两组
  表驱动合并；macOS 默认收集 94 个 Rust 测试。
- recorder 只在真实 stream 存在时等待在途 callback，7 个内存 recorder 测试运行时间为
  0.00 秒。
- STT retry 使用注入 sleeper；生产包装继续调用 `std::thread::sleep`，4xx/5xx 测试捕获
  并断言空等待序列与 `[1s]`，同时验证请求次数和结构化错误。
- local health stub 在调用线程预绑定动态端口，同一测试以 503→200 序列和两次请求证明
  loading 响应不会提前成功；stale PID fixture 使用 `tempfile`。
- Python runtime import 脚本由生产代码中的单一 `REQUIRED_RUNTIME_PACKAGES` 列表生成，
  不再依赖测试内复制的第二份包名清单。
- Orchestrator timeout 使用 Condvar gate；partial-failure 在删除 sleeps 后发现 worker
  队列容量会让第三个 chunk 产生非预期 queue-full，因此使用 channel 明确等待 worker
  取走每个 chunk，并用 `recv_timeout` 限制失败等待。这是对计划中“删除隐式 sleeps”的
  确定性同步细化，不改变产品逻辑。
- Python 自动测试未修改，仍收集 20 个测试。

独立代码审查提出的四条建议（503 负路径、runtime 包清单、worker 同步上限和 retry
等待断言）均已在上述实现中处理，并在修正后重新执行完整验证。

本地验证结果：

- `cargo fmt --check`：通过；
- `cargo check`：通过；
- `cargo test`：94 通过；
- `cargo clippy -- -D warnings`：通过；
- `uv run ruff check server`：通过；
- `uv run pytest -q`：18 通过，2 跳过。

Windows 默认测试数量和通过状态由同一 PR 的 GitHub Actions 验证。

## 风险与回退

主要风险是误删唯一覆盖。通过“删除项必须指向保留证明”、聚焦测试、完整跨平台 CI 和
最终测试清单复核控制风险。实施未发现删除候选保护独立分支或历史回归，也没有为了达到
94 这个数量而扩大删除或合并范围。

## 后续整理记录

### [001] — 2026-11-14 — 第二轮测试简化方案（待批准）

**状态：** 本节是新一轮提案；上文的“已实现”和历史验证结果仅指先前整理。本轮尚未修改产品或测试代码，也未提交、推送或创建 PR。

#### 目标与边界

用户在[本轮线程](https://ampcode.com/threads/T-01a1d87f-64f2-77ed-a301-eda75ad38bab)要求“按 skill 的 test 标准检查简化测试”。按 [`code-principles` 的 Testing Principles](../../.agents/skills/code-principles/SKILL.md)，本轮只删除明确重复或只验证简单赋值的测试，缩小未增加证明力的输入，并复用同一测试中未被修改的 fixture。不设测试数量、覆盖率或耗时目标。

范围仅限下表列出的测试、删除后不再使用的测试 import/helper，以及本文。产品行为、公开接口、配置、依赖、原生 UI 和 CI 工作流均不改变。不通过新增生产 seam 或通用测试框架实现简化。保留取消/并发、错误传播、隐私、Unicode、音频格式、数值边界、持久化及已知回归；不运行依赖个人数据的显式忽略测试。

#### 删除项与保留证明

下列路径和测试名均指本轮审计时的源码；实施前须重新核对工作区，不能据此覆盖他人后续改动。

| 文件 | 删除测试 | 理由及保留证明 |
|---|---|---|
| `src/core/config.rs` | `load_distinguishes_missing_loaded_and_invalid_documents` | 缺失与保存后读取已由 `store_uses_one_injected_path_for_missing_save_and_load` 覆盖；损坏文件的具体 `InvalidDocument` 错误由 `existing_invalid_document_fails_closed` 覆盖。已核对 `ConfigStore::read`：父目录不存在与文件不存在均按 `NotFound` 返回 `None`。 |
| `src/core/recording_session.rs` | `explicit_transition_table_accepts_applied_zero_effect_path` | `session_runs_one_start_and_stop_chain` 已通过调用方接口 `handle(SessionStopped)` 同时断言 effects 为空、状态成为 `Idle`。状态断言能区分成功转换与拒绝事件；拒绝分支不改状态。保留另一个早停拒绝测试。 |
| `src/core/orchestrator.rs` | `production_config_uses_fixed_convergence_timeout` | 仅检查构造器写入的固定 `30s`，属于简单常量断言。有意不再单独锁定这个字面值；真实超时、取消停止请求、超时后晚到结果等行为测试继续保留。 |
| `src/postprocess/llm.rs` | `configures_prompt_and_session_mode` | 仅查看构造器复制的私有字段。实际 endpoint、prompt 和输出由 `configured_processor_cleans_complete_text` 的本地 HTTP 请求保护；将 model 的断言迁入该请求体检查，避免只检查内部副本。配置来源仍由配置测试保护；不再单独断言私有 `preheat_enabled` 字段值，继续保留两种模式的实际 delivery、长文本与确认学习测试。 |
| `src/postprocess/llm.rs` | `session_processes_text_in_both_modes` | `aggregate_cleanup_preserves_long_text_for_cli_and_desktop` 已覆盖两种模式、memory 开/关和准确的转换结果，保留其整个矩阵。已核对 `new_desktop` 直接调用 `new`，memory 关闭时不增加初始化行为。 |
| `src/ui/setup.rs` | `hotkey_setup_keeps_the_displayed_defaults` | `confirmed_wizard_saves_the_selected_configuration` 已经断言 F8/F9 并验证落盘配置；小测试没有额外验证提示文案。 |
| `src/ui/setup.rs` | `hotkey_setup_captures_custom_keys_sequentially` | `hotkey_setup_only_applies_the_confirmed_pair` 已覆盖按顺序捕获两组不同按键、拒绝第一组并准确保留第二组；取消和失败测试不变。 |
| `src/audio/recorder.rs` | `stop_before_overflow_notification_preserves_failure` | 该用例手工置 overflow 标志后 stop。`pause_recording_keeps_short_wakeups_and_bounded_pcm` 实际触发溢出、未 drain 就 stop，并断言相同的空 chunks 和 capacity 警告；两种模式共用分支前的 stop 溢出检查。将“原生 stop 可先于 queued readiness 到达”的回归说明保留到更强用例。 |
| `src/dictation/config.rs` | `prompt_components_preserve_literals_and_use_fixed_order` | `saved_components_supply_instructions_and_conflicting_sources_are_rejected` 使用相同 fixture 和相同精确期望，经 `config.prompt()` 覆盖同一个 `compose()`，并额外验证来源冲突。 |

不删除 `core::cli::test_cli_setup`：虽然体量很小，但目前没有另一用例证明 CLI 字符串 `setup` 的入口映射。也不因体量较大就删减音频重采样、VAD 交错、纠错候选边缘、WebSocket ACK/删除和取消时点矩阵；此次没有充分证据认定其重复。

#### 合并与局部简化

| 文件与测试 | 拟议调整 | 保留的区分能力 |
|---|---|---|
| `src/postprocess/llm.rs`：`test_process_text_empty_input_bypasses_llm`、`empty_session_finishes_empty_in_both_modes` | 合为一个通过 `PostProcessor` 调用的空输入用例，分别检查 `process_text` 和 `process_for_delivery`。使用现有本地 HTTP stub，检查两条入口均返回空、delivery 没有 confirmation、stub 没有请求。开启 memory 以使 confirmation 断言有区分能力，结束时 shutdown。 | 两条独立入口仍分别执行。空输入守卫位于模式分派之前，不再对同一守卫重复两种 preheat 参数；移除守卫时本地 stub 必须使测试失败，不能访问示例公网 endpoint。 |
| `src/dictation/tests.rs`：`complete_first_object_accepts_json_fences_and_ignores_trailing_noise` | 保留所有 suffix × wrapper 输入，由 `parse()` 执行完整矩阵；`parse()` 当前必经 `parse_transcript()`，不再逐格重复后者。另在同一测试保留一个直接通过 `parse_transcript()` 的 fenced+trailing-noise 代表用例。 | 所有 wrapper、尾随内容、引号和字面反引号仍覆盖；aggregate 入口仍有直接证明，两入口不同的 text/JSON 大小限制仍由 `realtime_results_keep_segment_text_and_json_limits` 检查。 |
| `src/dictation/correction/tests.rs`：`surrounding_inserted_text_does_not_extend_a_name` | 仅移除 `Codex.` 和 `Codex！` 两个重复表项，保留三个邻接词场景。 | `punctuation_only_edits_do_not_retrain_automatic_names` 使用相同原词、规则和两个最终文本，额外检查持久化 corrections 完全不变及重复 mention；标点和 Unicode 证明不丢失。 |
| `src/dictation/tests.rs`：`manual_vocabulary_keeps_context_and_budget_before_recent_history` | 动态 topic 从八个缩至三个，将每条重复长历史换为短文本，并将测试名调整为实际保护的“人工语境优先于近期语境”。 | 两个人工语境加三个动态语境已超过四个名额，能区分先取人工词汇与先取近期历史。真正的跨语境全局预算边界继续由 `manual_terms_survive_other_contexts_large_history` 覆盖，不缩小其输入。 |
| `src/dictation/progress.rs`：`slow_consumers_coalesce_updates_and_closed_recordings_cannot_reopen` | 消费前只发布三个不同快照，独立写明最新值期望。 | 仍检查多次发布只通知一次、消费得到最新值、消费后再次通知、取消后拒绝晚到更新。实现没有与更新次数有关的阈值，不需要上千次重复。 |
| `src/prompt_lab/realtime/tests.rs`：`strict_metadata_rejects_malformed_fields_without_rejecting_valid_text` | 仅去除候选数为 8/9 的重复小循环。 | `capped_candidate_prefix_is_valid_without_authorizing_partial_name_groups` 用相同 JSON 调用同一校验器，并额外检查保留候选数量；其 7/8/9 和截断边界全部保留。 |
| `src/prompt_lab/production/tests.rs`：`ordinary_http_matches_desktop_pcm_cleanup_and_learning_except_audio_prompt` | 五个不兼容比较场景共用循环外构造的一个 `Runner`。 | 保留所有变体和实际 HTTP/PCM/cleanup/learning 验证。已核对 `regression` 在 execution 不兼容时、调用 recognizer 之前返回错误；这些失败不推进 Runner 识别或学习状态。 |

#### 实施顺序与验证

1. 获得本节方案的明确批准后，核对基线及工作区；记录两种构建的测试名单。逐模块删除表中重复项，随后完成局部简化，保持产品代码不变。
2. 对受影响的 config、recording_session、orchestrator、postprocess、setup、recorder、dictation 和 prompt_lab 测试做聚焦验证。检查删除项均有上表所列的保留证明，局部输入仍能区分所防止的错误实现。
3. 针对改写的用例做临时单机制对照：空输入误发请求、人工语境排在近期语境后、progress 丢失最新快照时，保留测试应失败；对照后完整恢复产品代码，不提交实验变体。不为纯重复删除新增测试，也不搭建变异测试框架。
4. 运行 `cargo fmt --check`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo clippy --locked --all-targets --features diagnostic-logs -- -D warnings` 和 `git diff --check`。对照测试名单，差异只允许来自本节条目；检查 diff 仅触及测试及本文。
5. 在本节记录实际结果及偏差。Windows 原生执行不能由本机 macOS 的结果替代，后续获授权推送后的 PR CI 才能验证；推送和创建 PR 仍需单独明确授权，代码推送前仍按既有 review gate 执行。

本轮不增加产品机制，因此没有新增机制需要保留/移除消融；上述临时错误实现仅用于确认简化后的测试仍有辨错能力。主要风险是误删唯一证明，控制方式是保留映射、保留高风险矩阵、对改写场景运行反例，而非以通过数量或耗时下降判定成功。可通过还原本轮测试改动回退，不涉及配置或数据迁移。

#### 已有证据与待办

- 已执行 `jj git fetch --remote origin`，审计工作区与最新 `master@origin` 的文件内容一致，初始工作区干净。
- 已在未改测试前运行 `cargo test --locked` 和 `cargo test --locked --features diagnostic-logs`，均通过；私有 VAD 数据集测试和真实 Realtime 服务测试按原有条件忽略。未执行 Windows 原生测试，也未运行忽略项。
- 方案主干通过本机 Claude Code 咨询起草，初始化及响应实际模型均为 `claude-opus-5-5`，tools/MCP 列表为空，最终成功；主线程对照源码确认条件并作取舍，包括保留 CLI setup 入口、将 model 断言移到请求边界，以及保留 aggregate parser 代表用例。咨询和并行静态审查不算运行验证。
- **待批准：** 本节删除、合并及局部简化范围。当前仅追加本文，所有产品和测试源码保持不变；本轮尚未 commit、push、创建 PR 或修改远端共享状态。

### [002] — 2026-11-14 — 按行为契约扩大精简范围（待批准）

**状态：** 用户认为 [001] 简化不足，本节取代其过度保守的取舍；尚未批准或实施。保留 [001] 的删除项和局部简化，增加下列范围。上文历史实现结果不代表本轮已经完成。

#### 调整标准与明确取舍

按 Testing Principles，简单派生、字段复制、固定文案、测试替身自证和重复私有状态观测，可以直接不测，不要求逐项补上等价测试。[001] 关于必须保留 `test_cli_setup` 的决定撤销；它只验证 Clap 派生映射，没有自定义解析逻辑。保留有独立风险的取消时点、候选所有权、协议、隐私、落盘与数值边界，不按规模或配额删测试。

只改测试代码、测试专用 import/helper 和本记录；不改产品行为、公开接口、依赖或工作流。复用现有有限 HTTP/WebSocket fixture，只为反复出现的同一初始状态抽窄 helper，不引入通用 runner、builder、新的生产 seam 或为了测试添加的容量常量。

#### 追加完整删除项

下表是 [001] 之外的增量。实施前重核源码；若已被其他改动替换，按实际行为重新判断。

| 文件 | 删除测试 | 理由与保留契约 |
|---|---|---|
| `src/core/config.rs` | `rejects_retired_v2_documents` | `preparation_rejects_missing_or_invalid_documents_without_writing` 已通过真实准备入口拒绝完整旧文档并验证不写盘。 |
| 同上 | `field_catalog_uses_canonical_keys_and_rejects_legacy_aliases`、`cloned_documents_keep_their_secret_source` | 前者重复单一宏表生成的 catalog/accessor 和固定长度，后者只验证 derive Clone 与 Arc。保留字段读写、未知字段、密钥隔离和配置落盘；不将 clone 断言搬到隐私测试。 |
| `src/core/orchestrator.rs` | `worker_reports_result_without_shared_chunks`、`session_applies_worker_state_transitions` | 直接驱动私有 worker 消息与状态；保留会话调用方的有序合并、失败和取消结果。 |
| 同上 | `test_single_chunk_success`、`test_convergence_timeout` | 重入测试已精确验证单片正常结果；旧 timeout 用例等待后只检查未完成项，不验证实际超时策略。保留 gate 控制的真实 HTTP 取消/收敛超时测试，并将其 pending 数量明确为上传中及排队中两项。已确认单 worker、ZERO 无特殊分支。 |
| `src/core/cli.rs` | `test_cli_setup`、`test_cli_config_set`、`rejects_removed_local_command`、`parses_prompt_lab_evaluation_thresholds_and_paths`、`parses_prompt_lab_agent_review_application` | 普通 Clap 声明和字段映射，没有自定义 parser、范围或语义默认值。保留无子命令、convert 可选输出、深层 sample correct 代表与参数冲突；config set 另有真实 CLI 落盘测试。 |
| `src/ui/setup.rs` | `verification_prompt_explains_the_window_transition`、`hotkey_confirmation_failure_is_not_cancellation` | 分别仅断言固定文案和直白的 `?` 传播，主动不再单测。保留 `SuccessfulVerifier`，用于现有 wizard 保存测试，继续覆盖验证成功后保存。 |
| `src/ui/listener/event_loop.rs` | `shutdown_cancellation_prevents_late_text_injection`、`active_finalization_injects_text_once` | 保留 in-flight shutdown 与 ready handoff 场景；正常确认恰好一次由 `ui/preview.rs` 的 `recording_stages_keep_the_first_position_and_deliver_to_its_target` 经真实 confirm_delivery/gate 精确验证。 |
| `src/platform/runtime.rs` | `native_input_maps_to_platform_neutral_actions` | 四种原生热键事件与 Tray Toggle 并入现有 runtime start/handle_event 测试，通过真实 wiring 验证动作；不再重复私有映射入口。 |
| `src/transcriber/api.rs` | `audible_chunk_still_uses_the_normal_upload_path`、`signal_classification_failure_preserves_the_upload_path`、`test_upload_success_returns_trimmed_text` | 第一项只证明发请求和文本返回，没有不加增益的数值断言；保留实际 quiet-speech 数值测试。后两项并入现有 multipart 测试：公开 transcribe 接受非 WAV fixture、完整发送原字节并 trim 响应。 |
| `src/audio/recorder.rs` | `each_new_chunk_boundary_is_reported_to_the_callback` | 私有 ready count 已由真实 callback 的精确通知次数和 drain/stop 的 PCM 完整性保护。 |
| `src/audio/chunk.rs` | `ordinary_audio_uses_the_fixed_thirty_second_capacity`、`recorder_samples_are_encoded_as_a_complete_wav_payload` | 分别重复 recorder 的真实硬边界和 replay 的 WAV 解码/完整 PCM。容量取小值、半秒临界、扩展 header、非法 spec 仍保留。 |
| `src/prompt_lab/dataset.rs` | `correction_uses_scoring_boundaries_for_proper_noun_occurrences`、`correction_counts_latin_proper_nouns_next_to_han_text` | 将无空格 Han/Latin 正例用于现有 capture/correction 成功用例，`MyCodexTool` 子串反例用于现有失败不改文件用例，删除重复建库、WAV、sidecar 生命周期。细分匹配规则继续由 metrics 测试保护。 |
| `src/dictation/transport/tests.rs` | `truncated_response_is_rejected_even_when_text_done_looks_valid` | engine 的 provisional/final 测试已发送可解析结果后返回 incomplete，验证失败、无 partial 交付且不持久化；更强于旧例的空对象响应。 |
| `src/dictation/config.rs` | `offline_metadata_records_the_saved_component_composition` | 只为检查 metadata 字段赋值启动 transcriber/runtime；有意不再单测该机械转发，保留 prompt 组合与冲突校验。 |
| `src/dictation/feedback.rs` | `confirmation_does_not_wait_for_memory_io` | 实际只验证 Tokio oneshot send/try_recv，没有 memory I/O。保留 engine 的确认顺序、丢弃不学习与退出落盘场景。 |

#### 删除重复机制并收缩 fixture

| 模块 | 调整 | 继续保护的差异 |
|---|---|---|
| `input/hotkey.rs` | 删掉完整 canonical/alias 逐行副本，只留 F8、A、5、RIGHTCTRL、NUMPADENTER、INTLBACKSLASH 及 RIGHTOPTION、RETURN、COMMAND、KP9 别名代表。 | 保留 catalog 唯一性、未知名、大小写/空白与 runtime canonicalization。有意不再快照完整拼写词表或每个 rdev 映射，不另建第二份全量名单。 |
| `core/config.rs` | `field_selection_builds_caller_owned_settings_without_reading_unrelated_secrets` 去掉一次性 RecorderSettings 包装，直接比较所选字段 tuple。 | NoSecrets 的失败哨兵仍证明没有读取不相关密钥。 |
| `core/orchestrator.rs` | 队列满测试由盲发输入、读取私有 inner 和计时阈值，改为 gate 阻塞首上传、明确排入两项、下一项拒绝，独立写出失败序号与成功输出。 | 有时限 channel 证明提交非阻塞，解除 gate 后验证准确结果；沿用现有 sender drop 解锁，不增加 Drop 框架。队列容量变化应使这个场景重新受审，不从实现反推期望。 |
| `platform/runtime.rs` | 去掉只操作 fake 自身的 set_recording、RecordingTyper 和两个静态全局，使用现有 MockTyper。 | 保留 driver 启动、hold/toggle 动作、toggle release 无动作、托盘动作和状态转发；不为机械保存的 Arc 引入身份比较 fixture。 |
| `ui/setup.rs` | recognition/mode/prompt/device 笛卡尔积改为两行：Realtime + 断开设备 + 短而丰富的 Unicode 非默认 prompt；HTTP + 枚举错误 + 无 prompt。ScriptedUi 沿用 Default。 | 两行均比较完整预期配置、磁盘密钥、未启用设置与内置 prompt 刷新；保留四种 invalid connection 用例，未找到可替代的底层边界测试，不将其缩为单一 scheme。 |
| `transcriber/api.rs` | 无限 accept/counter stub 与重复薄包装统一到已有有限 response sequence；按 Content-Length 读满请求立即回复，使用 Connection: close，不等 EOF/read timeout。 | 公开 multipart 用例用带空白响应保护 trim、文件名与完整 bytes；4xx/5xx 使用现有 upload_with_wait，分别检查空等待/一次退避、请求数与结构化错误。静音与 DC 合并，非阻塞 listener 验证没有连接，无服务器线程。 |
| `audio/segment.rs` | 纯计数测试使用不对齐步长的 limit=1001，四种 rate 各保留 limit−1、limit、limit+1、多块加尾巴。 | 精确 sample 数、跨块及 finish 幂等；真实长音频、声学尾部和 recorder 硬边界测试不缩短。 |
| `audio/recorder.rs` | recorder_for_buffer 使用 ActiveRecording/RecordingBuffer 的内存构造器及 push_mono，删除手抄私有默认字段。 | 不调用会枚举真实设备的 AudioRecorder 构造器；保留释放内存、增益、downmix、clip、停止门控断言。 |
| `audio/wav_file.rs`、`audio/vad.rs` | WAV 短文件与多块尾巴共用一个测试主体；交错 VAD 测试只解码固定 WAV 一次。 | WAV 两种文件边界和完整样本顺序不变；全部 VAD batch/stream 推理与交错位置不减。 |
| `prompt_lab/metrics.rs`、`regression.rs` | 同形 noun annotation 用窄 fixture；重复 completed-baseline 生命周期共用真实 evaluate/review fixture；旧 policy 循环去掉重复低层 validate。 | alias、大小写、期望次数等特殊配置仍显式；正常 delta、不兼容/旧版本在 STT 前拒绝、文件读取校验均保留。 |
| `dictation/transport/tests.rs`、`test_support.rs` | 长录音测试删除第二套服务器事件循环，复用 accept_session/commit_segment/complete_response/delete_segment；队列测试复用握手。 | 保留同一 socket、两次真实长录音、完整响应与重复文本，不缩短长录音输入。固定 speech WAV 装载在 engine/transport 复用。 |
| `dictation/correction/tests.rs` | 对反复出现的已确认 from→to 状态抽窄 fixture；删 overlap 测试的规则数组形状断言，以整名替换与普通词不被污染验证。撤销矩阵只去掉旁支“未修改确认”，保留 undo 后规则不变；disabled-memory 测试去掉已由 engine 保护的丢弃预览旁支。 | 不把首次学习、重启或标点训练动作藏入 fixture。候选歧义、跨段和邻接名字的规则不新增/不变断言仍保留：错误的新规则可能影响后续其他输入，仅复查当前预览不充分。 |
| `dictation/evaluation/tests.rs` | 去掉不经过 Replay 的 frozen 旁支、重复 Memory TTL 对照，复用完整响应 helper。 | Replay 支持计数、失败回滚、trace 防篡改、序列化与 cleanup 成败仍验证；词表不在音频后重复的布局回归不删。 |

[001] 的空输入合并、parser 入口去重、标点表项、topic/history、progress、Realtime 元数据与 Runner 复用继续适用。减少体积或构造成本不等于已证明总测试耗时下降，实施后以实际结果报告。

#### 修正实际未执行的编辑矩阵

`src/dictation/correction/tests.rs` 的 `unrelated_contexts_word_substrings_and_editorial_changes_do_not_become_rules` 为三种 editorial 修改复用 `"editorial"` 录音 ID。`Memory::confirm` 发现已有 observation/correction 会直接返回，后续行没有独立施加编辑行为。

实施时让每行使用独立已知状态及录音身份，逐行断言不产生纠正规则。保留三种拒绝学习输入；这是修正测试有效性，不作为删减收益。候选所有权和幂等保护的生产逻辑不改。

#### 验证与风险

1. 先按批准后的范围整理测试，逐模块运行；确认删除后的孤立 helper/import 一并移除，未触碰他人改动。对照 [001] 和本节清单解释差异，不按测试总量验收。
2. 对队列拒绝/非阻塞、HTTP trim/retry、空输入不请求、progress 最新值、语境优先级及 editorial 各行做临时错误实现对照；每项须能区分合理错误实现，并有有限失败路径。实验后恢复生产代码，不保留变异框架或额外运行时开关。
3. 执行 [001] 列出的普通/diagnostic 全量测试、两套 all-targets Clippy、fmt 和 diff 检查。新结果须与原始基线分开；现有通过日志只能证明修改前状态。Windows 和依赖私有数据/真实服务的忽略项仍不得声称本机已验证。
4. 主要风险是把有意义的未来行为约束当成私有实现细节删除；因此保留歧义/undo 的规则不新增断言和不同协议/取消边界。有意放弃常量、derive、固定文案与穷举映射快照，不为这些机械断言另造替代测试。

本节结合第二次本机 Claude 起草、后续源码核对及音频/实验工具、dictation 两组只读审查。咨询中对正常文本恰好一次、ZERO timeout、SuccessfulVerifier 和配置错误等前提均由主线程核对后取舍；咨询建议不等于执行验证。

**待批准：** [001] 加本节修订后的测试精简范围。当前仅有计划文档改动，没有测试或产品实现、commit、push 或 PR。

### [003] — 2026-11-14 — 精简实施与错误实现对照

**状态：** 已按用户“按这版实施”完成 [001] 与 [002] 的本地测试改写和验证。上节的待批准状态仅保留为历史记录。本轮未提交、推送或创建 PR，他人的文档改动未触碰。

#### 实际改动与取舍

- 删除清单中的重复测试及机械断言，合并同契约测试，移除孤立的测试 helper/import。通过修改前后测试注册清单核对删除项、合并项和纯重命名，没有按数量继续扩大删除范围。
- 向导只保留两组有区分力的配对，但四类无效连接全部保留；成功保存仍经过 SuccessfulVerifier。运行时按键映射并入真实 wiring，移除 fake 自证和测试全局变量。
- 队列测试明确挡住首上传、填入排队项、拒绝下一项，检查准确失败序号与部分输出；有限等待失败后先解除 gate 再断言，避免留下阻塞线程。HTTP stub 按 Content-Length 读满再回复，重试测试注入等待而不实际睡眠。
- 音频只缩小纯计数 fixture，不缩短真实长录音；VAD 全部交错推理保留，只去掉固定 WAV 的重复解码。annotation、完成基线、已接受纠正及 speech WAV 复用窄 fixture，首次学习、重启和特殊参数仍显式执行。
- editorial 表格每行使用独立 Memory 与录音身份，并逐行检查没有规则。撤销后的规则不变、跨段/邻接名字、Unicode、候选所有权及不同取消时点保持原有验证。

最终逐文件核对了内嵌测试模块之前的生产代码，并对实验涉及的完整产品文件与 HEAD 作字节比较；所有生产变异和临时行筛选均已移除。没有新增产品接口、依赖、运行时开关或通用测试框架。

#### 临时对照与结果

按 [002] 使用同一测试输入，每次替换一个机制，观察失败后恢复。队列改为阻塞发送、队列多接收一项、HTTP 去掉 trim、错误重试 4xx、不重试 5xx、分别移除两条空输入守卫、progress 只保存首值、业务快照漏合并冻结词表，均被对应测试拒绝。另临时上传大于单次缓冲区的二进制请求体，完整字节断言通过；大 fixture 未保留。

editorial 三行分别运行：将候选归属错误扩大到整段，整句重写与纯标点两行分别失败；放宽字母变化校验后，数字修改行失败。每次只执行对应矩阵行，避免前一行先失败掩盖后续行；最终恢复完整矩阵和产品实现。

一次把人工语境枚举移到历史之后的对照仍通过。源码核对表明当前冻结词表会在业务快照阶段重新合并人工语境，该内部顺序变化未破坏本测试检查的最终结果，因此不把它算作漏检或失败证据。改用漏合并冻结词表的可见回归，确认人工语境与词条保留断言确实失败。未根据这个内部顺序实验添加实现细节断言。

本轮只删减复杂度，没有新增需要保留的产品机制；上述临时错误实现用于确认保留测试的区分能力，不新增永久消融或变异测试设施。完整命令、各反例机制、日志、删除清单对照及本轮数量见[测试简化审计](https://ampcode.com/artifacts/8b701182-c6a3-4dbb-a207-be28c64491eb)。

#### 最终验证与边界

已通过 `cargo test --locked`、`cargo test --locked --features diagnostic-logs`、普通及 diagnostic 的 `cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --check`、`git diff --check`，以及 vendor/rdev 的 `viberwhisper_tests` 回归。原有 vendor/rdev 警告仍存在，不在本次范围内。

Windows 原生验证、私有 VAD 数据集和真实 Realtime 服务忽略项未运行。一次本机耗时不足以证明整体提速，本轮收益只按实际删除的重复结构与缩小的 fixture 报告。验证日志与修改前基线分开保存在上述审计中；代码和本文仍为本地未提交修改。

### [004] — 2026-11-14 — 将误删的行为保护合回现有测试

**状态：** 覆盖审查确认三处独立保护丢失；按用户“补充检查，能合并到其他测试的就合并”完成本地修正，不恢复独立测试或旧服务器循环。此前 [002] 对长录音上传完整性的替代覆盖判断不充分，dataset 失败 fixture 的替换也遗漏了非零次数不匹配，本节修正这两项取舍。

#### 合并方式与边界

- dataset 原有“拒绝且不写盘”测试共用同一存储 fixture，分别检查词内子串匹配零次，以及完整专有名词实际一次、声明两次；两行均要求报错且 sidecar 字节不变。
- 原有长录音测试保留同一连接、两次 33 秒录音和四次响应，逐轮检查请求引用本段前缀及音频，并恢复全部上传 PCM 的总字节断言。短录音及暂停切段测试不触发生产硬边界，不能替代这里的上传完整性保护。
- 扩展已有 `commit_audio` 返回收到的 PCM，供长录音计数、短片段长度和暂停切段样本比对共用；删除后两处重复的接收、解码及 ACK 循环。没有新增 helper、选项、独立测试或产品接口，原有长度及精确 PCM 断言保留。

#### 实际反例与验证

在临时源码副本分别注入五种单机制错误：复用首段前缀 ID、复用首段音频 ID、硬切段尾片漏传、硬切段尾片重复上传，以及把次数相等校验退化为“存在即可”。每种错误均在补充前测试通过、合并后的测试于目标断言失败；主工作区未注入产品错误，临时源码副本已删除，日志保留于[测试简化审计](https://ampcode.com/artifacts/8b701182-c6a3-4dbb-a207-be28c64491eb)。

已通过普通与 diagnostic 全量测试、两套 all-targets Clippy、`cargo fmt --check` 和 `git diff HEAD --check`。反例副本共用 target 曾污染普通配置的构建产物；源码哈希核对无变化，清理当前 crate 缓存后重新执行两种配置均通过，首次失败日志单独保留，不计为最终通过结果。

本节只修正已确认的覆盖损失，不扩大测试删减范围。Windows 原生、私有 VAD 数据集和真实 Realtime 服务仍未验证；修改仍在本地，未提交、推送或创建 PR。
