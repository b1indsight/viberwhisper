# 长音频自动分片转写

## 背景

当前 `ApiTranscriber::transcribe` 将整段录音作为单个 multipart 请求上传到转写 API。大多数 OpenAI 兼容端点（包括 Groq）对单次请求的音频文件有硬性上限：

| 提供商 | 文件大小上限 | 时长参考（44100 Hz 单声道 16-bit） |
|--------|-------------|-----------------------------------|
| Groq   | 25 MB       | ≈ 288 秒 |
| OpenAI | 25 MB       | ≈ 288 秒 |

**数学验证**：`44100 Hz × 1 ch × 2 bytes × 288 s = 25,401,600 bytes ≈ 24.2 MB`，实际安全阈值取 23 MB（留 2 MB 余量应对 HTTP multipart 封装头）。

Toggle 模式录音长度无上限，用户进行 5 分钟以上的会议记录或口述时，文件体积将超出该上限，当前代码直接将 API 返回的 4xx 错误透传给用户，没有任何降级处理。

## 当前限制

| 层 | 问题 |
|----|------|
| `AudioRecorder` | 全程录音数据累积在内存 `Vec<i16>` 中，无大小上限 |
| `ApiTranscriber::transcribe` | 一次性将整个文件上传，不检查文件大小或时长 |
| `AppConfig` | 无分片相关配置项 |
| `handle_convert` | 批量转写时同样无保护 |

## 目标

1. 超过单次 API 限制的长录音**自动分片**，依次上传后将结果拼接返回。
2. 单片上传遇到网络错误或 5xx 时，支持**指数退避自动重试**，次数可配置。
3. 分片和重试逻辑对调用方完全透明：`Transcriber::transcribe(&self, wav_path: &str)` 签名不变。
4. 新增三个可选配置项，有合理默认值，旧配置无需修改。

## 非目标

- **真正的流式 ASR**（边录音边转写、实时字幕流）：本期不涉及。
  - 流式 ASR 需要在 `AudioRecorder` 录音回调中周期性 flush 原始 PCM 到磁盘，同时并行驱动转写，最终实时拼接输出——这是与本方案完全不同的架构方向，留作后续专项规划。
- **录音过程中达到分片阈值后立刻后台上传**：本期不做。
  - 当前方案仍是“前台持续录音，停止后再统一分片并转写”。这样改动面最小，不需要把录音线程、磁盘落盘、上传调度三者拆开协调。
- **多分片并发上传**：串行上传已能满足需求，并发需引入 `tokio`，当前 codebase 使用 `reqwest::blocking`，不值得现阶段引入异步运行时。
- **非 WAV 格式支持**：分片输入和输出均维持 WAV，保持与现有 `ApiTranscriber` 接口的一致性。
- **修改 `Transcriber` trait**：调用接口不变。

## TODO

- [ ] 评估“录音阶段每满一个分片即后台上传转写，前台仅保持录音”的流式化方案
- [ ] 明确录音线程、临时文件落盘、后台上传队列之间的同步模型
- [ ] 评估该方案是否需要引入异步运行时或单独工作线程

## 架构方案

### 总体流程

```text
transcribe(wav_path)
  └─ detect_needs_splitting(wav_path)
       ├─ false（文件 ≤ 阈值）──→ 现有单次上传路径（零改动）
       └─ true（文件 > 阈值）
            ├─ split_wav(wav_path, max_duration_secs, max_size_bytes)
            │    → Vec<TmpChunk>
            ├─ for chunk in chunks:
            │    transcribe_chunk_with_retry(&chunk, max_retries)
            └─ merge_texts(results, language) → String
```

关键设计决策：

- **检测前置**：`detect_needs_splitting` 只读 WAV 文件头（44 字节），O(1) 代价。
- **分片纯本地**：`split_wav` 不依赖任何外部服务，仅使用 `hound` 读写 WAV。
- **RAII 清理**：分片文件封装在 `TmpChunk` 中，实现 `Drop` trait，无论成功或出错均删除临时文件。
- **短路失败**：任意一个分片在耗尽重试次数后仍失败，立即返回错误，不上传后续分片。

### WAV 分片策略

