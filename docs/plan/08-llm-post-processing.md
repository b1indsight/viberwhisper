# LLM 文本后处理

## 背景

Issue #16 需要的不是“让 LLM 直接识别音频”，而是在现有语音识别链路之后增加一层 **LLM 文本后处理**。

目标链路应为：

```text
audio -> speech-to-text -> raw text -> LLM post-process -> final text
```

当前 ViberWhisper 已经具备稳定的音频采集、分片、转写与文本注入链路。无论是短录音还是 `SessionOrchestrator` 驱动的长录音，会话最终都会收敛为一个 `String`，然后由 `TextTyper` 输出。

这意味着：

- **音频识别本身不是问题焦点**
- 问题在于 STT 原始输出往往接近口语草稿
- 用户希望在输入前再清洗一遍文本，使结果更接近可直接发送的书面表达

具体诉求包括：

- 添加标点
- 去掉不必要的语气词
- 去掉口误、自我打断和重复片段
- 保留原意，不凭空扩写

因此，这个 feature 更准确的定位是：

> 在转写结果与最终输入之间，增加一个可选的 **LLM rewrite/post-processing layer**。

## 目标

1. 保留现有 STT 流程不变，把 LLM 能力放在**文本后处理层**，而不是替换转写后端。
2. 允许用户通过配置开关启用/禁用后处理。
3. 支持为后处理单独配置模型、API 地址、提示词和温度等参数。
4. 后处理失败时，系统应优雅降级为输出原始转写文本，而不是整次会话失败。
5. 让 `run_listener` 与 `convert` 两条路径都能复用同一套后处理逻辑。
6. 补充测试与文档，明确该层的职责边界与失败策略。

## 非目标

- **不替换现有 `Transcriber`**：Whisper / Groq / OpenAI-compatible audio transcription 仍负责语音转文字。
- **不引入音频多模态 LLM 输入**：LLM 不直接接收音频文件。
- **不做实时边录边润色**：仍然在一次会话收敛后，对完整文本做单次后处理。
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

中间没有独立的文本修正阶段。

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

- **职责分离**：`Transcriber` 只负责把音频变成原始文本；新增组件负责把原始文本清洗成最终文本。
- **默认保守**：不开启配置时，行为与现在完全一致。
- **失败可降级**：后处理失败时，不影响原始转写结果输出。
- **接口轻量**：尽量以 `String -> String` 为核心抽象，避免过度设计。

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

建议在上层文本已经收敛、但尚未注入到输入框之前接入：

#### `run_listener` 路径

```text
录音结束
  -> orchestrator.stop_session()
  -> transcribed_text
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

- 同一套后处理逻辑覆盖交互录音和 CLI 转写
- `SessionOrchestrator` 完全不需要知道 LLM 的存在
- 音频相关模块不被文本清洗逻辑污染

### 配置设计

建议在 `src/core/config.rs` 的 `AppConfig` 中新增以下字段：

```rust
pub post_process_enabled: bool,
pub post_process_api_url: Option<String>,
pub post_process_api_key: Option<String>,
pub post_process_model: Option<String>,
pub post_process_prompt: Option<String>,
pub post_process_temperature: f32,
```

#### 默认值建议

| 字段 | 默认值 | 说明 |
|---|---|---|
| `post_process_enabled` | `false` | 默认关闭，保持现有行为 |
| `post_process_api_url` | `None` | 未启用时不需要 |
| `post_process_api_key` | `None` | 建议支持环境变量覆盖 |
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

后续用户可以通过 `post_process_prompt` 覆盖默认行为，例如：

- 更口语化
- 更书面化
- 保留 filler words
- 不改换行结构

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

本期建议保持 practical：

- 使用项目已经熟悉的 HTTP JSON 请求方式
- 选定一种明确的文本生成 API contract
- 不在第一期追求兼容所有 provider 的所有响应格式

换句话说，这里应该是“**先支持 1 条清晰且可测试的文本 rewrite 路径**”，而不是提前抽象成万能网关。

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

- 没配置好 LLM → 不影响主流程
- API 短暂异常 → 上层可决定是否回退原文

### 错误处理策略

推荐把后处理视为 **soft-fail enhancement**，不是 hard dependency。

也就是说：

| 场景 | 行为 |
|---|---|
| STT 成功，LLM 后处理成功 | 输出后处理结果 |
| STT 成功，LLM 后处理失败 | 记录 warning，输出原始 STT 结果 |
| STT 失败 | 与当前行为一致，直接报错 |

这是本 feature 最重要的产品决策之一：

> 后处理是“锦上添花”，不能变成“原本能用，现在因为 LLM 炸了所以整个不可用”。

## 模块改动点

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/postprocess/mod.rs` | 新增 | 导出 trait、factory、实现 |
| `src/postprocess/llm.rs` | 新增 | `LlmPostProcessor` 实现 |
| `src/postprocess/factory.rs` | 新增 | 根据配置返回 `NoopPostProcessor` 或 `LlmPostProcessor` |
| `src/core/config.rs` | 修改 | 新增后处理配置项、默认值、读取/保存、CLI 配置支持 |
| `src/main.rs` | 修改 | 在 `run_listener` / `convert` 中接入后处理层 |
| `docs/architecture/core.md` | 修改 | 补充后处理配置说明 |
| `docs/architecture/transcriber.md` 或新增文档 | 修改 | 说明转写与后处理的职责边界 |
| `README.md` | 修改 | 增加配置示例与功能说明 |
| `config.example.json` | 修改 | 增加后处理示例配置 |

