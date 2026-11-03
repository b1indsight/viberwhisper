# ViberWhisper 贡献指南

感谢你改进 ViberWhisper。本文面向从源码构建、修改和提交项目的贡献者；安装、配置与日常使用请先
阅读 [README](README.md)。

## 开发环境

- macOS 或 Windows
- 支持 Rust 2024 edition 的 stable toolchain
- GitHub CLI（可选，用于创建和检查 PR）

首次检出代码后运行：

```bash
cargo build --locked
cargo run
```

首次构建会下载校验过哈希的 CPU ONNX Runtime 静态库；Silero VAD 模型已随源码提供，
运行时无需下载。请同时保持 `ort` / `ort-sys` 的固定版本，升级前验证 macOS 11 和
Windows 静态 CRT 兼容性，详见 [VAD 资源说明](assets/vad/README.md)。

Windows 普通开发构建使用 Visual Studio 提供的动态 CRT。发行包要求静态 CRT，需先在
PowerShell 7.3+ 中运行 `./scripts/build-windows-vad-runtime.ps1`，再设置
`$env:RUSTFLAGS = "-C target-feature=+crt-static"` 构建；发行工作流会自动执行。
该脚本需要 Visual Studio C++ 工具、CMake、Ninja、Python 和 Git，并从固定提交编译 CPU
推理库。构建输出保存在当天的 `.cache` 目录中，可重复运行复用。

应用需要麦克风权限；macOS 的文字输入还需要辅助功能权限。API 和运行配置方法见
[配置指南](docs/configuration.md)。

## 主要技术栈

- `rdev`：全局热键监听
- `cpal` / `hound`：跨平台录音与 WAV 处理
- `reqwest`：OpenAI-compatible STT/LLM HTTP 客户端
- `tokio` / `tokio-tungstenite`：Realtime WebSocket 听写与连接生命周期
- `ort` / Silero VAD：本机静音过滤
- `tray-icon` / `winit`：系统托盘与原生事件循环
- `serde` / `serde_json`：配置、历史和报告序列化
- `clap`：CLI 参数解析
- `tracing`：结构化日志

模块说明、功能计划和维护者资料统一收录在[项目文档索引](docs/README.md)，代码代理还应遵循
[AGENTS.md](AGENTS.md)。

## 开发流程

所有改动都通过 GitHub pull request 提交到 `master`，但不限制本地使用的版本控制客户端。开始前先
同步最新 `master`，每个 PR 只处理一项聚焦的改动，并在说明中列出行为变化和验证结果。

非平凡功能应先在 `docs/plan/` 编写计划，通过 draft PR 获得明确批准后再实现。实现继续提交到同一
PR，不要用第二个 PR 替换已经获批的计划 PR。开始新任务前，确认当前工作副本没有夹带其他人的
未完成改动。

## 质量检查

提交前至少运行与改动范围相关的检查。常规 Rust 变更应与 macOS CI 保持一致：

```bash
cargo fmt --check
cargo build --locked
cargo test --locked
cargo clippy --locked -- -D warnings
```

Windows 专用代码还应验证 GUI 入口 feature：

```bash
cargo build --locked --features windows-app
cargo test --locked --features windows-app
cargo clippy --locked --all-targets --features windows-app -- -D warnings
```

测试应覆盖可观察行为和错误边界。文档改动至少运行 `git diff --check`，并检查链接、命令和模块名称
是否仍与代码一致。

## 原生预览验证

运行不录音、不调用 API、不保存历史的气泡演示：

```bash
cargo run --locked --features integration --bin viberwhisper-preview
```

启动后有 3 秒选择专用测试输入框，再演示流式正文、最终纠正和可编辑气泡。
Enter 会实际写入目标，Esc 取消后退出。正常听写仍使用默认程序入口。

```bash
# 对已聚焦的测试输入框执行原生编辑、按住 Enter 不确认、释放后只投递一次的检查
cargo run --locked --features integration --bin viberwhisper-preview -- --smoke

# macOS：不捕获目标或输入文字，保存正文追加与最终纠正的 PNG
cargo run --locked --features integration --bin viberwhisper-preview -- \
  --artifacts .cache/$(date +%F)/streaming-preview
```

后一个模式不需要麦克风、API、个人配置或历史；产物放在忽略提交的缓存目录。
原生窗口、输入法和目标恢复机制见 [UI 架构](docs/architecture/ui.md)。

## STT Prompt 回归调试

采集测试样本时使用现有托盘与 Hold/Toggle 热键：

```bash
viberwhisper prompt-lab record --dataset /path/to/my-stt-dataset

# 查看、校正并验证样本
viberwhisper prompt-lab sample list --dataset /path/to/my-stt-dataset --status pending
viberwhisper prompt-lab sample show --dataset /path/to/my-stt-dataset <sample-id>
viberwhisper prompt-lab sample correct --dataset /path/to/my-stt-dataset <sample-id> \
  --reference-file expected.txt --proper-nouns-file proper-nouns.json
viberwhisper prompt-lab dataset validate --dataset /path/to/my-stt-dataset

# 用临时候选 prompt 全量重跑所有 ready 历史录音
viberwhisper prompt-lab evaluate --dataset /path/to/my-stt-dataset \
  --prompt-file candidate.txt \
  --max-wer-percent 8 --min-llm-score 95 --min-proper-noun-percent 98

# 将编码代理给出的逐样本语义复核应用到同一份报告
viberwhisper prompt-lab report apply-review \
  --report /path/to/my-stt-dataset/runs/<run-id>.json \
  --review /path/to/<run-id>.agent-review.json
```

