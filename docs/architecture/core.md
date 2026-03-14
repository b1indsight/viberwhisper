# Core Module Architecture

## Purpose

The `core` module (`src/core/`) contains two sub-modules: configuration persistence (`config.rs`) and CLI argument parsing (`cli.rs`).

---

## Config (`src/core/config.rs`)

### `AppConfig` Struct

```rust
pub struct AppConfig {
    pub api_key: Option<String>,           // not saved to file; from env or JSON
    pub transcription_api_url: String,     // full URL of the audio transcription endpoint
    pub provider: Option<String>,          // informational label only, not used for dispatch
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: f32,
    pub hold_hotkey: String,
    pub toggle_hotkey: String,
    pub mic_gain: f32,
}
```

Serialized to/from `config.json` via `serde_json`. `api_key` is excluded from the saved file (`#[serde(skip)]`) and loaded from the `GROQ_API_KEY` or `TRANSCRIPTION_API_KEY` environment variable instead.

**Defaults:**

| Field | Default |
|---|---|
| `transcription_api_url` | `"https://api.groq.com/openai/v1/audio/transcriptions"` |
| `model` | `"whisper-large-v3-turbo"` |
| `language` | `"zh"` |
| `temperature` | `0.0` |
| `hold_hotkey` | `"F8"` |
| `toggle_hotkey` | `"F9"` |
| `mic_gain` | `1.0` |

**`transcription_api_url`** points to the audio transcription HTTP endpoint (OpenAI-compatible multipart format). Changing this field is sufficient to switch providers — no code changes needed.

### Key Methods

**`AppConfig::load() -> Self`**

Loads config in priority order:
1. Defaults via `Default::default()`
2. `config.json` (partial override via `apply_json`)
3. `GROQ_API_KEY` env var → `api_key` (backward compat, lower priority)
4. `TRANSCRIPTION_API_KEY` env var → `api_key` (higher priority)

**`save(&self) -> Result<()>`**

Serializes to pretty-printed JSON. `api_key` is never written to disk (marked `#[serde(skip)]`).

**`get_field(&self, key: &str) -> Option<String>`**

Returns a string representation of the named field. Returns `"*** (set)"` for `api_key` / `groq_api_key` if present, `None` for unknown keys.

**`set_field(&mut self, key: &str, value: &str) -> Result<(), String>`**

Sets a field by name, auto-parsing float values for `temperature` and `mic_gain`. Returns an error string for unknown keys or invalid float values. `groq_api_key` is accepted as an alias for `api_key`.

**`apply_json(&mut self, json: &Value)`** *(private)*

Applies partial JSON overrides. Backward compatibility:
- Old `"hotkey"` key maps to `hold_hotkey`
- Old `"groq_api_key"` key maps to `api_key` (if `api_key` not already set)

---

## CLI (`src/core/cli.rs`)

### `Cli` Struct

```rust
pub struct Cli {
    pub command: Option<Commands>,
}
```

Parsed with `clap::Parser`. No subcommand runs the main recording loop.

### `Commands` Enum

| Variant | Description |
|---|---|
| `Config { action: ConfigAction }` | Configuration management subcommand |
| `Convert { input: String, output: Option<String> }` | Transcribe a WAV file to text |

### `ConfigAction` Enum

| Variant | Description |
|---|---|
| `List` | Print all config fields and current values |
| `Get { key: String }` | Print a single field value |
| `Set { key: String, value: String }` | Update a field and save |
