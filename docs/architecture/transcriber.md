# Transcriber Module Architecture

## Purpose

Converts a WAV file path to a transcribed text string. The module defines a trait for orchestrator injection and provides a generic HTTP implementation with automatic chunking and retry.

## Module Layout

```
src/
  text.rs         — shared language-aware transcription text merging
  transcriber/
    mod.rs        — re-exports all public symbols
    api.rs        — Transcriber trait + ApiTranscriber + MockTranscriber
```

## `Transcriber` Trait (`src/transcriber/api.rs`)

```rust
pub trait Transcriber {
    fn transcribe(&self, wav_path: &str) -> Result<String, TranscribeError>;
}
```

The single method takes a file path and returns the transcribed text or an error.

## `ApiTranscriber`

```rust
pub struct ApiTranscriber {
    auth: ApiAuth,
    api_url: Url,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    chunk_limits: ChunkLimits,
    max_retries: u32,
}
```

A generic HTTP transcriber compatible with OpenAI-style multipart audio endpoints. All connection details come from config.

### Construction

**`ApiTranscriber::new(config: TranscriberConfig) -> Result<Self, reqwest::Error>`**

Consumes the assembled endpoint, authentication mode, model, transcription options, limits, and retry count owned by the transcriber. `ApiAuth::None` omits the `Authorization` header; `ApiAuth::Bearer` sends it. Listener assembly wraps the concrete value in `Arc<dyn Transcriber>` because the orchestrator needs runtime injection; one-shot conversion keeps the concrete type.

### `transcribe` Implementation

1. Reads the WAV file and checks if it exceeds chunk limits.
2. If the file is within limits, sends a single multipart request.
3. If the file exceeds limits, splits it via `split_wav()` and transcribes each chunk individually.
4. Merges chunk results using language-aware text joining.

### Automatic Chunking

When a WAV file exceeds `max_chunk_duration_secs` or `max_chunk_size_bytes`:

1. The file is split into chunks using `audio::splitter::split_wav()`.
2. Each chunk is transcribed independently.
3. Results are merged with `merge_texts()`.

### Retry with Exponential Backoff

Each chunk upload retries on transient failures:

- **4xx errors**: Non-retryable (client errors, bad request). Fails immediately.
- **5xx errors**: Retryable (server errors). Retries up to `max_retries` times with exponential backoff (1s, 2s, 4s, ...).
- **Network errors**: Retryable. Same backoff strategy.

### Language-Aware Text Merging

`crate::text::merge_texts(texts, language)` is shared by `ApiTranscriber` and
`SessionOrchestrator` when joining transcribed chunks:

- **Chinese** (`zh`, `zh-cn`, etc.): Joins without separator (Chinese text doesn't use spaces between words).
- **Other languages**: Joins with a single space.
- Empty segments are filtered out before joining.

### Request Format

Multipart POST with fields: `model`, `temperature`, `response_format=verbose_json`, optional `language` and `prompt`, and the `file` part.

**Dependencies:** `reqwest` (blocking client), `serde_json`

---

## `MockTranscriber`

```rust
pub struct MockTranscriber;
```

Returns the fixed string `"This is mock transcribed text"` without making network calls. It is compiled only for unit tests.

---

## Module Exports (`src/transcriber/mod.rs`)

```rust
pub mod api;
#[cfg(test)]
pub use api::MockTranscriber;
pub use api::{ApiTranscriber, Transcriber, TranscriberConfig};
```

---

## Switching Endpoints

To use a different OpenAI-compatible endpoint, set `inference.api.transcription.api_url` and `inference.api.transcription.model`, then supply `TRANSCRIPTION_API_KEY`.

## Adding a New Provider Type

If a future provider requires a fundamentally different request format (not multipart):

1. Create `src/transcriber/<name>.rs` implementing `Transcriber`.
2. Add a `pub mod <name>;` line in `mod.rs`.
3. Add runtime selection only when a second implementation actually requires it.
4. Add persisted fields to `ConfigDocument`/the canonical field catalog and validate them into `TranscriberConfig`.