以**时长**为主要切割维度，**字节大小**为安全上限，两者取先到者：

```text
chunk_max_samples = min(
    max_chunk_duration_secs × sample_rate,
    (max_chunk_size_bytes - WAV_HEADER_SIZE) / bytes_per_sample
)
```

**不同采样率下的实际安全时长**（`max_chunk_size_bytes = 23 MB`，单声道 16-bit）：

| 采样率 | bytes/sample | 23 MB 对应时长 | 30 s 对应大小 |
|--------|-------------|---------------|---------------|
| 16000 Hz | 2 | 731 s | 0.96 MB ✓ |
| 44100 Hz | 2 | 261 s | 2.52 MB ✓ |
| 48000 Hz | 2 | 239 s | 2.75 MB ✓ |

结论：默认 `30` 秒分片在常见采样率下都远小于 23 MB，上限更接近“交互友好、失败重试成本低”的经验值；`max_chunk_size_bytes` 继续作为安全护栏，防止非常规采样率或多声道输入把单片撑爆。

分片文件命名：`./tmp/chunk_<原始文件名去扩展名>_<序号零填充>_<unix_timestamp>.wav`

例：`./tmp/chunk_recording_1719000000_00_1719001000.wav`

### 重试策略

仅对可重试错误重试：网络超时 / 连接错误 / HTTP 5xx。**不重试 HTTP 4xx**（格式不支持、权限不足等客户端错误，重试无意义）。

```text
attempt 0: 立即上传
attempt 1: 等待 1 s 后重试
attempt 2: 等待 2 s 后重试
attempt 3: 等待 4 s 后重试
（最多 max_retries 次，退避上限 16 s）
```

退避计算：`wait_secs = min(2^attempt, 16)`，使用 `std::thread::sleep`。

### 文本拼接

各分片结果拼接时，分隔符根据配置的 `language` 决定：

- 中文（`zh`、`zh-CN`、`zh-TW`）：直接相邻拼接，不插入空格。
- 其他语言：插入单个空格。

拼接阶段仅去除分片之间引入的冗余空白，不额外把空结果视为失败；最终是否为空，以转写服务返回为准。

## 数据类型设计

### `TmpChunk`（`src/audio/splitter.rs`）

```rust
pub struct TmpChunk {
    pub path: PathBuf,
    pub index: usize,
}

impl Drop for TmpChunk {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
```

### `split_wav` 函数签名

```rust
pub fn split_wav(
    src_path: &str,
    max_duration_secs: u32,
    max_size_bytes: u64,
) -> Result<Vec<TmpChunk>, Box<dyn std::error::Error>>
```

实现要点：

```rust
let reader = hound::WavReader::open(src_path)?;
let spec = reader.spec();
let total_samples = reader.len();
let samples_per_frame = spec.channels as u32;
let total_frames = total_samples / samples_per_frame;

let bytes_per_frame = samples_per_frame * (spec.bits_per_sample as u32 / 8);
let chunk_max_frames = {
    let by_duration = max_duration_secs as u64 * spec.sample_rate as u64;
    let by_size = (max_size_bytes - 44) / bytes_per_frame as u64;
    by_duration.min(by_size) as u32
};

if total_frames <= chunk_max_frames {
    return Ok(vec![]);
}
```

每个分片用独立的 `hound::WavWriter` 写出，继承原始 `WavSpec`（保持采样率、声道数、位深度不变）。

## 模块改动点

| 文件 | 变更类型 | 具体说明 |
|------|----------|----------|
| `src/audio/splitter.rs` | **新增** | `TmpChunk` struct + `split_wav` 函数 |
| `src/audio/mod.rs` | **修改** | `pub mod splitter;` + `pub use splitter::{split_wav, TmpChunk};` |
| `src/transcriber/api.rs` | **修改** | `ApiTranscriber` 新增三个字段，`transcribe` 增加分派逻辑，新增两个私有方法 |
| `src/core/config.rs` | **修改** | `AppConfig` 新增三个带默认值的字段，更新 `get_field`/`set_field`/`apply_json` |

`Transcriber` trait、`MockTranscriber`、`factory.rs`、`main.rs`、`tray.rs`、`hotkey.rs` **均不需要修改**。

