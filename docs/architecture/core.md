# Core Module Architecture

## Purpose

The `core` module (`src/core/`) contains three sub-modules: configuration persistence (`config.rs`), CLI argument parsing (`cli.rs`), and session orchestration (`orchestrator.rs`).

---

## Config (`src/core/config.rs`)

### `AppConfig` Struct

```rust
pub struct AppConfig {
    // --- Transcription (STT) ---
    pub api_key: Option<String>,              // not saved to file; from env or JSON
    pub transcription_api_url: String,        // full URL of the audio transcription endpoint
    pub provider: Option<String>,             // informational label only
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: f32,

    // --- Hotkeys ---
    pub hold_hotkey: String,
    pub toggle_hotkey: String,

    // --- Audio ---
    pub mic_gain: f32,
    pub max_chunk_duration_secs: u32,         // max seconds per audio chunk (default: 30)
    pub max_chunk_size_bytes: u64,            // max bytes per chunk incl. WAV header (default: 23 MiB)
    pub max_retries: u32,                     // max retry attempts per chunk upload (default: 3)
    pub convergence_timeout_secs: u64,        // session convergence timeout (default: 30)

    // --- LLM Post-processing ---
    pub post_process_enabled: bool,           // default: false
    pub post_process_streaming_enabled: bool, // default: true (preheat mode)
    pub post_process_api_url: Option<String>,
    pub post_process_api_key: Option<String>, // not saved to file
    pub post_process_api_format: String,      // default: "openai"
    pub post_process_model: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_temperature: f32,        // default: 0.0
}
```

Serialized to/from `config.json` via `serde_json`. `api_key` and `post_process_api_key` are excluded from the saved file (`#[serde(skip)]`).

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
| `max_chunk_duration_secs` | `30` |
| `max_chunk_size_bytes` | `24117248` (23 MiB) |
| `max_retries` | `3` |
| `convergence_timeout_secs` | `30` |
| `post_process_enabled` | `false` |
| `post_process_streaming_enabled` | `true` |
| `post_process_api_format` | `"openai"` |
| `post_process_temperature` | `0.0` |

### Key Methods

**`AppConfig::load() -> Self`**

Loads config in priority order:
1. Defaults via `Default::default()`
2. `config.json` (partial override via `apply_json`)
3. `GROQ_API_KEY` env var → `api_key` (backward compat, lower priority)
4. `TRANSCRIPTION_API_KEY` env var → `api_key` (higher priority)
5. `POST_PROCESS_API_KEY` env var → `post_process_api_key`

**`save(&self) -> Result<()>`**

Serializes to pretty-printed JSON. `api_key` and `post_process_api_key` are never written to disk.

**`get_field(&self, key: &str) -> Option<String>`**

Returns a string representation of the named field. Supported keys: `api_key`, `groq_api_key`, `transcription_api_url`, `provider`, `model`, `hold_hotkey`, `toggle_hotkey`, `temperature`, `mic_gain`, `language`, `prompt`, `max_chunk_duration_secs`, `max_chunk_size_bytes`, `max_retries`, `convergence_timeout_secs`, `post_process_enabled`, `post_process_streaming_enabled`, `post_process_api_url`, `post_process_api_key`, `post_process_api_format`, `post_process_model`, `post_process_prompt`, `post_process_temperature`. Returns `"*** (set)"` for API key fields if present; `None` for unknown keys.

**`set_field(&mut self, key: &str, value: &str) -> Result<(), String>`**

Sets a field by name with auto type conversion (strings, floats, bools, integers). `groq_api_key` is accepted as an alias for `api_key`.

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

---

## Orchestrator (`src/core/orchestrator.rs`)

### Purpose

`SessionOrchestrator` unifies the lifecycle of Hold and Toggle recording sessions, managing background transcription of audio chunks with convergence timeout and error handling.

### Key Concepts

- **Chunk State Machine**: `Flushed → Uploading → Transcribed / Failed`
- **Convergence Timeout**: Configurable via `convergence_timeout_secs`; chunks still pending at the deadline are marked `Failed(Timeout)`
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error

### `SessionError` Enum

| Variant | Description |
|---|---|
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, failures }` | Some chunks succeeded, includes partial text |
| `ConvergenceTimeout { partial_text, pending }` | Timeout hit, includes what was completed |
