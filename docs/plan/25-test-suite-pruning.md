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
