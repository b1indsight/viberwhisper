# 67 - 保留 30 秒上限，按语音通断提前提交识别

## 当前状态

实现已提交至 [PR #173](https://github.com/b1indsight/viberwhisper/pull/173)，bookmark 为 `feat/pause-aware-recognition-segments`。当前产品及测试基准为 [130da7ba](https://github.com/b1indsight/viberwhisper/commit/130da7ba1d6f55a7b4a69b0112e0cfdf6a04e78a)，其 [macOS / Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/43051151725) 均已通过。PR 保持 Draft，真实语料、服务质量和实机验收尚未完成，未合并或发布。

本文按用户要求仅保留最新方案与验收状态，不再保留旧提案和变更时间线；历史版本可从版本控制查阅。

## 需求与范围

用户要求：**“第三点 保留30s限制，30s内根据通断发送识别”。**

- 每个识别音频段最长 30 秒，仍受 23 MiB 字节上限约束；两者取先到者。整次录音可以超过 30 秒。
- 段内出现持续停顿时提前封段；连续讲话到上限仍封段；正常手动停录提交非空尾段，不等待停顿阈值。
- “发送识别”仅指提交音频。停顿不自动停录、不向目标应用输入文字；保留预览、编辑、Enter 确认与 Esc 取消。
- 只改变普通桌面 HTTP / Realtime 听写。两种离线 `convert`、prompt-lab 评测、raw capture 和 setup 录音均保持固定分段，不迁移配置、数据集、历史或记忆。
- VAD 仅估计声学人声活动，不理解句意或语气；较长的句内思考停顿仍可能切段。

## 分段规则

两条桌面路径共享 [Segmenter](../../src/audio/segment.rs) 和 [StreamingVad](../../src/audio/vad.rs)，分别在自己的采样域使用，不统一传输方式。

| 项目 | 当前规则 |
|---|---|
| 人声证据 | Silero 概率 ≥0.3；本段先出现人声证据，才允许停顿切段 |
| 持续停顿 | 连续 25 个低概率分析帧；16kHz、512 样本 / 帧，对应 800ms 分析静音 |
| 短暂停顿 | 不满上述条件继续留在本段；恢复人声清零连续静音计数，不重置段长 |
| 分析推进 | 约 20ms 小步推进，跨输入包保留残余；不由 200ms 传输包决定切点 |
| 上限与尾段 | 容量优先于停顿；stop 只关闭一次非空尾段，恰好到上限不额外提交空段 |
| 检测失败 | 初始化、重采样或推理失败后，本次录音退回固定上限；保留原样本并记警告 |

切点位于检测确认时的样本游标。正常分段不裁剪、不重叠、不回溯；静音保留在所属段内，输入切片按顺序恰好消费一次。跨包、跨段保留 VAD / 重采样状态。样本守恒不能保证识别模型不漏字，也不能覆盖队列拒绝、录音溢出或主动取消造成的未提交音频。

纯静音不反复触发停顿段。HTTP 仍经既有整段能量 / VAD 门控，纯静音不请求服务；Realtime 可以已上传静音，但封段时清空缓冲，不 commit / 创建响应。短有效尾段不由分段层直接丢弃；Realtime 沿用小于 100ms 时补静音到协议最小长度的规则，补零不计入源样本。

0.3 和 800ms 是当前内部初值，没有新增用户配置，也未证明适合所有中文、轻声或噪声环境。

## 实现职责

### HTTP：录音器封段，worker 只识别完整 WAV

- [录音器](../../src/audio/recorder.rs) 在既有 `take_ready_chunk()` / stop 流程分析原生单声道 PCM、决定切点并编码完整 WAV。CPAL 回调只做下混、增益、写缓冲和轻量唤醒，不执行推理、编码或网络操作。
- 每 200ms 唤醒既有取块调用；VAD 在 PCM 锁之外同步执行。分析游标避免重复分析，一个待提交切点保留编码失败后的重试位置；没有第二份累计 PCM、完成段队列或新增音频线程。
- 正常 stop 补算未分析输入，按序提交完整段和尾段，再开始识别收集；cancel 直接丢弃待处理 PCM，不分析或封尾。
- [HttpSession](../../src/core/orchestrator.rs) 接收完整 WAV 时登记段号，由一个串行 worker 请求识别。所有段（包括尾段）均在 finalization 前登记，完成条件只需等待已登记段终态，不需要短帧重组或额外 EOF 协调。

### Realtime：持续 append，到切点才请求响应

- 录音器继续输出 200ms WAV 信封。[Recording](../../src/dictation/engine.rs) 解码并连续重采样为 24kHz，在此域运行 Segmenter，先确定切点，再按连续切片 append；不会等完整段形成后才开始上传。
- 到停顿、上限或 stop 切点后，经整段门控执行 `input_audio_buffer.commit` 与 `response.create`，后者请求模型生成响应；静音段执行 clear。正常 stop 先刷新重采样尾部，再关闭非空段。
- [transport](../../src/dictation/transport.rs) 保持 `turn_detection:null`，由客户端决定提交。前缀、响应关联、结果校验、清理及确认后学习契约不变。
- 仍只有一个响应在途；等待响应期间后续输入在本地有界排队，不并行上传下一段，也不承诺慢服务下上传始终不中断。

## 积压、失败与取消

| 层次 | 保留的约束 |
|---|---|
| 桌面录音器原生 PCM | 两条路径各有 60 秒缓冲上限；HTTP 的当前未封段也包含在内 |
| HTTP | 容量 2 的完整 WAV 队列，加一个在途请求；使用非阻塞 `try_send` |
| Realtime | 既有容量 300 的短帧通道，生产 200ms 帧对应约 60 秒；当前识别段另受 30 秒上限约束 |

- HTTP 队列满或断开时将该段记为失败，后续段仍可接收，最终显式报告失败及已完成文字；不等待空间、不扩容、不自动重新入队，也不把缺段结果报告为完整成功。
- 原生缓冲溢出停止接受输入并唤醒停录；HTTP 保存首个录音故障或 stop warning，以 `InputFailure` 返回已完成文字。原 stop 错误路径可能丢弃未提交缓冲，不承诺恢复全部已采集音频。raw capture 的 stop warning 和原 CPAL runtime error callback 保持既有日志行为。
- HTTP 收敛预算为关闭识别输入后的 30 秒，包含排队段及请求；录音器 stop 的同步补算与编码在预算开始前完成。Realtime 保留既有响应期限，不与音频段长混为一谈。
- 单段 HTTP 请求失败允许后续段继续，最终返回部分失败；Realtime 故障保留已验证部分文字、放弃本次记忆更新并清理 / 退役连接，不重放状态不明的音频。
- HTTP 与预览共享取消令牌，即使 stop 后 worker 已脱离 orchestrator，Esc / abort / 超时仍能阻止下一段请求。已进入同步 HTTP 调用及其既有重试无法撤回；迟到结果不能交付或影响新 SessionId。
- 主动取消不提交未提交尾段、不交付文字、不写历史或记忆；有部分结果的错误预览仍需用户确认，且不具有学习确认能力。

HTTP VAD 和 WAV 编码在 UI 既有取块 / stop 调用中同步执行，不能保证 UI 永不卡顿。离线 debug 探针的单次 drain 最大耗时约 41.4ms；人为先积压 40 秒再完整补处理约 952ms。这些不是实际 UI 心跳、真实麦克风或服务延迟测量。[测量设置与日志](../../.amp/in/artifacts/http-recorder-timing.log)

## 已有验证

- **边界与音频：** 16/44.1/48kHz、不同分包方式、600ms 换气 / 2 秒停顿、30 秒 ±1 样本及 65 秒输入；检查样本拼接完全一致、尾段只提交一次、编码失败恢复、stop 与 cancel。样本级断言由录音器和 Realtime 协议测试负责，HTTP 测试只保留会话契约与必要时序控制。
- **会话与隔离：** 停录前已开始请求、文字顺序与重复保留、纯静音无请求、满队列、部分失败、超时、停录后取消已分离 worker，以及离线 / raw capture 的固定分段负对照。
- **策略对照：** 英文 fixture 的 200/400/600ms 停顿未切段，800ms 参数能在 1.2 秒停顿内形成切点，1216ms 对照更晚；合成噪声对照支持保留 Silero，分包对照支持约 20ms 推进。移除 HTTP 分析游标或待提交切点均使相应回归失败。上述对照不证明真实语料准确率。
- **自动检查：** Rust 1.99.1 已完成格式、构建、默认 / diagnostic-logs / integration 测试及 all-targets/all-features 严格 Clippy；最新测试精简后复查格式、完整默认测试与严格 Clippy，均通过。最新代码提交的独立功能 / 安全和复杂度双审阅均完成，无发现项；macOS / Windows CI 均通过。

源码与证据：[录音器测试](../../src/audio/recorder.rs)、[HTTP 测试](../../src/core/orchestrator.rs)、[Realtime 协议测试](../../src/dictation/engine/tests.rs)、[组合验证](../../.amp/in/artifacts/http-recorder-validation.log)、[最新测试精简验证](../../.amp/in/artifacts/http-tests-simplification-validation.log)、[双审阅结果](../../.amp/in/artifacts/http-tests-review.json)、[双平台 CI](https://github.com/b1indsight/viberwhisper/actions/runs/43051151725)。`.amp/in` 是独立 workspace 的本地证据，不随 Git 提交分发；其余调查、对照与截图见[实现线程](https://ampcode.com/threads/T-01a1c439-d77d-71d2-9889-001df09562eb)。

## 待验收与发布边界

- 中文 / 中英混说、轻声和尾音、真实噪声、句内思考及词跨 30 秒的识别质量；以固定分段为对照评估 WER、专名和边界丢字 / 重复，不能用英文 fixture 或固定分段 prompt-lab 旧成绩替代。
- 真人标注下的目标仍待验证：≤600ms 换气不误切，≥1.2 秒可接受停顿的切段召回 ≥90%，空闲后端停声至提交 p95 ≤1.2 秒；微平均 WER 恶化不超过 1 个百分点、专名不退化。这些是验收目标，非已达成结果。
- 真实服务的提交 / 返回延迟、请求数、费用、慢后端持续吞吐，以及 CPU、UI 心跳、取消响应和进程峰值资源；分别记录声学停顿、切点判定、网络提交、结果完成时间，不混报延迟。
- macOS / Windows 真实麦克风、目标应用与预览交互验收。Windows CI 通过不等于 Windows 实机验收完成；已有无目标原生预览 fixture 只验证了呈现与基本确认 / 取消行为。

验收完成前保持 Draft，不标记 Ready、合并或发布。不加入语义 / 语气端点、服务端自动 turn detection、音频重叠或回溯、并行请求、自动文字投递、新配置或通用音频管线；不改变版本号、依赖或部署。
