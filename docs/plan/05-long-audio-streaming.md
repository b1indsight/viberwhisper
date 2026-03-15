# 长音频自动分片转写

## 背景

当前 `ApiTranscriber::transcribe` 将整段录音作为单个 multipart 请求上传到转写 API。大多数 OpenAI 兼容端点（包括 Groq）对单次请求的音频文件有硬性上限：

| 提供商 | 文件大小上限 | 时长参考（44100 Hz 单声道 16-bit） |
|--------|-------------|-----------------------------------|
| Groq   | 25 MB       | ≈ 288 秒 |
| OpenAI | 25 MB       | ≈ 288 秒 |

**数学验证**：`44100 Hz × 1 ch × 2 bytes × 288 s = 25,401,600 bytes ≈ 24.2 MB`，实际安全阈值取 `23 MB`，给 multipart 封装和元数据预留余量。

Toggle 模式录音长度无上限，用户进行 5 分钟以上的会议记录或口述时，文件体积可能超出单次上传限制。当前实现既不会在录音过程中按分片滚动上传，也不会在录音结束后自动拆分重试，只能依赖转写服务对超限请求的实际返回。

## 当前限制

| 层 | 问题 |
|----|------|
| `AudioRecorder` | 全程录音数据累积在内存 `Vec<i16>` 中，无大小上限 |
| `ApiTranscriber::transcribe` | 一次性上传整个 WAV，不检查文件大小或时长 |
| `AppConfig` | 无分片相关配置项 |
| `handle_convert` | 批量转写已有文件时同样无保护 |

## 目标

1. 超过单次 API 限制的长录音自动分片。
2. 每当一个分片达到阈值，就立刻在后台上传转写；前台只保持录音，不被网络阻塞。
3. 录音结束时补齐最后一个未满阈值的尾分片，并按分片顺序拼接转写结果返回。
4. 单片上传遇到网络错误或 `5xx` 时，支持指数退避自动重试。
5. 对调用方保持透明：`Transcriber::transcribe(&self, wav_path: &str)` 签名不变。
6. 新增少量可选配置项，默认值合理，旧配置无需修改。

## 非目标

- **真正的流式 ASR / 实时字幕流**：本期不在 UI 上逐字刷新结果，也不追求句级低延迟展示。
  - 本期只是把上传时机前移到“分片封口后立即后台转写”，最终仍在录音结束后统一拿到完整文本。
- **多分片并发上传**：默认仍按分片顺序串行处理，先保证实现简单、结果有序；若后续证明确有吞吐瓶颈，再单独评估并发。
- **非 WAV 格式支持**：分片输入和输出均维持 WAV，保持与现有 `ApiTranscriber` 接口一致。
- **修改 `Transcriber` trait`**：调用接口不变。

## 架构方案

### 总体流程

#### 路径 A：录音中的后台分片上传（主路径）

```text
start_recording()
  └─ create_chunked_recording_session()
       ├─ audio callback 持续写入当前 chunk buffer
       ├─ 达到 max_chunk_duration_secs / max_chunk_size_bytes
       │    ├─ seal_current_chunk_to_wav()
       │    ├─ enqueue_chunk_for_transcription(chunk)
       │    └─ rotate_to_next_chunk_buffer()
       └─ stop_recording()
            ├─ flush_final_partial_chunk_if_needed()
            ├─ wait_for_background_worker()
            └─ merge_texts(results_in_chunk_order, language) → String
```

#### 路径 B：已有 WAV 文件的离线分片上传（兜底路径）

```text
transcribe(wav_path)
  └─ detect_needs_splitting(wav_path)
       ├─ false（文件 ≤ 阈值）──→ 现有单次上传路径
       └─ true（文件 > 阈值）
            ├─ split_wav(wav_path, max_duration_secs, max_size_bytes)
            ├─ for chunk in chunks:
            │    transcribe_chunk_with_retry(&chunk, max_retries)
            └─ merge_texts(results, language) → String
