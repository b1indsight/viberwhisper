# ViberWhisper 文档

## 使用文档

| 文档 | 内容 |
|---|---|
| [快速上手](../README.md) | 下载、首次设置、录音、预览确认与常用命令 |
| [使用详情与故障排查](usage.md) | 安装校验、原生气泡、平台权限、剪贴板与本地历史 |
| [配置指南](configuration.md) | 严格 v3 配置、密钥、麦克风、单键热键、HTTP STT 与可选 LLM 整理 |
| [Realtime 与确认后学习](dictation.md) | WebSocket 服务、提示词组件、两条桌面路径共用的语境记忆和人工纠错 |
| [命令行参考](cli.md) | CLI 入口、setup、config、离线 WAV 转写与 Prompt-lab 入口 |
| [配置示例](../config.example.json) | 不含密钥的完整 v3 示例，含可编辑提示词组件；增益示例为 3.0，内置默认值为 1.0 |

## 贡献与维护

| 文档 | 内容 |
|---|---|
| [贡献指南](../CONTRIBUTING.md) | 源码构建、质量检查、原生预览验证、STT Prompt 回归、跨平台与 PR 流程 |
| [发布手册](releasing.md) | macOS / Windows 打包、dry-run、版本与 tag、发布校验及失败恢复 |

## 架构

| 文档 | 当前职责 |
|---|---|
| [audio](architecture/audio.md) | 麦克风录音、分段、增益与 Silero VAD 静音过滤 |
| [core](architecture/core.md) | 进程入口、严格 v3 配置、CLI、录音状态机与会话编排 |
| [ui](architecture/ui.md) | 桌面事件循环、首次设置、原生预览与确认投递 |
| [input](architecture/input.md) | 热键与托盘驱动、文字投递契约 |
| [platform](architecture/platform.md) | macOS / Windows 原生预览、目标恢复、文字输入、剪贴板与权限 |
| [transcriber](architecture/transcriber.md) | HTTP STT、分段请求、固定重试与文字拼接 |
| [dictation](architecture/dictation.md) | Realtime 生命周期、流式正文、提示词组合、语境与确认后学习 |
| [postprocess](architecture/postprocess.md) | HTTP LLM 整理、结构化校验、共用纠错规则及记忆所有权 |
| [prompt-lab](architecture/prompt-lab.md) | 录音数据集、WER / 专有名词指标、语义复核与 JSON 报告 |
| [Realtime 评测记录](architecture/realtime-prompt-evaluation.md) | 2026-10-12 的指定服务与模型实验，包含质量和兼容性限制 |

## 实现记录

以下计划记录设计、验证范围与已知限制。早期计划可能已被后续实现替代，当前使用方式以使用文档和代码为准。
完整历史见 [plan 目录](plan/)，近期主题可从下列入口阅读：

| 记录 | 主题 |
|---|---|
| [音量与 VAD](plan/50-audio-loudness-adjustment.md) | 有界增益与本机语音活动检测 |
| [core / UI 边界](plan/50-core-ui-boundary.md) | 进程与工作流归 core，桌面交互归 UI |
| [Windows 设置输入](plan/51-windows-setup-input-dialogs.md) | 原生 Unicode 文本和密码输入 |
| [Realtime 听写](plan/52-contextual-streaming-dictation.md) | 分段、语境记忆与服务兼容性验证范围 |
| [提示词组件](plan/53-configurable-dictation-prompt-components.md) | 文件配置、默认资源和全文覆盖 |
| [Realtime 场景回归](plan/54-realtime-prompt-lab-context-memory-validation.md) | 显式种子、模拟时钟、记忆断言与报告 |
| [macOS 热键与权限](plan/55-macos-hotkey-crash-and-permission-guidance.md) | 原生热键修复和辅助功能授权提示 |
| [配置补齐](plan/56-realtime-config-default-materialization.md) | 验证后补齐缺失字段，保留显式值 |
| [确认气泡](plan/57-floating-confirmation-preview.md) | 编辑、确认键释放、目标恢复与投递 |
| [确认后学习](plan/58-confirmed-text-learning.md) | 已确认文字、候选定位与人工纠错 |
| [P2 修复](plan/59-p2-review-fixes.md) | 跨模块边界与错误处理修复 |
| [稳定前缀](plan/59-realtime-stable-prefix.md) | 音频前后语境与连接复用 |
| [Prompt-lab 简化](plan/60-prompt-lab-simplification.md) | 共用扫描与评测语言配置 |
| [流式确认气泡](plan/60-streaming-confirmation-preview.md) | 只读正文增量、最终纠正与取消 |
| [HTTP 整理与学习](plan/61-http-postprocess-correction.md) | 与 Realtime 共用改口规则、语境纠错和确认反馈 |
