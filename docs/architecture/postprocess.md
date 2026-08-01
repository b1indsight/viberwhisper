# Post-processing Module Architecture

## Purpose

Optional LLM-based text cleanup applied after STT transcription. Adds punctuation, removes filler words, cleans up interruptions and repetitions while preserving original meaning.

## Module Layout

```
src/postprocess/
  mod.rs      — PostProcessor facade, session facade, typed config and errors
  llm.rs      — LlmPostProcessor, ConservativeLlmSession, PreheatLlmSession
```

## `PostProcessor` Facade

```rust
pub struct PostProcessor(/* private implementation enum */);

impl PostProcessor {
    pub fn new(config: PostProcessConfig) -> Self;
    pub fn process(&self, text: &str) -> Result<String, PostProcessError>;
    pub fn start_session(&self) -> PostProcessorSession;
}
```

Two interfaces for different use cases:
- `process`: one-shot processing for the `convert` CLI path
- `start_session`: incremental session for the `run_listener` path

The supported implementations are a closed set, so the module uses a concrete facade over a private enum instead of allocating boxed trait objects. Only the much larger LLM session variant uses `Box`, preventing the pass-through session from inheriting its stack size. Construction selects pass-through or LLM behavior from the validated config. If HTTP-client initialization fails, it logs the error and falls back to pass-through because cleanup is optional.

## `PostProcessorSession`

```rust
impl PostProcessorSession {
    pub fn push_stable_chunk(&mut self, text: &str);
    pub fn finish(&mut self) -> Result<String, PostProcessError>;
}
```

Designed for incremental input: stable STT chunks are pushed as they become available; `finish` returns the final processed text.

## Implementations

### Pass-through

Passes text through unchanged when post-processing is disabled. Its session simply concatenates all pushed chunks.

### `LlmPostProcessor`

Calls an OpenAI-compatible chat completions API to clean up transcribed text. This is the only supported post-processing API format.

**Construction:** `LlmPostProcessor::new(config: LlmConfig) -> Result<Self>`

`PostProcessConfig::validate` owns URL/model validation and produces either `Disabled` or a validated `LlmConfig`. Authentication is explicit: Local uses `ApiAuth::None`; API mode uses Bearer auth when enabled.

**`process` method:** Sends a single blocking request to the LLM API. Empty text is returned immediately without a network call.

**Session modes (controlled by `post_process.preheat_enabled`):**

| Mode | Config Value | Behavior |
|------|-------------|----------|
| Conservative | `false` | Accumulates all chunks, calls LLM once in `finish()` |
| Preheat | `true` (default) | Fires a background LLM request on every `push_stable_chunk()` call |

#### Conservative Mode (`ConservativeLlmSession`)

Simple accumulation: `push_stable_chunk` appends text to a `Vec<String>`, `finish` joins and calls LLM once. Zero token waste, but full LLM latency after recording ends.

#### Preheat Mode (`PreheatLlmSession`)

Reduces perceived latency by pre-firing LLM requests during recording:

- Each `push_stable_chunk` spawns a `std::thread` that sends a new LLM request with ALL accumulated text so far
- A generation counter (`u64`) tracks request freshness; only the latest generation's result is kept
- Shared state via `Arc<(Mutex<PreheatState>, Condvar)>`
- `finish()` blocks on the `Condvar` until the latest generation completes
- If the latest request fails, retries once with the full accumulated text (graceful degradation)
- Stale thread results (from superseded generations) are silently dropped

**Trade-off:** Intermediate requests waste tokens, but `finish()` returns near-instantly if the last request completed before recording stopped.

#### Default Prompt

```
请将下面的语音转写结果整理为适合直接发送的中文文本：
- 保留原意，不要扩写
- 添加自然标点
- 删除无意义语气词、重复和明显自我打断
- 若句子本身不完整，可做最小必要整理
- 只输出整理后的最终文本，不要解释
```

#### LLM Request Format

Non-streaming OpenAI chat completions (`"stream": false`):

```json
{
  "model": "<post_process_model>",
  "messages": [
    {"role": "system", "content": "<prompt>"},
    {"role": "user", "content": "<accumulated_text>"}
  ],
  "temperature": 0.0
}
```

## Error Handling

`PostProcessError` distinguishes HTTP transport, malformed JSON, API status, missing content, and empty content failures. Configuration errors are returned before processor construction. Runtime request failures are returned to the caller, which keeps the original STT text.

## Dependencies

| Crate | Usage |
|---|---|
| `reqwest` | Blocking HTTP client for LLM API calls |
| `serde_json` | JSON request/response serialization |
| `tracing` | Structured logging |
