# 长音频分片/流式识别支持

## 背景

当前 `ApiTranscriber` 将整段录音作为单个 multipart 请求发送给转写 API。大多数 OpenAI 兼容端点（包括 Groq）对单次请求的音频文件有大小和时长上限（Groq 限制约为 25 MB / 单次请求）。

用户在以下场景中会触碰该上限：

- Toggle 模式下长时间连续录音（超过约 10 分钟）
- 低比特率设备上采样率较高时，文件体积增长更快
- 未来若引入更高质量的录音格式

超出限制时，API 返回 4xx 错误，当前代码直接将错误暴露给用户，没有任何降级或重试机制。

## 当前限制

| 层 | 问题 |
|----|------|
| `AudioRecorder` | 全程录音数据累积在内存 `Vec<i16>` 中，不设上限 |
| `ApiTranscriber::transcribe` | 一次性将整个文件上传，不检查文件大小 |
| `AppConfig` | 无分片相关配置项 |
| `main.rs` / `handle_convert` | 无超时保护，无分片重组逻辑 |

## 目标

1. 支持对超过单次 API 请求上限的长音频**按时长/大小自动分片**，依次转写后拼接结果。
2. 单片请求失败时支持可配置次数的**自动重试**（指数退避）。
3. 分片和重试逻辑对调用方（`main.rs`）完全透明，调用接口不变。
4. 新增相关配置项，允许用户按需调整分片策略。

## 非目标

- **真正的流式识别**（边录音边转写、实时字幕）：本期不涉及，留作后续扩展点。
- **多线程并发上传多个分片**：串行上传已足够，并发带来实现复杂度不值得现阶段引入。
- **跨平台音频格式支持**（mp3、opus 等）：当前维持 WAV，分片同样输出 WAV。
- **修改 `Transcriber` trait 签名**：调用接口保持 `transcribe(&self, wav_path: &str)` 不变。

## 架构方案

### 整体思路

在 `ApiTranscriber::transcribe` 内部引入一个私有的"分片-转写-拼接"流程：

```
transcribe(wav_path)
  └─ detect_needs_splitting(wav_path) → bool
       ├─ false: 现有单次请求路径（不变）
       └─ true:
            split_wav(wav_path) → Vec<TmpChunk>
            for chunk in chunks:
                transcribe_chunk_with_retry(&chunk) → String
            merge_texts(Vec<String>) → String
            cleanup temp chunks
```

分片本身是**纯本地操作**（读 WAV header、按采样数切割、写临时 WAV 文件），不依赖外部服务。

### 分片策略

以**最大时长**（`max_chunk_duration_secs`，默认 600 秒 = 10 分钟）为主要切割维度，原因：

- 采样率因设备不同（如 44100 Hz vs 16000 Hz），按字节切割需要换算；按时长更直观。
- 10 分钟 × 单声道 16-bit 44100 Hz ≈ 53 MB，超出 Groq 25 MB 限制；实际默认值应根据目标 API 限制调整（建议 300 秒，对应约 26 MB）。

辅助安全上限：`max_chunk_size_bytes`（默认 23 MB），两者取更严格的先到先切。

### 重试策略

针对单个分片的网络/5xx 错误（不重试 4xx 客户端错误，例如格式不支持）：

- 最大重试次数：`max_retries`（默认 3）
- 退避：初始等待 1 秒，每次翻倍，最大 16 秒

### 文本拼接

各分片转写结果以空格（或换行符，视语言而定）拼接。若 API 返回 `verbose_json` 中包含 `segments`，可在拼接前去除首尾标点重复，但本期以简单字符串拼接为准，保留扩展点。

## 模块改动点

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/audio/splitter.rs` | **新增** | WAV 分片工具函数：`split_wav(path, max_duration_secs, max_bytes) -> Vec<PathBuf>` |
| `src/audio/mod.rs` | **修改** | 导出 `splitter` 模块 |
| `src/transcriber/api.rs` | **修改** | `ApiTranscriber` 新增私有方法 `transcribe_with_chunks`、`transcribe_chunk_with_retry`；`transcribe` 入口按需分派 |
| `src/transcriber/api.rs` | **修改** | `ApiTranscriber` 结构体新增 `max_chunk_duration_secs`、`max_chunk_size_bytes`、`max_retries` 字段，由 `from_config` 注入 |
| `src/core/config.rs` | **修改** | `AppConfig` 新增三个可选配置字段（见下方配置项建议） |

`Transcriber` trait、`MockTranscriber`、`factory.rs`、`main.rs` **均不需要修改**。

## 配置项建议

在 `AppConfig` 中新增以下字段（均为可选，有合理默认值）：

```json
{
  "max_chunk_duration_secs": 300,
  "max_chunk_size_bytes": 24117248,
  "max_retries": 3
}
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `max_chunk_duration_secs` | `u32` | `300` | 单个分片最大时长（秒）。设为 `0` 禁用时长切割 |
| `max_chunk_size_bytes` | `u64` | `23 * 1024 * 1024` (23 MB) | 单个分片最大字节数。设为 `0` 禁用大小切割 |
| `max_retries` | `u32` | `3` | 单片上传失败后最大重试次数（指数退避） |

