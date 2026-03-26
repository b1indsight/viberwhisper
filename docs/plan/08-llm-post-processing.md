# LLM 文本后处理

## 背景

Issue #16 需要的不是“让 LLM 直接识别音频”，而是在现有语音识别链路之后增加一层 **LLM 文本后处理**。

结合 review 反馈，这条链路应明确拆成两层：

```text
audio -> OpenAI-compatible streaming STT -> partial/final text buffer -> LLM post-process -> final text
```

这里有两个关键点：

1. **识别层要优先使用兼容 OpenAI 格式的流式 API**，并配合语言识别或语言提示来降低整体延迟。
2. **LLM 只负责文本后处理**，例如补标点、去掉无意义语气词、清理中断与重复，不直接替代音频识别层。

当前 ViberWhisper 已经具备稳定的音频采集、分片、转写与文本注入链路。无论是短录音还是 `SessionOrchestrator` 驱动的长录音，会话最终都会收敛为文本并输出。因此，这个 feature 更准确的定位是：

> 在“流式识别结果”与“最终输入文本”之间，增加一个可选的 **LLM rewrite/post-processing layer**。

## 目标

1. 将识别链路升级为**兼容 OpenAI 格式的流式语音识别 API**，用语言识别/语言提示配合流式结果来降低延迟。
2. 在 STT 之后增加独立的 **LLM 文本后处理层**，负责补标点、去语气词、清理中断与重复。
3. 允许用户通过配置开关启用/禁用后处理。
4. 支持为识别与后处理分别配置模型、API 地址、提示词和温度等参数，并优先直接兼容 OpenAI 格式 API。
5. 后处理失败时，系统应优雅降级为输出原始 STT 文本，而不是整次会话失败。
6. 让 `run_listener` 与 `convert` 两条路径都能复用同一套后处理逻辑。
7. 补充测试与文档，明确流式识别层与后处理层的职责边界。

## 非目标

- **不让 LLM 直接替代 STT 的职责**：语音转文字仍然是识别层的工作，LLM 负责文本整理。
- **不引入音频多模态 LLM 输入**：后处理层不直接接收音频文件。
- **不做 token 级实时润色 UI**：本期重点是流式 STT 降低识别延迟，LLM 后处理仍以稳定的阶段性文本或最终文本为输入。
- **不尝试做复杂 NLP 管线**：例如句法分析、关键词提取、风格模板链等，都不在本期范围内。
- **不强制所有用户使用 LLM 后处理**：默认应保持关闭，避免破坏当前体验与成本预期。

## 当前限制

### 1. 识别链路还不是以流式 API 为中心

当前主要链路大致是：

```text
AudioRecorder / SessionOrchestrator
  -> Transcriber::transcribe(...)
  -> String
  -> TextTyper::type_text(...)
```

问题有两个：

- 识别结果以整段文本收口为主，延迟主要取决于录音结束后的整体转写。
- 还没有把 **兼容 OpenAI 格式的流式识别 API** 作为明确前提写清楚。

### 2. 转写结果直接进入输出层

即使识别成功，中间也没有独立的文本修正阶段。

### 3. 配置层没有“后处理”概念

`AppConfig` 目前只覆盖：

- 转写 API
- 模型
- 语言
- prompt
- temperature
- 录音 / 分片相关参数

但没有表达：

- 是否启用文本后处理
- 后处理用哪个模型
- 后处理 API 地址是什么
- 后处理提示词如何定制
- 识别层和后处理层是否都走 OpenAI-compatible API

### 4. 失败语义过于粗糙

如果未来把 LLM 后处理硬塞进现有转写器内部：

- 会把“识别失败”和“润色失败”混成一类错误
- 上层无法明确选择是否回退到原始文本

这会让主流程的错误处理变脏。

## 架构方案

### 设计原则

