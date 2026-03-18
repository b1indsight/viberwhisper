# LLM 转写支持

## 背景

Issue #16 要求为 ViberWhisper 增加 **LLM-powered transcription** 能力。

当前项目已经具备一条稳定的“音频文件 → 文本”链路：

| 能力 | 所在模块 |
|------|----------|
| 通用转写接口 `Transcriber` | `src/transcriber/api.rs` |
| OpenAI-compatible multipart 音频转写实现 `ApiTranscriber` | `src/transcriber/api.rs` |
| 配置加载与 CLI 配置读写 | `src/core/config.rs` / `src/core/cli.rs` |
| 录音 / 长音频分片 / 会话编排 | `src/audio/*` / `src/core/orchestrator.rs` |

但这条链路的**协议假设仍然过窄**：

- 默认假设后端是 `POST /audio/transcriptions` 这类 **multipart 音频转写端点**
- `language`、`prompt`、`temperature` 等字段的语义完全沿用 Whisper-style API
- `model` 只有一个平面字符串，没有表达“这是 STT 模型还是 LLM 模型”
- `factory.rs` 只负责“有 key 就建 `ApiTranscriber`”，没有区分**请求协议类型**

这在 Whisper / Groq 这类语音转写接口上够用，但一旦接入 **LLM 音频理解 / chat-completions / responses** 风格端点，就会出现两个问题：

1. **请求格式不同**：可能不再是 multipart 文件上传，而是 JSON + base64 音频 / 多模态输入。
2. **参数语义不同**：`language` / `prompt` / `temperature` 可能仍有用，但发送位置和字段名不同，甚至需要 system prompt / instruction 模板。

因此，本计划的重点不是“往现有 multipart 请求里硬塞一个 LLM model 名称”，而是把“**转写后端协议**”正式抽象出来，在不破坏现有 Whisper 路径的前提下，增加一条 **LLM transcription backend**。

## 目标

1. 在配置层增加 **转写后端类型** 概念，区分传统音频转写接口与 LLM 转写接口。
2. 在 `src/transcriber/` 下新增独立的 **LLM transcriber** 实现，而不是污染现有 `ApiTranscriber`。
3. 保持主流程接口稳定：`main.rs`、`convert`、`SessionOrchestrator` 继续只依赖 `Box<dyn Transcriber>`。
4. 允许用户通过配置切换：
   - 传统 Whisper/Groq/OpenAI 音频转写
   - LLM-based transcription
5. 为未来扩展预留空间：后续若接更多 provider / model / request format，不需要再次大改主流程。
6. 补充文档和测试，确保配置与工厂选择逻辑可验证。

## 非目标

- **本期不做实时 token streaming**：仍保持“整段音频 / chunk 收敛后返回完整文本”的边界。
- **本期不做 provider SDK 深度集成**：优先继续走 `reqwest::blocking` + HTTP API，保持当前代码风格。
- **本期不做自动模型发现**：模型名仍由用户配置填写。
- **本期不重做 orchestrator / recorder**：录音、分片、收敛逻辑不应因为 LLM 支持而被改写。
- **本期不承诺所有 OpenAI-like LLM 音频接口全兼容**：先支持 1 条明确、可测试的 LLM 路径，再抽象扩展。

## 现状问题

### 1. `AppConfig` 无法表达“后端协议”

当前核心配置只有：

- `transcription_api_url`
- `provider`（仅信息性字段）
- `model`
- `language`
- `prompt`
- `temperature`

但缺少类似 `transcription_backend` / `transcription_mode` 这样的字段，导致系统不知道：

- 该向 `/audio/transcriptions` 发 multipart
- 还是向 `/chat/completions` / `/responses` 发 JSON

### 2. `ApiTranscriber` 承担了特定协议假设

`ApiTranscriber` 当前内置如下假设：

- 请求体一定是 multipart form
- 一定有 `file`
- 一定有 `response_format=verbose_json`
- 结果一定从顶层 `text` 字段读取

这些假设对 LLM 音频理解接口未必成立。

### 3. 工厂缺少可扩展的分派条件

当前 `create_transcriber(config)` 实质上是：

- 能从 config 构造 `ApiTranscriber` → 用它
- 否则 → `MockTranscriber`

这对于“单一协议实现”没问题，但不足以支持多个请求协议并存。

## 架构方案

### 设计原则

- **稳定上层接口**：`Transcriber` trait 尽量保持不变。
- **新增实现，不污染旧实现**：multipart 路径继续归 `ApiTranscriber`，LLM 路径单独实现。
- **配置驱动选择**：由 `AppConfig` 决定构造哪种 transcriber。
- **最小可行抽象**：只抽象“后端协议类型”，不预先设计过度复杂的 provider registry。