```

关键设计决策：

- **录音线程不碰网络**：录音回调只负责采样、封片、轮转 buffer，不做 HTTP 请求，避免因网络抖动造成丢帧或卡顿。
- **后台尽早吃分片**：每当分片封口后立即进入后台转写，减少停止录音后的尾部等待时间。
- **已有文件走离线兜底**：`handle_convert` 和手动指定 WAV 的场景保留 `split_wav` 的纯本地切片能力。
- **结果以服务返回为准**：不额外把“空字符串”解释成失败；若服务返回空文本，就按服务结果透传。
- **RAII 清理**：临时分片文件封装在 `TmpChunk` 中，实现 `Drop` trait，无论成功或出错都自动删除。
- **短路失败**：任意一个分片在耗尽重试后仍失败，则在汇总阶段返回错误，并包含分片序号。

### WAV 分片策略

以**时长**为主要切割维度，以**字节大小**为安全上限，两者取先到者：

```text
chunk_max_frames = min(
    max_chunk_duration_secs × sample_rate,
    (max_chunk_size_bytes - WAV_HEADER_SIZE) / bytes_per_frame
)
```

**不同采样率下的实际体积**（`max_chunk_size_bytes = 23 MB`，单声道 `16-bit`）：

| 采样率 | bytes/frame | 23 MB 对应时长 | 30 s 对应大小 |
|--------|-------------|---------------|---------------|
| 16000 Hz | 2 | 731 s | 0.96 MB |
| 44100 Hz | 2 | 261 s | 2.52 MB |
| 48000 Hz | 2 | 239 s | 2.75 MB |

结论：默认 `30s` 分片在常见采样率下都远小于 `23 MB`。这个默认值更接近常见 ASR 分片经验值，也让失败重试成本更低；`max_chunk_size_bytes` 继续作为安全护栏，防止非常规采样率或多声道输入把单片撑爆。

分片文件命名：`./tmp/chunk_<原始文件名去扩展名>_<序号零填充>_<unix_timestamp>.wav`

例：`./tmp/chunk_recording_1719000000_00_1719001000.wav`

### 重试策略

仅对可重试错误重试：网络超时 / 连接错误 / HTTP `5xx`。**不重试 HTTP `4xx`**（格式不支持、权限不足、参数错误等客户端错误，重试无意义）。

```text
attempt 0: 立即上传
attempt 1: 等待 1 s 后重试
attempt 2: 等待 2 s 后重试
attempt 3: 等待 4 s 后重试
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

每个分片用独立的 `hound::WavWriter` 写出，继承原始 `WavSpec`，保持采样率、声道数、位深度不变。

## 模块改动点

| 文件 | 变更类型 | 具体说明 |
|------|----------|----------|
| `src/audio/splitter.rs` | 新增 | `TmpChunk`、`split_wav`、分片辅助函数 |
| `src/audio/mod.rs` | 修改 | 导出 `split_wav` 和 `TmpChunk` |
| `src/audio/recorder.rs` | 修改 | 录音过程中按阈值封片，并把分片投递给后台转写 worker |
| `src/transcriber/api.rs` | 修改 | `ApiTranscriber` 新增配置字段，提取 `upload_file`，新增重试与汇总逻辑 |
| `src/core/config.rs` | 修改 | `AppConfig` 新增带默认值的配置项，更新 `get_field` / `set_field` / `apply_json` |
| `src/commands/convert.rs` 或对应批量转写入口 | 修改 | 已有 WAV 走离线分片兜底路径 |

`Transcriber` trait、`MockTranscriber` 的公开接口保持不变。

### `ApiTranscriber` 字段扩展

```rust
pub struct ApiTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    max_chunk_duration_secs: u32, // 默认 30
    max_chunk_size_bytes: u64,    // 默认 23 * 1024 * 1024
    max_retries: u32,             // 默认 3
}
```

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

    let texts: Vec<String> = chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| self.transcribe_chunk_with_retry(i, chunk))
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
| `max_chunk_duration_secs` | `u32` | `30` | 单分片最大时长（秒）；默认值取常见 ASR 分片经验值 |
| `max_chunk_size_bytes` | `u64` | `23 * 1024 * 1024` | 单分片最大字节数；作为时长阈值之外的安全护栏 |
| `max_retries` | `u32` | `3` | 单分片上传失败后的最大重试次数（不含首次） |

Rust 端定义：

```rust
#[serde(default = "default_chunk_duration")]
pub max_chunk_duration_secs: u32, // fn default_chunk_duration() -> u32 { 30 }

#[serde(default = "default_chunk_size")]
pub max_chunk_size_bytes: u64,    // fn default_chunk_size() -> u64 { 23 * 1024 * 1024 }

#[serde(default = "default_retries")]
pub max_retries: u32,             // fn default_retries() -> u32 { 3 }
```

## 边界与错误处理

| 情况 | 处理方式 |
|------|----------|
| 文件未超过任何阈值 | `split_wav` 返回空 `Vec`，走原有单次上传路径 |
| 录音中途达到阈值 | 当前分片封口后立即进入后台上传，录音继续写入下一个分片 |
| 录音停止时存在尾分片 | flush 成最后一个 WAV，并参与同样的后台转写流程 |
| 分片写入失败（磁盘满等） | 立即返回错误，`TmpChunk` 自动清理已写出的文件 |
| 某分片 `5xx` / 网络错误，重试耗尽 | 返回错误并包含分片序号 |
| 某分片 `4xx` | 不重试，立即返回错误并附带服务响应 |
| 分片返回空字符串 | 透传空字符串，以转写服务返回为准 |
| 分片数 > 100 | 提前返回错误，避免无限制磁盘占用 |
| `max_chunk_duration_secs=0` 且 `max_chunk_size_bytes=0` | 退化为单次上传 |

