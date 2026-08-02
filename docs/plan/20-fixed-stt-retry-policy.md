# 内部运行策略与配置面精简

## 背景

当前 canonical v2 配置同时包含用户意图和模块内部运行策略。后者包括 chunk 切分安全值、请求重试预算、
session 收敛预算，以及一个不参与任何运行时分支的 provider 标注。把这些值暴露给用户会允许相互耦合的
时间/容量预算失配，也扩大了无效配置、CLI 和文档的维护面。

本计划先审计全部 26 个 canonical key，再把没有真实用户选择的字段收回对应模块。STT 策略固定为：
单次请求最多 12 秒，网络错误或 HTTP 5xx 最多重试一次，重试前等待 1 秒；最坏请求窗口约为
`12 + 1 + 12 = 25` 秒。HTTP 4xx 仍立即失败。

## 配置审计结论

### 移除并内置

| 当前字段 | 结论 | 新所有者/固定值 |
| --- | --- | --- |
| `chunking.max_duration_secs` | producer 的分片/延迟策略，不是用户业务意图 | `audio`，30 秒 |
| `chunking.max_size_bytes` | WAV payload 安全护栏，不应允许关闭或突破 | `audio`，23 MiB |
| `chunking.max_retries` | 与 HTTP/session 时间预算耦合 | `transcriber`，1 次 |
| `session.convergence_timeout_secs` | 与 retry 窗口耦合的 session 生命周期策略 | `orchestrator`，30 秒 |
| `inference.api.provider` | 仅作标注，运行时没有读取点 | 删除，不替换 |

删除前三项后 canonical JSON 不再需要 `chunking` section；删除收敛字段后也不再需要 `session`
section。API profile 只由 endpoint、model 和认证决定，不再保存 provider 标签。

### 保留为显式用户选择

| 分组 | 保留字段 | 理由 |
| --- | --- | --- |
| 格式元数据 | `schema_version` | 只读但用于识别持久化格式，不是运行策略 |
| 输入 | `input.hold_hotkey`、`input.toggle_hotkey` | 用户快捷键选择，可分别禁用 |
| 音频 | `audio.mic_gain` | 麦克风硬件和输入音量存在真实差异 |
| 转写 | `transcription.language`、`prompt`、`temperature` | 直接改变识别请求和结果 |
| 后处理 | `post_process.enabled`、`preheat_enabled`、`prompt`、`temperature` | 分别控制开关、延迟/成本取舍和输出策略 |
| profile | `inference.active` | API/Local 是明确的用户选择 |
| API | transcription/post-process 的 URL、model、只读 key 状态 | 支持不同 OpenAI-compatible 服务与模型 |
| Local | `data_dir`、`server_port`、`quantization` | 分别对应存储位置、端口冲突和硬件/精度取舍 |

审计后 canonical key 从 26 个缩减到 21 个。

## 目标

1. audio、transcriber、orchestrator 各自用模块常量拥有内部容量和时间预算。
2. 从 canonical JSON、CLI 字段目录、运行时组装参数和用户文档中删除上述五个字段。
3. 只接受当前 `schema_version = 2` canonical 结构；已删除字段与其他未知字段一样直接拒绝。
4. 保持错误分类、chunk 内容、session partial-result 语义和用户可配置字段的行为不变。

## 非目标

- 不开放请求超时、退避间隔或其他替代运行策略。
- 不改变并发模型、队列容量或超时后的 detached-worker 行为。
- 不移除高级但确有用户取舍的模型、后处理和 Local 设置。
- 不修改历史 plan 中记录的当时设计；只更新当前架构和用户文档。

## 设计

### 模块内策略

各模块直接拥有自己唯一的生产策略：

```rust
// audio
pub(crate) const MAX_CHUNK_DURATION_SECS: u32 = 30;
pub(crate) const MAX_CHUNK_SIZE_BYTES: u64 = 23 * 1024 * 1024;

// transcriber
const STT_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const STT_MAX_RETRIES: u32 = 1;

// orchestrator
const CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(30);
```

生产组装不再从 `ConfigDocument` 搬运这些值：

- `AudioConfig` 只从持久化配置接收 `mic_gain`，内部填入固定 chunk limits；
- 离线 `ConvertConfig` 使用相同 audio 模块常量，避免实时/离线策略漂移；
- `TranscriberConfig` 不再携带 retry 次数，也不再接收 `ChunkingSection`；
- `OrchestratorConfig` 不再接收或校验 `SessionSection`，生产构造固定使用 30 秒；模块内测试仍可直接构造短 timeout，保持测试快速确定。