### 配置层新增字段

建议在 `src/core/config.rs` 中新增：

```rust
pub transcription_backend: String,
```

默认值：

```rust
"audio_api".to_string()
```

可选值第一期先支持：

| 值 | 含义 |
|---|---|
| `audio_api` | 现有 multipart 音频转写接口（Groq/OpenAI Whisper-compatible） |
| `llm` | LLM 音频理解转写接口 |

说明：

- `provider` 仍保留为信息性标签，可选。
- `model` 继续保留，表示当前 backend 下使用的模型。
- `transcription_api_url` 继续保留，但其语义从“音频转写 URL”放宽为“当前转写 backend 的 HTTP 入口 URL”。

### LLM 配置补充

为了尽量少改现有配置结构，第一期建议复用已有字段，并只补充 1 个可选字段：

```rust
pub system_prompt: Option<String>,
```

字段职责：

| 字段 | 在 `audio_api` 中 | 在 `llm` 中 |
|---|---|---|
| `model` | Whisper / STT 模型名 | LLM 模型名 |
| `language` | 传给音频接口的语言提示 | 注入 instruction / prompt 模板 |
| `prompt` | 原生 prompt 字段 | 作为用户转写提示词 |
| `temperature` | 原样透传 | 原样透传（若目标接口支持） |
| `system_prompt` | 忽略 | 作为 system / instruction 文本 |

这样做的好处是：

- 旧配置几乎不用改
- 新增字段足够表达 LLM 的 system-level 约束
- 后续如果发现不同 LLM provider 还需要额外字段，再增量补充

### 转写器分层

建议形成以下结构：

```text
src/transcriber/
  api.rs        // 现有 multipart audio API transcriber
  llm.rs        // 新增 LLM transcriber
  factory.rs    // 根据 config.transcription_backend 分派
  mod.rs
```

### `Transcriber` trait

第一期建议**保持不变**：

```rust
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>>;
}
```

理由：

- recorder / orchestrator / convert 路径已经围绕这个接口稳定运作
- LLM transcriber 完全可以在内部把 WAV 读成字节、编码、发请求
- 现在没必要为“未来也许会返回 richer metadata”提前改 trait

如果后续确实需要返回时间戳、置信度、structured segments，再另开 issue 升级为 richer result type。

### `LlmTranscriber` 设计

新增 `src/transcriber/llm.rs`：

```rust
pub struct LlmTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    system_prompt: Option<String>,
    temperature: f32,
}
```

职责：

1. 读取 WAV 文件
2. 将音频编码为目标接口可接受的请求格式（第一期默认 base64）
3. 构建 LLM 请求体（JSON）
4. 发送 HTTP 请求
5. 从响应中提取最终文本

### 请求协议（第一期建议）

为了保持“practical”且便于测试，建议第一期只支持一种明确格式：

- 请求方式：`POST` JSON
- 音频内容：base64 编码
- 文本输入：`system_prompt` + `prompt` + `language`
- 输出：从一个稳定字段中提取最终文本

也就是说，先明确支持一种 **项目自己定义清楚的 LLM contract**，而不是妄图兼容所有 provider 的变体。

推荐约定如下（伪结构）：

```json
{
  "model": "gpt-4o-mini-transcribe-or-other",
  "temperature": 0,
  "messages": [
    {
      "role": "system",
      "content": "你是一个音频转写助手，只输出最终转写文本。"
    },
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "请将这段音频转写为简体中文文本。" },
        { "type": "input_audio", "data": "<base64>", "format": "wav" }
      ]
    }
  ]
}
```

注意：这只是**仓库内部的目标 contract**。落地时要选定一个明确 provider 格式，并在文档中写死，不做模糊兼容。

### 工厂分派逻辑

`src/transcriber/factory.rs` 改造为：

```rust
match config.transcription_backend.as_str() {
    "audio_api" => ApiTranscriber::from_config(config),
    "llm" => LlmTranscriber::from_config(config),
    _ => Err(...)
}
```

错误策略建议保持与现有一致：

- backend 已配置但初始化失败 → 打 warning，回退到 `MockTranscriber`
- backend 值未知 → 打 warning，回退到 `MockTranscriber`

这样不会破坏本地开发体验。

### 长音频行为

`SessionOrchestrator` 与 `split_wav` 不需要知道下层是否是 LLM backend。

也就是说：

- 短音频：`transcriber.transcribe(wav_path)`
- 长音频：仍由现有 chunking / convergence 路径把 chunk 路径逐个喂给 `Transcriber`

