# LLM 文本后处理

## 背景

Issue #16 需要的不是“让 LLM 直接识别音频”，而是在现有语音识别链路之后增加一层 **LLM 文本后处理**。

结合 review 反馈，这条链路应明确拆成三层：

```text
audio -> current STT interface -> stable text chunks / final text -> streaming LLM post-process -> final text
```

这里有三个关键点：

1. **保持现有 STT 接口不变**：继续以当前 `Transcriber::transcribe(...) -> String` 作为识别层契约，不在本期调整协议、provider 兼容面或 CLI 表面。
2. **LLM 只负责文本后处理**：例如补标点、去掉无意义语气词、清理中断与重复，不直接替代音频识别层。
3. **后处理尽量流式化以降低延迟**：`run_listener` 路径应支持把 `SessionOrchestrator` 已经稳定的分段文本持续喂给后处理层；后处理层再以流式方式请求 LLM API，尽量在录音尚未完全结束前就完成一部分整理工作。`convert` 路径则可直接一次性处理整段文本。

当前 ViberWhisper 已经具备稳定的音频采集、分片、转写与文本注入链路。无论是短录音还是 `SessionOrchestrator` 驱动的长录音，会话最终都会收敛为文本并输出。因此，这个 feature 更准确的定位是：

> 在“现有 STT 输出”与“最终输入文本”之间，增加一个可选的、支持增量处理的 **LLM rewrite/post-processing layer**。

## 目标

1. 在**不改变现有 STT 接口**的前提下，为后处理层预留“稳定文本分段输入 / 最终收口”的接入点。
2. 在 STT 之后增加独立的 **LLM 文本后处理层**，负责补标点、去语气词、清理中断与重复。
3. 允许用户通过配置开关启用/禁用后处理。
4. 支持为后处理配置模型、API 地址、提示词、温度，以及是否启用流式调用，并优先直接兼容 OpenAI 格式 API。
5. 后处理失败时，系统应优雅降级为输出原始 STT 文本，而不是整次会话失败。
6. 让 `run_listener` 与 `convert` 两条路径都能复用同一套后处理逻辑，其中前者优先走增量输入，后者保留整段输入。
7. 补充测试与文档，明确“现有 STT 接口不变”与“后处理增量降延迟”这两个边界。

## 非目标

- **不让 LLM 直接替代 STT 的职责**：语音转文字仍然是识别层的工作，LLM 负责文本整理。
- **不引入音频多模态 LLM 输入**：后处理层不直接接收音频文件。
- **不改现有 STT 接口**：这次计划不调整识别层协议、流式接口或 provider 兼容面。
- **不尝试做复杂 NLP 管线**：例如句法分析、关键词提取、风格模板链等，都不在本期范围内。
- **不强制所有用户使用 LLM 后处理**：默认应保持关闭，避免破坏当前体验与成本预期。

## 当前限制

### 1. 转写结果直接进入输出层

当前主要链路大致是：

```text
AudioRecorder / SessionOrchestrator
  -> Transcriber::transcribe(...)
  -> String
  -> TextTyper::type_text(...)
```

即使识别成功，中间也没有独立的文本修正阶段。

### 2. 配置层没有“后处理”概念

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

### 3. 失败语义过于粗糙

如果未来把 LLM 后处理硬塞进现有转写器内部：

- 会把“识别失败”和“润色失败”混成一类错误
- 上层无法明确选择是否回退到原始文本

这会让主流程的错误处理变脏。

## 架构方案

### 设计原则

- **职责分离**：STT 继续负责把音频转成原始文本；后处理层只负责整理文本。
- **接口稳定**：保留现有 `Transcriber::transcribe(...) -> String` 契约，不为了后处理去改 STT 边界。
- **增量优先**：`run_listener` 路径尽量把已经稳定的分段文本提早送入后处理，减少“录音结束后再统一润色”的等待。
- **默认保守**：不开启配置时，行为与现在完全一致。
- **失败可降级**：后处理失败时，不影响原始转写结果输出。

### 接入前提：复用现有 STT 输出

当前代码里的公开识别契约仍然是：

```text
Transcriber::transcribe(path) -> String
```

因此本计划不要求改造 `src/transcriber/*` 的对外接口，而是把增量能力放在 **STT 结果进入后处理层之前**：