### `ApiTranscriber` 字段扩展

```rust
pub struct ApiTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    max_chunk_duration_secs: u32,   // 默认 30
    max_chunk_size_bytes: u64,      // 默认 23 * 1024 * 1024
    max_retries: u32,               // 默认 3
}
```

`from_config` 从 `AppConfig` 注入新字段（有默认值，不影响现有调用）。

### `transcribe` 入口改动（伪代码）

```rust
fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn Error>> {
    let chunks = split_wav(wav_path, self.max_chunk_duration_secs, self.max_chunk_size_bytes)?;
    if chunks.is_empty() {
        return self.upload_file(wav_path);
    }
    if chunks.len() > 100 {
        return Err("分片数量超过 100，拒绝处理".into());
    }
    let texts: Vec<String> = chunks.iter().enumerate()
        .map(|(i, chunk)| {
            tracing::info!("转写分片 {}/{}", i + 1, chunks.len());
            self.transcribe_chunk_with_retry(chunk)
        })
        .collect::<Result<_, _>>()?;

    Ok(merge_texts(&texts, self.language.as_deref()))
}
```

## 配置项

在 `AppConfig` 中新增以下字段（均可选，有默认值，旧配置向后兼容）：

```json
{
  "max_chunk_duration_secs": 30,
  "max_chunk_size_bytes": 24117248,
  "max_retries": 3
}
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `max_chunk_duration_secs` | `u32` | `30` | 单分片最大时长（秒）。默认值取常见 ASR 分片经验值；`0` 表示不按时长切割（仍受大小限制） |
| `max_chunk_size_bytes` | `u64` | `23 * 1024 * 1024` | 单分片最大字节数。`0` 表示不按大小切割（仍受时长限制） |
| `max_retries` | `u32` | `3` | 单分片上传失败后的最大重试次数（指数退避，不含首次） |

Rust 端 `AppConfig` 定义：

```rust
#[serde(default = "default_chunk_duration")]
pub max_chunk_duration_secs: u32,  // fn default_chunk_duration() -> u32 { 30 }

#[serde(default = "default_chunk_size")]
pub max_chunk_size_bytes: u64,     // fn default_chunk_size() -> u64 { 23 * 1024 * 1024 }

