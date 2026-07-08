# Post-processing Module Architecture

## Purpose

Optional LLM-based text cleanup applied after STT transcription. Adds punctuation, removes filler words, cleans up interruptions and repetitions while preserving original meaning.

## Module Layout

```
src/postprocess/
  mod.rs      — traits (TextPostProcessor, TextPostProcessorSession), NoopPostProcessor
  llm.rs      — LlmPostProcessor, ConservativeLlmSession, PreheatLlmSession
  factory.rs  — create_post_processor factory function
```

## `TextPostProcessor` Trait

```rust
pub trait TextPostProcessor: Send + Sync {
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>>;
    fn start_session(&self) -> Box<dyn TextPostProcessorSession>;
}
```

Two interfaces for different use cases:
- `process`: one-shot processing for the `convert` CLI path
- `start_session`: incremental session for the `run_listener` path

## `TextPostProcessorSession` Trait

```rust
pub trait TextPostProcessorSession: Send {
    fn push_stable_chunk(&mut self, text: &str);
    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>>;
}
```

Designed for incremental input: stable STT chunks are pushed as they become available; `finish` returns the final processed text. Fragments are concatenated verbatim — the caller includes inter-chunk separators (`PostFeed` in `main.rs` owns the language-specific spacing).

**Main-loop wiring:** the session is opened when recording starts. While recording continues, the main loop polls `SessionOrchestrator::take_stable_texts()` and pushes each newly stable chunk text; after `stop_session()`, only the unconsumed remainder (`SessionOutput::unconsumed_text`) is pushed before `finish()`.

## Implementations

### `NoopPostProcessor`

Passes text through unchanged. Used when post-processing is disabled or as a fallback when LLM configuration is incomplete. Its session simply concatenates all pushed chunks.

### `LlmPostProcessor`

Calls an OpenAI-compatible chat completions API to clean up transcribed text. This is the only supported post-processing API format.

**Construction:** `LlmPostProcessor::from_config(config) -> Result<Self>`

Requires `post_process_api_key`, `post_process_api_url`, and `post_process_model` to be configured. Returns an error if any are missing.

**`process` method:** Sends a single blocking request to the LLM API. Empty text is returned immediately without a network call.

**Retry:** every LLM call retries once on transient failures (network errors, HTTP 5xx) after a short delay; 4xx and malformed responses fail immediately (the caller's fallback keeps the raw STT text). Classification uses an internal structured `LlmCallError`, not string parsing.

**Session modes (controlled by `post_process_streaming_enabled`):**

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
- `finish()` waits on the `Condvar` with a bounded timeout (client timeout + margin); on timeout or failure it falls back to a synchronous full-text request
- Stale thread results (from superseded generations) are silently dropped

**Compatibility note:** the chat-completions endpoint has no incremental input channel, so "streaming" here means re-sending the full accumulated text per stable chunk and letting the newest generation win. If the backend ever exposes a true incremental interface, only this session type needs replacing — the trait contract and main-loop feeding stay the same.

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

## Factory (`src/postprocess/factory.rs`)

```rust
pub fn create_post_processor(config: &AppConfig) -> Box<dyn TextPostProcessor>
```

| Condition | Result |
|-----------|--------|
| `post_process_enabled = false` | `NoopPostProcessor` |
| `post_process_enabled = true`, config valid | `LlmPostProcessor` |
| `post_process_enabled = true`, config invalid | `NoopPostProcessor` (with warning log) |

Ensures the main pipeline is never blocked by a missing or broken LLM setup.

Configuration errors fall back to `NoopPostProcessor` because post-processing is optional. Runtime LLM request failures and empty LLM outputs are returned to the caller, which keeps the original STT text.

## Dependencies

| Crate | Usage |
|---|---|
| `reqwest` | Blocking HTTP client for LLM API calls |
| `serde_json` | JSON request/response serialization |
| `tracing` | Structured logging |
