# Transcriber Module Architecture

## Purpose

Converts a WAV file path to a transcribed text string. The module defines a trait, provides a generic HTTP implementation, and exposes a factory function that creates a transcriber from config — no provider name hardcoding required.

## Module Layout

```
src/transcriber/
  mod.rs      — re-exports all public symbols
  api.rs      — Transcriber trait + ApiTranscriber + MockTranscriber
  factory.rs  — create_transcriber factory function
```

## `Transcriber` Trait (`src/transcriber/api.rs`)

```rust
pub trait Transcriber {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>>;
}
```

The single method takes a file path and returns the transcribed text or an error.

---

## Factory (`src/transcriber/factory.rs`)

```rust
pub fn create_transcriber(config: &AppConfig) -> Box<dyn Transcriber>
```

Creates an `ApiTranscriber` from `config.api_key` and `config.transcription_api_url`. Falls back to `MockTranscriber` when no API key is configured.

**Dispatch logic:**

| Condition | Result |
|---|---|
| `config.api_key` is set | `ApiTranscriber` |
| `config.api_key` is `None` | `MockTranscriber` (with a warning log) |

`main.rs` calls `create_transcriber(&config)` — it has no direct dependency on any concrete transcriber type and no dependency on provider names.

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
}
```

A generic HTTP transcriber compatible with OpenAI-style multipart audio endpoints. All connection details come from config — no provider name is hardcoded in the struct or its constructor.

### Construction

**`ApiTranscriber::from_config(config: &AppConfig) -> Result<Self>`**

Reads `config.api_key` (required), `config.transcription_api_url`, and other transcription fields. Returns an error if `api_key` is not set.

### `transcribe` Implementation

1. Reads the WAV file into bytes.
2. Builds a `multipart/form-data` request with fields: `model`, `temperature`, `response_format=verbose_json`, optional `language` and `prompt`, and the `file` part.
3. POSTs to `config.transcription_api_url` with `Bearer` auth.
4. On non-2xx status, returns an error with status code and body.
5. Parses the JSON response and extracts the `text` field (trimmed).

**Dependencies:** `reqwest` (blocking client), `serde_json`

---

## `MockTranscriber`

```rust
pub struct MockTranscriber;
```

Returns the fixed string `"This is mock transcribed text"` without making any network calls or reading any file. Used in unit tests to isolate the transcription step, and as a runtime fallback when no valid API key is configured.

---

## Module Exports (`src/transcriber/mod.rs`)

```rust
pub mod api;
pub mod factory;
pub use api::{ApiTranscriber, MockTranscriber, Transcriber};
pub use factory::create_transcriber;
```

---

## Switching Endpoints

To use a different OpenAI-compatible transcription endpoint (e.g. OpenAI, a local whisper server), set `transcription_api_url` in `config.json` or via `config set transcription_api_url <url>`. No code changes needed.

## Adding a New Provider Type

If a future provider requires a fundamentally different request format (not multipart):

1. Create `src/transcriber/<name>.rs` implementing `Transcriber`.
2. Add a `pub mod <name>;` line in `mod.rs`.
3. Update `factory.rs` to select the new implementation based on a config field.
4. Add any new config fields to `AppConfig` if needed.