## 测试计划

### `src/core/config.rs`

- [ ] 默认配置中 `post_process_enabled == false`
- [ ] `config get/set post_process_enabled` 可用
- [ ] `config get/set post_process_model` / `post_process_prompt` 可用
- [ ] 环境变量 `POST_PROCESS_API_KEY` 可覆盖配置
- [ ] 旧配置缺少新字段时仍能正常加载

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

- [ ] `run_listener` 成功转写后能调用后处理器
- [ ] 后处理失败时仍会输出原始文本
- [ ] `convert` 子命令也能复用同一后处理逻辑
- [ ] `cargo test` 全绿

## 验收标准

1. 用户可以显式开启/关闭 LLM 文本后处理。
2. 不开启时，行为与当前版本完全一致。
3. 开启后，最终输出文本会经过 LLM 整理。
4. LLM 失败不会导致整次录音/转写失败。
5. README 与配置模板包含可直接参考的使用示例。

## 分阶段实施

### Phase 1 — 抽象后处理层

1. 新增 `TextPostProcessor` trait 与 `NoopPostProcessor`
2. 在 `main.rs` 中接好调用点，但先默认只走 no-op
3. 在 `AppConfig` 中新增后处理相关配置字段

**验收**：编译通过；默认行为完全不变。

### Phase 2 — 接入 LLM 后处理实现

1. 实现 `LlmPostProcessor`
2. 增加工厂函数与降级逻辑
3. 完成请求 / 响应 / 错误处理测试

**验收**：启用配置后可得到整理后的文本；失败时回退原文。

### Phase 3 — 文档与示例

1. 更新 `README.md`
2. 更新架构文档
3. 更新 `config.example.json`
4. 补充示例 prompt

**验收**：新用户可按文档独立配置与验证。

## 风险与取舍

### 风险 1：LLM 可能“过度聪明”，改写超过用户预期

取舍：默认 prompt 强调“只整理，不扩写，不改变原意”，并允许用户自定义 prompt。

### 风险 2：后处理会增加整体延迟

取舍：默认关闭；启用后接受一次额外网络请求成本。必要时后续再考虑更小模型或开关策略。

### 风险 3：后处理结果偶尔为空或格式异常

取舍：把原始 STT 文本作为最后兜底，不让空结果吞掉用户内容。

## 推荐 PR 策略

建议仍沿用“先 plan，再 implementation”：

1. **Plan PR（当前）**
   - 新增本文档
   - 更新 `docs/README.md`
   - 更新 `changelog`

2. **Implementation PR**
   - 先做 Phase 1 + Phase 2
   - 再补 README / config example / architecture 文档

这样 review 边界清楚，也避免再把“音频识别后端”和“文本后处理层”混成一锅。