## 测试计划

### `src/audio/splitter.rs`

| 测试名 | 验证内容 |
|--------|----------|
| `test_short_audio_no_split` | 未超阈值文件返回空 `Vec` |
| `test_split_by_duration` | 超过时长阈值时产生正确分片数 |
| `test_split_by_size` | 超过字节阈值时产生正确分片数 |
| `test_each_chunk_is_valid_wav` | 每个分片都可被 `hound::WavReader` 正常解析 |
| `test_chunks_cover_all_samples` | 所有分片采样总和等于原始文件 |
| `test_tmp_chunk_drop_deletes_file` | `TmpChunk` drop 后文件被删除 |

### 录音中的后台分片上传

| 测试名 | 验证内容 |
|--------|----------|
| `test_rotate_chunk_when_threshold_reached` | 达到阈值后当前 chunk 正确封口并轮转 |
| `test_background_worker_preserves_order` | 后台转写完成顺序乱序时，最终按 chunk index 汇总 |
| `test_stop_recording_flushes_tail_chunk` | 停止录音时尾分片被正确写出并参与转写 |
| `test_recording_thread_not_blocked_by_upload` | 上传阻塞不会卡住录音回调 |

### `src/transcriber/api.rs`

| 测试名 | 验证内容 |
|--------|----------|
| `test_short_file_single_request` | 短文件只发出 1 次 HTTP 请求 |
| `test_long_file_multiple_requests` | 长文件按分片数发出 N 次请求 |
| `test_retry_on_503` | `5xx` 触发重试，次数符合 `max_retries` |
| `test_no_retry_on_400` | `4xx` 不重试 |
| `test_results_merged_zh` | 中文分片结果直接拼接 |
| `test_results_merged_en` | 英文分片结果以空格拼接 |
| `test_empty_merge_passthrough` | 空字符串结果原样透传 |

### `src/core/config.rs`

| 测试名 | 验证内容 |
|--------|----------|
| `test_default_chunk_config` | 三个字段默认值正确 |
| `test_apply_json_chunk_config` | 从 JSON 反序列化正确加载 |
| `test_backward_compat_missing_fields` | 旧配置缺失字段时仍使用默认值 |
| `test_get_set_chunk_fields` | `get_field` / `set_field` 可正常读写新字段 |

## 分阶段落地

### Phase 1 — 纯本地分片能力

1. 新增 `src/audio/splitter.rs`，实现 `TmpChunk` 和 `split_wav`
2. 写 splitter 单元测试
3. 更新 `src/audio/mod.rs` 导出

**验收**：`cargo test audio::splitter` 全绿。

### Phase 2 — 配置扩展

1. `AppConfig` 新增三个字段及 serde 默认值函数
2. 更新 `get_field`、`set_field`、`apply_json`
3. 更新示例配置与说明
4. 写配置单元测试

**验收**：`cargo test core::config` 全绿。

### Phase 3 — 录音中的分片轮转与后台上传

1. 在录音路径中加入 chunk buffer、封片与轮转逻辑
2. 增加后台 worker，消费已封口分片并调用转写
3. 录音停止时 flush 尾分片并等待 worker 收尾
4. 按 chunk index 汇总结果并调用 `merge_texts`

**验收**：录音超过 `30s` 时可在后台持续上传，停止录音后仅等待尾部分片与未完成任务。

### Phase 4 — 已有文件的离线兜底路径

1. `ApiTranscriber` 提取 `upload_file`
2. `transcribe` 入口接入 `split_wav`
3. 写 mock HTTP 单元测试

**验收**：`cargo test transcriber::api` 全绿；短音频行为与改动前一致。

### Phase 5 — 文档与收尾

1. 更新相关架构文档与 README 配置表格
2. 校对默认值、边界条件、错误语义
3. 本文档状态标记为 `IMPLEMENTED`

**验收**：`cargo test` 全绿；PR review 通过。

## 后续扩展方向

- **真正的流式 ASR**：在后台分片上传基础上，引入增量结果回调，把文本实时推送到 UI。
- **多分片并发上传**：在保证顺序合并的前提下评估有限并发。
- **非 WAV 分片**：后续可评估 opus / mp3 等更省流量的中间格式。