底层 `max_frames_per_chunk` 和 `WavChunkReader::open` 仍接受明确参数，便于纯逻辑测试和复用；只是生产调用点
不再由用户配置这些参数。

### 严格 v2 配置

新的 canonical JSON 顶层不再含 `chunking`、`session`，API profile 不再含 `provider`。

`ConfigDocument` 直接派生严格反序列化，只接受新的 21-key canonical 结构。顶层出现 `chunking`、
`session`，或 API profile 出现 `provider` 时，均按未知字段失败；不设置迁移 DTO，也不保留旧配置读取路径。
`schema_version` 继续为 2，但只描述当前结构。

## 文件改动

| 文件 | 改动 |
| --- | --- |
| `src/audio/mod.rs` | 拥有生产 chunk limits，`AudioConfig` 不再接收 `ChunkingSection` |
| `src/transcriber/api.rs` | 固定 12 秒请求超时和一次重试；删除 retry 配置传递与校验 |
| `src/core/orchestrator.rs` | 固定 30 秒生产收敛预算；删除 session 配置校验 |
| `src/core/config/document.rs` | 删除 `chunking`、`session`、API provider 字段，只反序列化当前 canonical 结构 |
| `src/core/config/fields.rs` | 从 canonical CLI 字段目录及 get/set 分支删除五个 key |
| `src/runtime_config.rs` | 用模块策略组装 listener/convert，不再传递已移除字段 |
| `config.example.json` | 删除两个内部策略 section 和 provider 标签 |
| `README.md`、`docs/architecture/{audio,core,transcriber}.md` | 记录精简后的配置面和固定策略 |
| `changelog` | 记录用户可见配置面收窄 |

## 实现顺序

1. 先增加严格配置与字段目录测试，锁定已退役字段被拒绝、新 v2 不输出五个字段。
2. 在 audio、transcriber、orchestrator 中建立单一生产常量，并增加最小行为测试。
3. 删除运行时配置传递、领域字段、验证分支和 CLI get/set 分支。
4. 删除 serde 边界中的旧字段读取逻辑。
5. 更新示例、当前架构文档、README 和 changelog。
6. 运行格式、全量测试和 lint，并通过代码审查 gate 后更新同一个 PR。

## 测试策略

- 新配置 round-trip：canonical 示例不包含五个已移除字段，仍能完整序列化/反序列化。
- 严格结构：向新示例分别注入旧 `chunking`、`session` 和 provider 后均加载失败。
- CLI 字段目录：五个 key 不在 catalog 中，`get`/`set` 返回 unknown key，总数为 21。
- chunk 行为：实时录音和离线 reader 都使用 30 秒/23 MiB 的同一生产限制；容量换算纯函数测试保持可注入边界值。
- retry 行为：HTTP 503 恰好收到两次请求；HTTP 4xx 仍只收到一次请求。
- convergence 行为：生产配置得到 30 秒预算；现有短 timeout 单元测试不变慢。
- 回归：`cargo fmt --check`、`cargo test`、`cargo clippy -- -D warnings`。

## 验收条件

- 用户无法通过 JSON 或 CLI 改变 chunk limits、STT retry 次数或 session convergence timeout。
- 每个 chunk 最多发起两次 STT 请求，每次最多 12 秒，重试间隔 1 秒。
- 实时和离线生产路径统一使用 30 秒/23 MiB chunk limits，session 使用 30 秒 convergence timeout。
- API provider 标签不再存在，endpoint/model/auth 行为不变。
- 含已退役字段的旧 v2 配置启动失败，并明确报告未知字段。
- 其余未知字段继续被拒绝，现有转写、离线转换和 session 测试保持通过。

## 实现状态

- [x] 五个内部/无行为字段已从 canonical JSON 和 CLI catalog 移除。
- [x] audio、transcriber、orchestrator 已接管固定生产策略。
- [x] 已退役的旧 v2 字段会被严格拒绝，不存在兼容反序列化入口。
- [x] 示例、README、架构文档和 changelog 已同步。
- [x] 严格配置、固定 chunk limits、一次 retry 和固定 convergence timeout 均有回归测试。