只要 `LlmTranscriber` 能处理单个 WAV 文件，整个长音频链路天然兼容。

需要注意的唯一一点：

- LLM backend 的单请求 payload 可能更大（base64 会膨胀）
- 因此现有 `max_chunk_size_bytes` 默认值可能需要在文档里提醒用户酌情下调

第一期不强制改默认值，只在 README / plan 中注明风险。

## 模块改动点

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/core/config.rs` | 修改 | 新增 `transcription_backend`、`system_prompt`；同步默认值、JSON 兼容、CLI 读写 |
| `src/transcriber/llm.rs` | 新增 | 实现 `LlmTranscriber` |
| `src/transcriber/factory.rs` | 修改 | 按 backend 分派 transcriber |
| `src/transcriber/mod.rs` | 修改 | 导出 `LlmTranscriber` |
| `docs/architecture/core.md` | 修改 | 补充 backend 配置语义 |
| `docs/architecture/transcriber.md` | 修改 | 补充 `LlmTranscriber` 与工厂分派 |
| `README.md` | 修改 | 增加 LLM 配置示例与注意事项 |
| `config.example.json` | 修改 | 增加 `transcription_backend` / 可选 `system_prompt` 示例 |

## 测试计划

### `src/core/config.rs`

- [ ] 默认配置中 `transcription_backend == "audio_api"`
- [ ] `config get/set transcription_backend` 可用
- [ ] `config get/set system_prompt` 可用
- [ ] 旧配置缺少新字段时，仍按默认值加载

### `src/transcriber/factory.rs`

- [ ] `audio_api` 能构造 `ApiTranscriber`
- [ ] `llm` 能构造 `LlmTranscriber`
- [ ] 未知 backend 回退到 `MockTranscriber`

### `src/transcriber/llm.rs`

- [ ] 无 API key 时初始化失败
- [ ] 能正确构造 JSON 请求体
- [ ] 能从成功响应中提取文本
- [ ] 4xx / 5xx / 非法 JSON 能正确报错

### 集成验证

- [ ] `cargo test` 全绿
- [ ] `cargo run -- config list` 能展示新字段
- [ ] `cargo run -- convert sample.wav` 在 `llm` backend 下能走通 mock/fixture 测试

## 验收标准

1. 用户可通过配置明确选择 `audio_api` 或 `llm` backend。
2. 主流程 (`run_listener` / `convert`) 无需感知具体 backend 类型。
3. 默认行为保持不变：旧用户不改配置时仍走现有音频转写接口。
4. 文档中提供至少 1 份 LLM backend 配置示例。
5. 新测试覆盖配置兼容、工厂分派和 LLM 响应解析。

## 分阶段实施

### Phase 1 — 配置与工厂抽象

1. 在 `AppConfig` 中新增 `transcription_backend` / `system_prompt`
2. 更新 `get_field` / `set_field` / `apply_json` / `config list`
3. 让 `factory.rs` 根据 backend 分派，但暂时可以先保留 `llm => MockTranscriber` 占位

**验收**：配置测试通过；主流程默认行为不变。

### Phase 2 — LLM transcriber 落地

1. 新增 `src/transcriber/llm.rs`
2. 实现请求体构造、HTTP 调用、响应解析
3. 补齐单元测试

**验收**：`cargo test transcriber` 全绿。

### Phase 3 — 文档与配置示例

1. 更新 `README.md`
2. 更新 `docs/architecture/core.md` / `docs/architecture/transcriber.md`
3. 更新 `config.example.json`

**验收**：新用户可根据文档完成配置切换。

## 风险与取舍

### 风险 1：不同 LLM provider 的 JSON 格式并不统一

取舍：第一期只支持 1 条清晰 contract，不做“理论上兼容一切”的假抽象。

### 风险 2：base64 音频导致请求体膨胀

取舍：先沿用现有 chunking 能力；若后续实测成为瓶颈，再评估更小 chunk 或 provider-specific 文件上传。

### 风险 3：LLM 输出可能包含解释性废话

取舍：通过 `system_prompt` 明确约束“只输出转写文本”，并在解析后做 `trim()`；若仍不稳定，再新增更强的输出约束字段。

## 推荐分支 / PR 策略

本 issue 建议拆成两个 PR：

1. **Plan PR（当前）**
   - 新增本文档
   - 更新 `docs/README.md`
   - 说明为什么需要 backend 抽象而不是直接往 `ApiTranscriber` 里堆分支

2. **Implementation PR**
   - 先做 Phase 1 + Phase 2
   - 文档示例和配置模板一并补齐

这样 review 压力更小，也更符合当前仓库“先 plan，再 implementation”的节奏。