- **职责分离**：流式识别层负责尽快产出原始文本；后处理层负责把原始文本清洗成最终文本。
- **OpenAI 兼容优先**：识别与后处理都优先直接兼容 OpenAI 格式 API，减少自定义协议。
- **默认保守**：不开启配置时，行为与现在完全一致。
- **失败可降级**：后处理失败时，不影响原始转写结果输出。
- **接口轻量**：尽量以 `String -> String` 为核心抽象，避免过度设计。

### 识别层前提：OpenAI-compatible streaming STT

本计划不把识别层视为完全静态前提，而是明确要求上游识别优先对接 **兼容 OpenAI 格式的流式语音识别 API**。

核心要求：

- 使用流式识别降低“停止录音后再等整段转写”的感知延迟。
- 保留语言识别或语言提示能力，让识别端尽可能拿到足够上下文。
- 允许上层维护一个 **partial/final text buffer**，把阶段性结果与最终结果区分开。
- 后处理层只消费已经稳定的阶段性文本或最终文本，不直接绑定底层流事件细节。

职责关系：

```text
streaming STT (OpenAI-compatible)
  -> partial/final text buffer
  -> post-processor
  -> typer / CLI output
```

### 新增抽象：`TextPostProcessor`

建议新增模块：

```text
src/postprocess/
  mod.rs
  llm.rs
  factory.rs
```

核心 trait：

```rust
pub trait TextPostProcessor: Send + Sync {
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>>;
}
```

默认实现至少包含两种：

| 实现 | 作用 |
|---|---|
| `NoopPostProcessor` | 直接返回原文，用于默认关闭或降级场景 |
| `LlmPostProcessor` | 调用 LLM API 对文本进行清洗与重写 |

### 主流程接入点

建议在上层已经拿到**稳定文本**、但尚未注入到输入框之前接入。

#### `run_listener` 路径

```text
录音结束 / 阶段性稳定文本到达
  -> streaming STT final text
  -> post_processor.process(&transcribed_text)
  -> final_text
  -> typer.type_text(&final_text)
```

#### `convert` 路径

```text
输入 WAV 文件
  -> transcriber.transcribe(path)
  -> transcribed_text
  -> post_processor.process(&transcribed_text)
  -> 输出终稿文本
```

这样可以保证：

- 同一套后处理逻辑覆盖交互录音和 CLI 转写。
- `SessionOrchestrator` 不需要理解 LLM 细节。
- 音频相关模块不被文本清洗逻辑污染。

### 配置设计

建议把“识别层”和“后处理层”配置显式分开，并默认优先兼容 OpenAI 格式 API。

建议在 `src/core/config.rs` 的 `AppConfig` 中新增以下字段：

```rust
pub transcription_streaming_enabled: bool,
pub transcription_api_format: String,
pub post_process_enabled: bool,
pub post_process_api_url: Option<String>,
pub post_process_api_key: Option<String>,
pub post_process_api_format: String,
pub post_process_model: Option<String>,
pub post_process_prompt: Option<String>,
pub post_process_temperature: f32,
```

#### 默认值建议

| 字段 | 默认值 | 说明 |
|---|---|---|
| `transcription_streaming_enabled` | `true` | 新实现默认走流式识别路径 |
| `transcription_api_format` | `"openai"` | 识别层直接兼容 OpenAI 格式 API |
| `post_process_enabled` | `false` | 默认关闭，保持现有行为 |
| `post_process_api_url` | `None` | 未启用时不需要 |
| `post_process_api_key` | `None` | 建议支持环境变量覆盖 |
| `post_process_api_format` | `"openai"` | 后处理层直接兼容 OpenAI 格式 API |
| `post_process_model` | `None` | 未启用时不需要 |
| `post_process_prompt` | 内置默认 prompt 或 `None` | 用户可覆盖 |
| `post_process_temperature` | `0.0` | 保守输出，降低发散 |

#### 环境变量建议

为了避免把第二套密钥硬编码进 `config.json`，建议额外支持：

- `POST_PROCESS_API_KEY`

优先级可设计为：

1. `POST_PROCESS_API_KEY`
2. `config.json` 中的 `post_process_api_key`
3. `None`

### 默认 Prompt 策略

默认 prompt 应该明确约束 LLM：