- `convert` 路径：沿用整段 `String` 输入，作为最简单的 fallback。
- `run_listener` 路径：由 `SessionOrchestrator` 在 chunk 收敛后，把**已经稳定的文本片段**持续交给后处理层。
- 后处理层内部再决定是一次性请求 LLM，还是以流式方式把这些稳定片段送进 OpenAI-compatible API。

职责关系：

```text
current STT interface
  -> stable text chunks / final text
  -> post-processor session
  -> final text
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

为了同时覆盖 `run_listener` 的增量路径和 `convert` 的整段路径，建议把接口拆成“processor + session”两层：

```rust
pub trait TextPostProcessor: Send + Sync {
    fn start_session(&self) -> Result<Box<dyn TextPostProcessorSession>, Box<dyn std::error::Error>>;
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>>;
}

pub trait TextPostProcessorSession: Send {
    fn push_stable_chunk(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>>;
}
```

其中：

- `process(&str)` 给 `convert` 或其它整段输入路径使用。
- `start_session() + push_stable_chunk() + finish()` 给 `run_listener` 的增量场景使用。
- `NoopPostProcessor` 两种路径都直接透传原文。
- `LlmPostProcessor` 可以在 session 内部把 stable chunk 聚合后，以流式方式请求 LLM API，并在 `finish()` 时返回最终收口文本。

### 主流程接入点

建议在上层已经拿到**稳定文本**、但尚未注入到输入框之前接入。

#### `run_listener` 路径

```text
chunk transcribed and converged
  -> stable text chunk
  -> post_process_session.push_stable_chunk(chunk)
  -> (optional) streaming LLM request progresses in background
recording stops
  -> post_process_session.finish()
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
- `SessionOrchestrator` 只负责提供稳定文本，不需要理解 LLM 协议细节。
- 音频相关模块不被文本清洗逻辑污染。
- 不改变现有 STT 接口，也能把一部分后处理开销前移到录音进行中。

### 配置设计

建议只新增**后处理层配置**，不要为了这个 feature 去扩展现有 STT 接口配置面。

建议在 `src/core/config.rs` 的 `AppConfig` 中新增以下字段：

```rust
pub post_process_enabled: bool,
pub post_process_streaming_enabled: bool,
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
| `post_process_enabled` | `false` | 默认关闭，保持现有行为 |
| `post_process_streaming_enabled` | `true` | 启用后优先采用增量输入 + 流式 LLM 调用 |
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

本期建议**后处理层直接兼容 OpenAI 格式 API**，而不是重新定义一套自有协议。

- STT 层：维持当前接口与 provider 兼容面，不在本期借这个 feature 额外扩张范围
- 后处理层：优先对接 OpenAI-compatible text generation / streaming API
- 配置层通过 `post_process_api_url` + `post_process_api_format = "openai"` 保持明确语义
- 第一阶段先把 OpenAI 格式跑通；其他 provider 如有必要再在此基础上扩展

换句话说，这里不是“顺手重做整套识别协议”，而是“**在现有 STT 边界之外，把后处理层的 OpenAI 兼容流式调用先跑通**”。

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
| `src/postprocess/mod.rs` | 新增 | 导出 `TextPostProcessor` / `TextPostProcessorSession`、factory、实现 |
| `src/postprocess/llm.rs` | 新增 | `LlmPostProcessor` 实现，支持整段处理与增量 session |
| `src/postprocess/factory.rs` | 新增 | 根据配置返回 `NoopPostProcessor` 或 `LlmPostProcessor` |
| `src/core/config.rs` | 修改 | 新增后处理配置项、默认值、读取/保存、CLI 配置支持 |
| `src/core/orchestrator.rs` | 修改 | 暴露稳定文本片段的接入点或回调，不改变 `Transcriber` trait |
| `src/main.rs` | 修改 | 在 `run_listener` / `convert` 中接入后处理层；前者走 session，后者走整段 `process` |
| `docs/architecture/core.md` | 修改 | 补充后处理配置说明 |
| `docs/architecture/transcriber.md` 或新增文档 | 修改 | 明确“STT 接口不变、后处理增量接入”的职责边界 |
| `README.md` | 修改 | 增加后处理 OpenAI-compatible API 配置示例与功能说明 |
| `config.example.json` | 修改 | 增加后处理示例配置 |

## 测试计划

### `src/core/config.rs`

- [ ] 默认配置中 `post_process_enabled == false`
- [ ] 默认配置中 `post_process_streaming_enabled == true`
- [ ] `config get/set post_process_enabled` 可用
- [ ] `config get/set post_process_model` / `post_process_prompt` 可用
- [ ] `config get/set post_process_streaming_enabled` 可用
- [ ] 环境变量 `POST_PROCESS_API_KEY` 可覆盖配置
- [ ] 旧配置缺少新字段时仍能正常加载

### `src/postprocess/factory.rs`

- [ ] 未启用时返回 `NoopPostProcessor`
- [ ] 已启用且配置完整时返回 `LlmPostProcessor`
- [ ] 配置不完整时自动降级为 `NoopPostProcessor`

### `src/postprocess/llm.rs`

- [ ] `process(text)` 能正确构造请求体并解析响应
- [ ] session 模式下 `push_stable_chunk()` + `finish()` 能得到一致结果
- [ ] 流式响应能逐步消费并正确收口
- [ ] API 4xx / 5xx / 非法 JSON 时返回错误
- [ ] 空响应或仅空白响应时有合理处理

### `src/core/orchestrator.rs` / `src/main.rs`

- [ ] `run_listener` 能把稳定文本片段交给后处理 session，而不修改 `Transcriber` 接口
- [ ] `convert` 子命令能复用同一后处理逻辑
- [ ] 后处理失败时仍会输出原始文本
- [ ] 整个录音结束后的额外等待时间较“录音结束后再统一后处理”方案更短或不更差

### 集成验证

- [ ] 开启后处理后，最终输出文本会经过 LLM 整理
- [ ] 关闭后处理后，行为与当前版本一致
- [ ] `cargo test` 全绿

## 验收标准

1. 用户可以显式开启/关闭 LLM 文本后处理。
2. 不开启时，行为与当前版本完全一致。
3. 现有 `Transcriber::transcribe(...) -> String` 接口保持不变。
4. `run_listener` 可以在录音期间持续把稳定文本片段送入后处理层，以降低录音结束后的额外等待。
5. 开启后，最终输出文本会经过 LLM 整理。
6. LLM 失败不会导致整次录音/转写失败。
7. README 与配置模板包含可直接参考的 OpenAI-compatible API 示例。

## 分阶段实施

### Phase 1 — 明确边界与配置

1. 固定“STT 接口不变”的实现边界
2. 在 `AppConfig` 中新增后处理相关配置字段
3. 明确稳定文本片段与后处理 session 的衔接方式

**验收**：配置与接口边界明确；默认行为保持兼容。

### Phase 2 — 抽象后处理层

1. 新增 `TextPostProcessor` / `TextPostProcessorSession`
2. 先实现 `NoopPostProcessor`
3. 在 `main.rs` 中接好 `convert` 与 `run_listener` 的调用点

**验收**：编译通过；默认行为完全不变。

### Phase 3 — 接入流式 LLM 后处理

1. 实现 `LlmPostProcessor`
2. 增加工厂函数与降级逻辑
3. 在 `run_listener` 中把稳定文本片段增量送入 session
4. 完成 OpenAI-compatible 请求 / 响应 / 错误处理测试

**验收**：启用配置后可得到整理后的文本；失败时回退原文；录音结束后的等待时间可控。

### Phase 4 — 文档与示例

1. 更新 `README.md`
2. 更新架构文档
3. 更新 `config.example.json`
4. 补充示例 prompt

**验收**：新用户可按文档独立配置与验证。

## 风险与取舍

### 风险 1：LLM 可能“过度聪明”，改写超过用户预期

取舍：默认 prompt 强调“只整理，不扩写，不改变原意”，并允许用户自定义 prompt。

### 风险 2：额外后处理仍可能增加整体延迟

取舍：通过“稳定文本片段增量输入 + LLM 流式调用”把一部分成本前移到录音进行中；后处理默认关闭，启用后仍接受一定额外网络与推理开销。

### 风险 3：后处理结果偶尔为空或格式异常

取舍：把原始 STT 文本作为最后兜底，不让空结果吞掉用户内容。

## 推荐 PR 策略

建议仍沿用“先 plan，再 implementation”：

1. **Plan PR（当前）**
   - 新增本文档
   - 更新 `docs/README.md`
   - 更新 `changelog`

2. **Implementation PR**
   - 先做后处理层抽象与 `run_listener` 的增量接入
   - 再补 README / `config.example.json` / architecture 文档

这样 review 边界清楚，也避免再把“音频识别后端”和“文本后处理层”混成一锅。
