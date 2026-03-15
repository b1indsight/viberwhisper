# 长音频分片转写计划

## 背景

当前 `ApiTranscriber::transcribe` 会把整个 WAV 文件一次性上传到 OpenAI 兼容转写接口。像 Groq 这类服务通常有单请求音频大小上限（约 `25 MB`），因此 Toggle 模式下较长录音、以及 `handle_convert` 处理的大文件，都会在超过上限后直接失败。

这次计划的目标不是做“实时字幕”那种真正的流式 ASR，而是把**超长录音拆成可控的小片段**，在录音阶段或转写阶段完成上传，并在最后把文本结果收敛回来。

## 本次目标

1. Toggle 模式下，录音达到阈值后**立即封片并在后台上传转写**。
2. 停止录音时只处理尾片，并等待所有后台任务完成后再输出最终文本。
3. 对已有 WAV 文件和批量转写场景，保留**离线分片**兜底路径。
4. 单片上传遇到网络错误或 `5xx` 时支持**指数退避重试**。
5. 调用方接口保持不变：`Transcriber::transcribe(&self, wav_path: &str)` 不改签名。

## 非目标

- 不做真正的流式 ASR：不提供逐句、逐 token 的实时字幕输出。
- 不引入多分片并发上传：本期保持顺序可控，先把正确性做稳。
- 不改 `Transcriber` trait 对外接口。
- 不扩展到非 WAV 分片格式。

## 设计原则

- **主路径前移**：长录音的主要优化点应发生在录音过程中，而不是录完后再统一补救。
- **已有路径兼容**：已有 WAV 文件、CLI 转换场景继续通过 `transcribe(wav_path)` 接口进入系统。
- **失败可定位**：日志和错误信息要能说明是第几个分片失败、失败类型是什么。
- **资源可回收**：临时分片文件必须自动清理，避免堆积。

## 总体方案

### 路径一：Toggle 录音主路径

```text
start_recording()
  └─ 持续写入 PCM buffer
      ├─ 未达到阈值 → 继续录音
      └─ 达到阈值
           ├─ flush 为 chunk.wav
           ├─ 放入后台转写队列
           └─ 清空当前 buffer 继续录音

stop_recording()
  ├─ flush 尾片（如果有）
  ├─ 等待后台队列全部完成
  └─ merge_texts(results) → 最终文本
```

这个路径对应主人的最新要求：**每满足一个分片大小，直接后台上传转写，前台只保持录音**。

### 路径二：已有 WAV / 批量转写兜底路径

```text
transcribe(wav_path)
  ├─ 文件未超阈值 → 直接单次上传
  └─ 文件超阈值
       ├─ split_wav(...)
       ├─ 逐片 transcribe_chunk_with_retry(...)
       └─ merge_texts(results)
```

这样做的原因很简单：导入文件场景没有“录音中的 buffer”可以利用，还是需要离线分片工具兜底。

## 分片策略

### 默认阈值

- `max_chunk_duration_secs = 30`
- `max_chunk_size_bytes = 23 * 1024 * 1024`
- `max_retries = 3`

`30s` 是更合理的默认分片时长：失败重试成本低，单片体积也远低于常见服务上限。

### 切片规则

分片大小同时受**时长阈值**和**字节阈值**限制，谁先达到就按谁切：

```text
chunk_max_frames = min(
  max_chunk_duration_secs * sample_rate,
  (max_chunk_size_bytes - 44) / bytes_per_frame
)
```

### 临时文件

分片统一写入 `./tmp/`，命名格式：

```text
chunk_<source>_<index>_<timestamp>.wav
```

每个临时分片用 `TmpChunk` 封装，并在 `Drop` 中自动删除。

## 上传与重试策略

只对这些错误重试：

- 网络超时
- 连接中断
- HTTP `5xx`

这些错误不重试：

- HTTP `4xx`
- 文件格式错误
- 鉴权失败
- 服务明确返回的业务错误

退避策略：

```text
第 1 次重试：1s
第 2 次重试：2s
第 3 次重试：4s
上限：16s
```