- 只做文本整理，不改变原意
- 补足自然标点
- 删除明显的语气词、重复和自我打断
- 不增加新信息
- 只输出最终文本，不要解释

示意：

```text
请将下面的语音转写结果整理为适合直接发送的中文文本：
- 保留原意，不要扩写
- 添加自然标点
- 删除无意义语气词、重复和明显自我打断
- 若句子本身不完整，可做最小必要整理
- 只输出整理后的最终文本，不要解释
```

### `LlmPostProcessor` 设计

建议新增 `src/postprocess/llm.rs`，结构大致如下：

```rust
pub struct LlmPostProcessor {
    api_key: String,
    api_url: String,
    model: String,
    prompt: String,
    temperature: f32,
}
```

职责：

1. 接收原始转写文本
2. 构造 LLM 请求
3. 调用文本生成接口
4. 解析响应中的最终文本
5. 返回 `trim()` 后的结果

#### 请求协议

本期建议**直接兼容 OpenAI 格式 API**，而不是重新定义一套自有协议。

- 识别层：优先对接 OpenAI-compatible streaming transcription / realtime API
- 后处理层：优先对接 OpenAI-compatible text generation API
- 配置层通过 `*_api_url` + `*_api_format = "openai"` 保持明确语义
- 第一阶段只做 OpenAI 格式，其他 provider 如有必要再在此基础上扩展

换句话说，这里不是“先造一个万能 contract”，而是“**先把 OpenAI 格式直接跑通并作为默认兼容面**”。

### 工厂与降级逻辑

新增 `create_post_processor(&AppConfig) -> Box<dyn TextPostProcessor>`：

```rust
if !config.post_process_enabled {
    return Box::new(NoopPostProcessor);
}

match LlmPostProcessor::from_config(config) {
    Ok(processor) => Box::new(processor),
    Err(err) => {
        warn!(...);
        Box::new(NoopPostProcessor)
    }
}
```

这样可以把风险收口：

- 没配置好 LLM -> 不影响主流程
- API 短暂异常 -> 上层可回退原文

### 错误处理策略

推荐把后处理视为 **soft-fail enhancement**，不是 hard dependency。

| 场景 | 行为 |
|---|---|
| STT 成功，LLM 后处理成功 | 输出后处理结果 |
| STT 成功，LLM 后处理失败 | 记录 warning，输出原始 STT 结果 |
| STT 失败 | 与当前行为一致，直接报错 |

最重要的产品决策是：

> 后处理是“锦上添花”，不能变成“原本能用，现在因为 LLM 炸了所以整个不可用”。