- 旧配置不含这些字段时，按默认值处理，**向后兼容**。
- `config get/set` 支持读写这三个字段。

## 失败重试与边界情况

| 情况 | 处理方式 |
|------|----------|
| 整个文件未超过阈值 | 走原有单次上传路径，零额外开销 |
| 分片过程中磁盘写入失败 | 返回错误，不上传任何分片，临时文件尽力清理 |
| 某分片重试耗尽仍失败 | 整体返回错误，附带分片索引信息，临时文件清理 |
| API 返回 4xx（格式/大小不符） | 不重试，立即透传错误 |
| 分片数量异常多（> 100 片）| 提前返回错误，避免无限制磁盘占用 |
| 拼接结果为空字符串 | 视同转写失败，返回错误 |
| 临时分片文件未清理 | 通过 `Drop` 或显式 `cleanup` 确保退出前删除 |

## 测试计划

### 单元测试（`src/audio/splitter.rs`）

- `test_split_short_audio_no_split`：短于阈值的音频不产生额外分片，返回单元素列表
- `test_split_long_audio_produces_chunks`：超过时长阈值时产生正确数量的分片
- `test_split_by_size_threshold`：超过字节阈值时正确切割
- `test_split_chunk_wav_valid`：每个分片都是合法的 WAV 文件（header 可解析）
- `test_split_cleanup`：分片临时文件在使用后可被删除

### 单元测试（`src/transcriber/api.rs`）

- `test_no_split_when_short`：短文件走单次请求路径（mock HTTP）
- `test_retry_on_server_error`：5xx 触发重试，达到最大次数后返回错误
- `test_no_retry_on_client_error`：4xx 不触发重试
- `test_chunk_results_merged`：多片结果按序拼接

### 集成测试

- `test_long_recording_end_to_end`（需要 API key，CI 中跳过）：使用真实 API 转写超过 5 分钟的测试音频，验证分片拼接后结果语义连贯

### 配置测试（`src/core/config.rs`）

- `test_default_chunk_config`：默认值正确
- `test_apply_json_chunk_config`：从 JSON 正确加载三个新字段
- `test_backward_compat_no_chunk_fields`：旧配置加载后使用默认值

## 分阶段落地步骤

### Phase 1 — WAV 分片工具（纯本地，无网络）

1. 新增 `src/audio/splitter.rs`，实现 `split_wav` 函数
2. 写对应单元测试（使用 `hound` 生成测试 WAV 数据）
3. 更新 `src/audio/mod.rs` 导出

**验收**：`cargo test -p viberwhisper audio::splitter` 全绿。

### Phase 2 — 配置扩展

1. `AppConfig` 新增三个字段及默认值
2. 更新 `get_field`、`set_field`、`apply_json`
3. 写配置测试

**验收**：`cargo test -p viberwhisper core::config` 全绿，`config list` 展示新字段。

### Phase 3 — 转写器分片逻辑

1. `ApiTranscriber` 结构体新增字段，`from_config` 注入
2. 实现 `transcribe_chunk_with_retry`（含指数退避）
3. 实现 `transcribe_with_chunks`（调用 `split_wav` + 循环重试 + 拼接）
4. `transcribe` 入口：检测文件大小/时长，按需分派
5. 写 mock HTTP 的单元测试

**验收**：`cargo test -p viberwhisper transcriber::api` 全绿；对短音频行为与改动前完全一致。

### Phase 4 — 文档与收尾

1. 更新 `docs/architecture/transcriber.md`，说明分片流程和配置项
2. 更新 `docs/architecture/audio.md`，说明 `splitter` 模块
3. 更新根目录 `README.md` 配置表格，加入新字段说明
4. 更新 `config.example.json` 注释，说明新字段用途

**验收**：`cargo test` 全绿；文档 review 通过。

## 后续扩展方向

- **边录边转写**（真正流式）：在 `AudioRecorder` 录音过程中周期性 flush 分片到磁盘并触发转写，最终结果实时追加到输出
- **并发上传**：多个分片并行上传，需引入 `tokio` 或 `rayon`，待评估是否值得引入异步运行时
- **支持非 WAV 格式**：若 API 支持 opus/mp3，可在分片后转码以减小体积