#[serde(default = "default_retries")]
pub max_retries: u32,              // fn default_retries() -> u32 { 3 }
```

`config get/set` 支持读写这三个字段（与现有 `get_field`/`set_field` 模式一致）。

## 边界与错误处理

| 情况 | 处理方式 |
|------|----------|
| 文件未超过任何阈值 | `split_wav` 返回空 Vec，走原有单次上传路径，零额外开销 |
| 分片写入失败（磁盘满等） | 返回错误，`TmpChunk` Drop 自动清理已写出的文件 |
| 分片数 > 100 | 提前返回错误，避免无限制磁盘占用（100 片 × 23 MB ≈ 2.3 GB） |
| 某分片 5xx，重试耗尽 | 返回错误（包含分片序号），`TmpChunk` Drop 清理所有临时文件 |
| 某分片 4xx | 不重试，立即返回错误（包含 HTTP 状态码和响应体） |
| 所有分片成功但拼接为空 | 透传空字符串，以转写服务返回为准 |
| `max_chunk_duration_secs=0` 且 `max_chunk_size_bytes=0` | `split_wav` 直接返回空 Vec，退化为单次上传 |

## 测试计划

### `src/audio/splitter.rs` 单元测试

用 `hound` 在内存生成合成 WAV，无需真实麦克风。

| 测试名 | 验证内容 |
|--------|----------|
| `test_short_audio_no_split` | 未超阈值的文件返回空 Vec |
| `test_split_by_duration` | 超过时长阈值时产生正确分片数 |
| `test_split_by_size` | 超过字节阈值时产生正确分片数（高采样率场景） |
| `test_each_chunk_is_valid_wav` | 每个分片可被 `hound::WavReader` 解析，spec 与原始一致 |
| `test_chunks_cover_all_samples` | 所有分片的采样数总和 = 原始文件采样数 |
| `test_tmp_chunk_drop_deletes_file` | `TmpChunk` drop 后文件不存在 |
| `test_chunk_count_limit` | 超过 100 片时返回错误 |

### `src/transcriber/api.rs` 单元测试（mock HTTP）

使用 `mockito` 或直接构造 mock server 拦截请求。

| 测试名 | 验证内容 |
|--------|----------|
| `test_short_file_single_request` | 未超阈值时只发出 1 次 HTTP 请求 |
| `test_long_file_multiple_requests` | 超阈值时发出 N 次请求（N = 分片数） |
| `test_retry_on_503` | 5xx 触发重试，验证重试次数 = max_retries |
| `test_no_retry_on_400` | 4xx 不触发重试，立即返回错误 |
| `test_results_merged_zh` | 中文分片结果相邻拼接（无空格） |
| `test_results_merged_en` | 英文分片结果以空格拼接 |
| `test_empty_merge_passthrough` | 所有分片返回空字符串时，按转写服务结果原样返回 |

### `src/core/config.rs` 配置测试

| 测试名 | 验证内容 |
|--------|----------|
| `test_default_chunk_config` | 三个字段默认值正确 |
| `test_apply_json_chunk_config` | 从 JSON 反序列化正确加载 |
| `test_backward_compat_missing_fields` | 旧配置（无三个字段）加载后使用默认值，不报错 |
| `test_get_set_chunk_fields` | `get_field`/`set_field` 对三个新字段正常工作 |

### 集成测试

`test_long_recording_end_to_end`（需要 `TRANSCRIPTION_API_KEY`，CI 中自动跳过）：使用 5 分钟以上的合成语音 WAV，验证分片上传后返回语义连贯的文本。

## 分阶段落地

### Phase 1 — WAV 分片工具（纯本地，无网络）

1. 新增 `src/audio/splitter.rs`，实现 `TmpChunk` 和 `split_wav`
2. 写全部 splitter 单元测试（使用 `hound` 生成合成 WAV）
3. 更新 `src/audio/mod.rs` 导出

**验收**：`cargo test audio::splitter` 全绿。

### Phase 2 — 配置扩展

1. `AppConfig` 新增三个字段及 serde 默认值函数
2. 更新 `get_field`、`set_field`、`apply_json`
3. 更新 `config.example.json` 注释
4. 写配置单元测试

**验收**：`cargo test core::config` 全绿；`config list` 显示三个新字段及当前值。

### Phase 3 — 转写器分片逻辑

1. `ApiTranscriber` 结构体新增三个字段，`from_config` 注入（有默认值）
2. 将现有上传逻辑提取为私有方法 `upload_file`
3. 实现 `transcribe_chunk_with_retry`（含指数退避 + 重试判断）
4. 实现 `merge_texts`（语言感知拼接，空字符串按服务结果透传）
5. 改写 `transcribe` 入口，调用 `split_wav` 并按需分派
6. 写 mock HTTP 单元测试

**验收**：`cargo test transcriber::api` 全绿；对短音频，行为与改动前完全一致。

### Phase 4 — 文档收尾

1. 更新 `docs/architecture/transcriber.md`：说明分片流程、新字段、`upload_file` 私有方法
2. 更新 `docs/architecture/audio.md`：说明 `splitter` 模块和 `TmpChunk`
3. 更新根 `README.md` 配置表格，加入三个新字段
4. 本文档（`05-long-audio-streaming.md`）状态标记为 IMPLEMENTED

**验收**：`cargo test` 全绿；PR review 通过。

## 后续扩展方向

- **真正的流式 ASR**：在 `AudioRecorder` 的 cpal 回调中积累固定帧数后写临时 WAV 并触发异步转写，最终实时注入输出链路。
- **录音中后台上传**：达到分片阈值即封片并后台转写，停止录音后仅做结果收敛，详见上方 TODO。
- **多分片并发上传**：引入 `rayon` 并行迭代 `chunks`，需处理结果有序合并；仅在串行延迟成为实际瓶颈后评估。
- **非 WAV 格式分片**：分片后转码为 opus/mp3 可显著减小上传体积，需引入 FFmpeg 绑定或 `symphonia`。