## 模块改动点

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/transcriber/*` | 修改 | 将识别层整理为兼容 OpenAI 格式的流式 STT 路径，并暴露稳定的 partial/final text 接口 |
| `src/postprocess/mod.rs` | 新增 | 导出 trait、factory、实现 |
| `src/postprocess/llm.rs` | 新增 | `LlmPostProcessor` 实现 |
| `src/postprocess/factory.rs` | 新增 | 根据配置返回 `NoopPostProcessor` 或 `LlmPostProcessor` |
| `src/core/config.rs` | 修改 | 新增流式识别与后处理配置项、默认值、读取/保存、CLI 配置支持 |
| `src/main.rs` | 修改 | 在 `run_listener` / `convert` 中接入后处理层，并衔接流式 STT 输出 |
| `docs/architecture/core.md` | 修改 | 补充流式识别与后处理配置说明 |
| `docs/architecture/transcriber.md` 或新增文档 | 修改 | 说明流式识别、OpenAI 兼容格式与后处理的职责边界 |
| `README.md` | 修改 | 增加 OpenAI-compatible API 配置示例与功能说明 |
| `config.example.json` | 修改 | 增加流式识别与后处理示例配置 |

## 测试计划

### `src/core/config.rs`

- [ ] 默认配置中 `transcription_streaming_enabled == true`
- [ ] 默认配置中 `transcription_api_format == "openai"`
- [ ] 默认配置中 `post_process_enabled == false`
- [ ] `config get/set post_process_enabled` 可用
- [ ] `config get/set post_process_model` / `post_process_prompt` 可用
- [ ] `config get/set transcription_api_format` 可用
- [ ] 环境变量 `POST_PROCESS_API_KEY` 可覆盖配置
- [ ] 旧配置缺少新字段时仍能正常加载

### `src/transcriber/*`

- [ ] 流式识别路径能正确消费 OpenAI-compatible API 响应
- [ ] 语言识别/语言提示配置能透传到识别层
- [ ] partial/final 结果边界清晰，不会把未稳定文本直接交给后处理层

### `src/postprocess/factory.rs`

- [ ] 未启用时返回 `NoopPostProcessor`
- [ ] 已启用且配置完整时返回 `LlmPostProcessor`
- [ ] 配置不完整时自动降级为 `NoopPostProcessor`

### `src/postprocess/llm.rs`

- [ ] 能正确构造请求体
- [ ] 能解析成功响应
- [ ] API 4xx / 5xx / 非法 JSON 时返回错误
- [ ] 空响应或仅空白响应时有合理处理

### 集成验证

- [ ] `run_listener` 能消费流式 STT 的稳定文本并调用后处理器
- [ ] 后处理失败时仍会输出原始文本
- [ ] `convert` 子命令也能复用同一后处理逻辑
- [ ] OpenAI-compatible API 配置能同时覆盖识别层与后处理层
- [ ] `cargo test` 全绿

## 验收标准

1. 用户可以显式开启/关闭 LLM 文本后处理。
2. 不开启时，行为与当前版本完全一致。
3. 识别层优先通过 OpenAI-compatible streaming STT 降低延迟。
4. 开启后，最终输出文本会经过 LLM 整理。
5. LLM 失败不会导致整次录音/转写失败。
6. README 与配置模板包含可直接参考的 OpenAI-compatible API 示例。

## 分阶段实施

### Phase 1 — 流式识别前提与配置抽象

1. 明确识别层走 OpenAI-compatible streaming STT
2. 在 `AppConfig` 中新增流式识别与后处理相关配置字段
3. 规范 partial/final text buffer 与后处理的边界

**验收**：配置与接口边界明确；默认行为可保持兼容。

### Phase 2 — 抽象后处理层

1. 新增 `TextPostProcessor` trait 与 `NoopPostProcessor`
2. 在 `main.rs` 中接好调用点，但先默认只走 no-op
3. 确保后处理只消费稳定文本

**验收**：编译通过；默认行为完全不变。

### Phase 3 — 接入 LLM 后处理实现

1. 实现 `LlmPostProcessor`
2. 增加工厂函数与降级逻辑
3. 完成 OpenAI-compatible 请求 / 响应 / 错误处理测试

**验收**：启用配置后可得到整理后的文本；失败时回退原文。

### Phase 4 — 文档与示例

1. 更新 `README.md`
2. 更新架构文档
3. 更新 `config.example.json`
4. 补充示例 prompt

**验收**：新用户可按文档独立配置与验证。

## 风险与取舍

### 风险 1：LLM 可能“过度聪明”，改写超过用户预期

取舍：默认 prompt 强调“只整理，不扩写，不改变原意”，并允许用户自定义 prompt。

### 风险 2：额外后处理会增加整体延迟

取舍：识别层优先通过流式 API 降低前半段延迟；后处理默认关闭，启用后接受一次额外网络请求成本。

### 风险 3：后处理结果偶尔为空或格式异常

取舍：把原始 STT 文本作为最后兜底，不让空结果吞掉用户内容。

## 推荐 PR 策略

建议仍沿用“先 plan，再 implementation”：

1. **Plan PR（当前）**
   - 新增本文档
   - 更新 `docs/README.md`
   - 更新 `changelog`

2. **Implementation PR**
   - 先做流式识别前提与后处理层抽象
   - 再补 README / `config.example.json` / architecture 文档

这样 review 边界清楚，也避免再把“音频识别后端”和“文本后处理层”混成一锅。