`prompt-lab record` 只保存当前 STT API 返回的原始结果，不执行 LLM 后处理、不写普通
`history.jsonl`，也不向当前输入框注入文字。每次完成的录音在数据集的 `audio/` 下保存一个完整
WAV，并在 `samples/` 下保存同 ID 的 JSON；初始识别不是标准答案，必须通过 `sample correct`
写入人工参考文本后才会成为 `ready`。专有名词文件是由 `canonical`、`accepted`、
`case_sensitive` 和 `expected_occurrences` 组成的 JSON 数组；省略该文件表示该样本没有专有名词。

数据集中的录音、初始识别和人工参考文本均为本机明文。录音会发送给当前配置的 STT 服务；在后续
调试任务中，编码代理也会读取参考文本与候选结果。请只采集愿意通过这些路径处理的内容。首个版本
要求同一数据集同一时间只由一个 `prompt-lab` 进程使用，不提供锁、原子写入或中断自动恢复；
`dataset validate` 会报告损坏 JSON、截断 WAV、摘要不匹配和无侧车录音，需人工清理或重录。

`prompt-lab evaluate` 每次都会按稳定 ID 顺序重新读取并转写全部 `ready` WAV，不复用旧 STT
结果，也不把人工参考文本发给 STT。`--prompt-file` 的原文只覆盖本次进程内的
提示词，不会修改 `config.json`；HTTP 路径覆盖 `transcription.prompt`，Realtime 路径覆盖完整任务提示词并忽略配置组件。
省略它会使用当前配置作为基线，`--no-prompt` 在 HTTP 路径不发送 multipart prompt，在 Realtime 路径清空指令。
`--compare-to` 可指向一份已完成且数据集、后端与评分版本兼容的
旧报告；不兼容会在任何 STT 请求前失败。报告默认直接写入数据集的 `runs/`，只生成 JSON，
不生成 Markdown 或网页报告。

Realtime 评估不读取个人记忆；`--realtime-cases cases.json` 可使用显式种子与模拟时钟检查语境、记忆学习和过期，场景历史保存在评估报告中。格式与示例见[场景回归说明](docs/architecture/prompt-lab.md#realtime-context-and-memory-scenarios)。

三项指标各自独立，不合成加权总分：

- **词错误率（WER）**：NFKC 规范化后，中文使用固定 Jieba 词典且关闭 HMM，其他文本使用
  Unicode 词边界；数据集值是替换、删除、插入总数除以参考词总数的微平均，可超过 100%。
- **语义差异评分**：编码代理只依据每条人工参考文本和本次 STT 结果，按固定 rubric 给出
  `0..=100` 整数、简短原因和结构化差异；程序不配置或调用 Judge API/model。
- **专有名词准确率**：按 canonical/accepted 形式、大小写策略、词边界和期望出现次数计算匹配数
  的微平均。整个 ready 集必须至少标注一次专有名词，否则回归会拒绝启动。

首次评估成功后，规范报告状态为 `awaiting_agent_review`，此时只有 WER 和专有名词指标，不能判定
通过。编码代理读取该 JSON，生成覆盖全部样本的 versioned review JSON，再执行 `apply-review`；
程序验证 run ID、rubric、完整样本覆盖、`0..=100` 分数及差异结构后，直接改写同一份报告，计算
语义均分和三个门禁。只有 `WER <= max`、`LLM >= min`、`专有名词 >= min` 同时成立时
`meets_targets` 才为 `true`。若未通过，编码代理根据逐条差异修改候选 prompt 并再次全量回归，
直到达到目标或判断 prompt 已无法继续改善。单条 STT 失败不会跳过后续样本，但报告会标记为
`incomplete`、命令返回非零，且该报告不能接受复核或作为比较基线。

## 跨平台改动

- 公共逻辑放在目标无关模块，平台差异通过 `src/platform/` 的编译期后端隔离。
- macOS 文字注入涉及 Accessibility、AppKit/CoreGraphics 和 Chromium paste fallback；不要假定
  剪贴板会恢复原值。
- Windows 桌面发布使用 `windows-app` feature 和 GUI subsystem 入口；CLI 与桌面入口都需要保持
  可构建。
- 修改热键、托盘、录音或文字注入时，应同时评估 macOS 和 Windows 行为，并在 PR 中说明未能
  本地验证的平台。

## 文档与发布

README 保留安装与日常使用入口。行为、配置字段或 CLI 变化应同步更新相应的[使用文档](docs/README.md#使用文档)、配置示例与架构文档；源码构建和验证步骤维护在本贡献指南，打包步骤维护在发布手册。新增文档或实现记录时更新[文档索引](docs/README.md)，避免把历史计划中的状态当作当前使用说明。

维护者执行打包、版本校验、tag 发布、产物验证或失败恢复前，必须遵循
[发布手册](docs/releasing.md)。
