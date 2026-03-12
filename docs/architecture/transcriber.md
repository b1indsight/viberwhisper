# Transcriber Module Architecture

## Purpose

Converts a WAV file path to a transcribed text string. The module defines a trait and provides two implementations: a real Groq API client and a mock for testing.

## `Transcriber` Trait (`src/transcriber/groq.rs`)

```rust
pub trait Transcriber {
    fn transcribe(&self, wav_path: &str) -> Result<String, Box<dyn std::error::Error>>;
}
```

The single method takes a file path and returns the transcribed text or an error.

---

## `GroqTranscriber`

```rust
pub struct GroqTranscriber {
    api_key: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    temperature: f32,
}
```

### Construction

**`GroqTranscriber::from_config(config: &AppConfig) -> Result<Self>`**

Reads all fields from `AppConfig`. Returns an error if `groq_api_key` is not set.

### `transcribe` Implementation

1. Reads the WAV file into bytes.
2. Builds a `multipart/form-data` request with fields: `model`, `temperature`, `response_format=verbose_json`, optional `language` and `prompt`, and the `file` part.
3. POSTs to `https://api.groq.com/openai/v1/audio/transcriptions` with `Bearer` auth.
4. On non-2xx status, returns an error with status code and body.
5. Parses the JSON response and extracts the `text` field (trimmed).

**Dependencies:** `reqwest` (blocking client), `serde_json`

---

## `MockTranscriber`

```rust
pub struct MockTranscriber;
```

Returns the fixed string `"This is mock transcribed text"` without making any network calls or reading any file. Used in unit tests to isolate the transcription step.

---

## Module Exports (`src/transcriber/mod.rs`)

```rust
pub mod groq;
pub use groq::{GroqTranscriber, MockTranscriber, Transcriber};
```

All three symbols are re-exported at the module root.
