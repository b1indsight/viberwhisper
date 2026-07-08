# Transcriber Module Architecture

## Purpose

Converts a WAV file path to a transcribed text string. The module defines a trait, provides a generic HTTP implementation with automatic chunking and retry, and exposes a factory function that creates a transcriber from config.

## Module Layout

```
src/transcriber/
  mod.rs      — re-exports all public symbols
  api.rs      — Transcriber trait + ApiTranscriber + MockTranscriber
  factory.rs  — create_transcriber factory function
```

## `Transcriber` Trait (`src/transcriber/api.rs`)

```rust
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, wav_path: &str) -> Result<String, TranscribeError>;
}
```

The single method takes a file path and returns the transcribed text or a structured error.

## `TranscribeError` (`src/transcriber/mod.rs`)

```rust
pub enum TranscribeError {
    Api { status: u16, body: String }, // HTTP error response (4xx / 5xx)
    Network(String),                   // network, I/O, or response-parsing failure
    Timeout,                           // convergence timeout (set by the orchestrator only)
}
```

Producers construct the variant at the failure site, so downstream layers (retry
logic, the session orchestrator) match on it directly instead of parsing error
strings. `core::orchestrator` re-exports it.

`mod.rs` also owns the shared text-merging helpers: `merge_texts(texts, language)`
and `merge_separator(language)` (used by the orchestrator, `ApiTranscriber`, and
the main loop's post-process feeder).

---

## Factory (`src/transcriber/factory.rs`)

```rust
pub fn create_transcriber(config: &AppConfig) -> Result<Box<dyn Transcriber>, Box<dyn std::error::Error>>
```

Creates an `ApiTranscriber` from `config.api_key` and `config.transcription_api_url`. In debug/test builds, falls back to `MockTranscriber` when no API key is configured. In release builds, returns the initialization error instead of silently using mock transcription.

| Condition | Result |
|---|---|
| `config.api_key` is set | `ApiTranscriber` |
| `config.api_key` is `None`, debug/test build | `MockTranscriber` (with a warning log) |
| `config.api_key` is `None`, release build | Error |

---

## `ApiTranscriber`

```rust
pub struct ApiTranscriber {
    api_key: String,
    api_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
    max_chunk_duration_secs: u32,
    max_chunk_size_bytes: u64,
    max_retries: u32,
    parallel_uploads: usize,           // from max_parallel_transcriptions
    client: reqwest::blocking::Client, // shared; 120s per-request timeout
}
```

A generic HTTP transcriber compatible with OpenAI-style multipart audio endpoints. All connection details come from config. The HTTP client is built once in `from_config` with a 120 s per-request timeout, so a hung request can never pin a worker thread forever, and connections are reused across chunk uploads.

### Construction

**`ApiTranscriber::from_config(config: &AppConfig) -> Result<Self>`**

Reads `config.api_key` (required), `config.transcription_api_url`, transcription fields, and chunk config fields.

### `transcribe` Implementation

1. Reads the WAV file and checks if it exceeds chunk limits.
2. If the file is within limits, sends a single multipart request.
3. If the file exceeds limits, splits it via `split_wav()` and transcribes each chunk individually.
4. Merges chunk results using language-aware text joining.

### Automatic Chunking

When a WAV file exceeds `max_chunk_duration_secs` or `max_chunk_size_bytes`:

1. The file is split into chunks using `audio::splitter::split_wav()`.
2. Chunks are uploaded in parallel with bounded concurrency (`parallel_uploads`, default 3).
3. Results are merged with `merge_texts()` in chunk order; the first error (by index) is returned if any chunk fails.

### Retry with Exponential Backoff

Each chunk upload retries on transient failures:

- **4xx errors**: Non-retryable (client errors, bad request). Fails immediately.
- **5xx errors**: Retryable (server errors). Retries up to `max_retries` times with exponential backoff (1s, 2s, 4s, ...).
- **Network errors**: Retryable. Same backoff strategy.

### Language-Aware Text Merging

`merge_texts(texts, language)` joins transcribed chunks:

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

Returns the fixed string `"This is mock transcribed text"` without making any network calls. Used in unit tests and as a runtime fallback when no API key is configured.

---

## Module Exports (`src/transcriber/mod.rs`)

```rust
pub mod api;
pub mod factory;
#[cfg(test)]
pub use api::MockTranscriber;
pub use api::Transcriber;
pub use factory::create_transcriber;
```

---

## Switching Endpoints

To use a different OpenAI-compatible transcription endpoint, set `transcription_api_url` in `config.json` or via `config set transcription_api_url <url>`. No code changes needed.

## Adding a New Provider Type

If a future provider requires a fundamentally different request format (not multipart):

1. Create `src/transcriber/<name>.rs` implementing `Transcriber`.
2. Add a `pub mod <name>;` line in `mod.rs`.
3. Update `factory.rs` to select the new implementation based on a config field.
4. Add any new config fields to `AppConfig` if needed.
