# Transcriber Module Architecture

## Purpose

Converts a WAV file path to a transcribed text string. The module defines a trait, provides concrete implementations per provider, and exposes a factory function that selects the right implementation based on configuration.

## Module Layout

```
src/transcriber/
  mod.rs      — re-exports all public symbols
  groq.rs     — Transcriber trait + GroqTranscriber + MockTranscriber
  factory.rs  — create_transcriber factory function
```

## `Transcriber` Trait (`src/transcriber/groq.rs`)

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

Selects a `Transcriber` implementation based on `config.provider`. This is the single extension point for adding new providers: add a new match arm that constructs the appropriate `Box<dyn Transcriber>`.

**Current provider routing:**

| `config.provider` | Result |
|---|---|
| `"groq"` | `GroqTranscriber` if API key is set, else `MockTranscriber` |
| anything else | `MockTranscriber` (with a warning log) |

`main.rs` calls `create_transcriber(&config)` — it has no direct dependency on any concrete transcriber type.

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

Returns the fixed string `"This is mock transcribed text"` without making any network calls or reading any file. Used in unit tests to isolate the transcription step, and as a runtime fallback when no valid provider is configured.

---

## Module Exports (`src/transcriber/mod.rs`)

```rust
pub mod factory;
pub mod groq;
pub use factory::create_transcriber;
pub use groq::{GroqTranscriber, MockTranscriber, Transcriber};
```

---

## Adding a New Provider

1. Create `src/transcriber/<name>.rs` implementing `Transcriber`.
2. Add a `pub mod <name>;` line in `mod.rs`.
3. Add a match arm in `factory.rs`:
   ```rust
   "<name>" => Box::new(YourTranscriber::from_config(config)?),
   ```
4. Add the new provider's config fields to `AppConfig` if needed.