## 文本收敛规则

- 中文（`zh`、`zh-CN`、`zh-TW`）分片结果直接拼接，不插空格。
- 其他语言用单个空格拼接。
- 不把空字符串硬判为失败，**以转写服务实际返回为准**。

## 配置项

在 `AppConfig` 中新增：

```json
{
  "max_chunk_duration_secs": 30,
  "max_chunk_size_bytes": 24117248,
  "max_retries": 3
}
```

字段说明：

| 字段 | 类型 | 默认值 | 含义 |
|---|---|---:|---|
| `max_chunk_duration_secs` | `u32` | `30` | 单分片最大时长 |
| `max_chunk_size_bytes` | `u64` | `23 * 1024 * 1024` | 单分片最大字节数 |
| `max_retries` | `u32` | `3` | 单分片失败后的最大重试次数 |

## 模块改动建议

| 文件 | 变更 |
|---|---|
| `src/audio/splitter.rs` | 新增离线 WAV 分片工具与 `TmpChunk` |
| `src/audio/recorder.rs` | 增加录音中封片、落盘、提交后台队列的逻辑 |
| `src/transcriber/api.rs` | 增加单片上传、重试、文本合并逻辑 |
| `src/core/config.rs` | 增加三个配置字段及默认值 |
| `docs/architecture/audio.md` | 后续补录音分片与后台队列说明 |
| `docs/architecture/transcriber.md` | 后续补离线分片与重试逻辑说明 |

## 边界情况

| 情况 | 处理方式 |
|---|---|
| 文件未超过阈值 | 直接走现有单次上传路径 |
| 分片写盘失败 | 立即返回错误，并清理已生成临时文件 |
| 某分片重试耗尽仍失败 | 返回带分片序号的错误，停止后续处理 |
| 所有分片都成功但结果为空 | 原样返回空字符串 |
| 两个分片阈值都设为 `0` | 退化为不分片，直接单次上传 |

## 测试计划

### `src/audio/splitter.rs`

- `test_short_audio_no_split`
- `test_split_by_duration`
- `test_split_by_size`
- `test_each_chunk_is_valid_wav`
- `test_chunks_cover_all_samples`
- `test_tmp_chunk_drop_deletes_file`

### `src/transcriber/api.rs`

- `test_short_file_single_request`
- `test_long_file_multiple_requests`
- `test_retry_on_503`
- `test_no_retry_on_400`
- `test_results_merged_zh`
- `test_results_merged_en`
- `test_empty_merge_passthrough`

### 录音链路

- `test_toggle_recording_flushes_chunk_on_threshold`
- `test_stop_recording_flushes_tail_chunk`
- `test_background_results_merge_in_order`
- `test_chunk_failure_propagates_to_final_result`

## 分阶段落地

### Phase 1 — 离线分片工具

1. 新增 `split_wav`
2. 补齐 `TmpChunk` 清理逻辑
3. 写 splitter 单元测试

### Phase 2 — 配置扩展

1. 增加三个配置字段
2. 更新 `get_field` / `set_field` / `apply_json`
3. 更新示例配置

### Phase 3 — Toggle 录音后台转写

1. 录音过程中达到阈值即封片
2. 将分片提交到后台转写队列
3. 停止录音时 flush 尾片并等待收敛
4. 保证结果按分片顺序合并

### Phase 4 — 离线路径重试与收敛

1. 提取 `upload_file`
2. 实现 `transcribe_chunk_with_retry`
3. 实现 `merge_texts`
4. 接回 `transcribe(wav_path)`

### Phase 5 — 文档与验收

1. 更新架构文档
2. 跑相关测试
3. 标记本文档状态

## TODO

- [ ] 明确录音线程、临时文件落盘和后台转写队列的同步边界
- [ ] 明确停止录音时如何等待后台任务完成并回传错误
- [ ] 增加进度与日志方案，避免长录音时用户无反馈
- [ ] 评估后续是否需要独立 worker 或异步运行时
