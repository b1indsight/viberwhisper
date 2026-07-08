# Core Module Architecture

## Purpose

The `core` module (`src/core/`) contains three sub-modules: configuration persistence (`config.rs`), CLI argument parsing (`cli.rs`), and session orchestration (`orchestrator.rs`). It also serves as the boundary where local-mode runtime settings are carried into the main loop.

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
    pub max_retries: u32,                     // max retry attempts (default: 3, max: 16)
    pub convergence_timeout_secs: u64,        // timeout (default: 30s, max: 3600s)

    // --- LLM Post-processing ---
    pub post_process_enabled: bool,           // default: false
    pub post_process_streaming_enabled: bool, // default: true (preheat mode)
    pub post_process_api_url: Option<String>,
    pub post_process_api_key: Option<String>, // not saved to file
    pub post_process_model: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_temperature: f32,        // default: 0.0

    // --- Local runtime ---
    pub local_mode: bool,                     // default: false
    pub local_data_dir: Option<String>,       // default: ~/.viberwhisper
    pub local_server_port: u16,               // default: 17265
    pub local_quantization: String,           // default: "int8"

    // --- Concurrency ---
    pub max_parallel_transcriptions: u32,     // default: 3, range 1..=16
}
```

Serialized to/from `config.json` via `serde_json`. `api_key` and `post_process_api_key` are excluded from the saved file (`#[serde(skip)]`).

**File location:** a `config.json` in the current working directory takes precedence (developer workflow / existing setups); otherwise the per-user config directory `<config_dir>/viberwhisper/config.json` is used, so a bundled app launched from Finder/Explorer (cwd = `/`) still finds its config. `save()` writes to the same resolved path and creates the directory if needed.

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
| `post_process_temperature` | `0.0` |
| `local_mode` | `false` |
| `local_server_port` | `17265` |
| `local_quantization` | `"int8"` |
| `max_parallel_transcriptions` | `3` |

### Key Methods

**`AppConfig::load() -> Self`**

Loads config in priority order:
1. Defaults via `Default::default()`
2. `config.json` (partial override via `apply_json`)
3. `GROQ_API_KEY` env var → `api_key` (backward compat, lower priority)
4. `TRANSCRIPTION_API_KEY` env var → `api_key` (higher priority)
5. `POST_PROCESS_API_KEY` env var → `post_process_api_key`

**`save(&self) -> Result<()>`**

Serializes to pretty-printed JSON. Runtime/environment secrets are never introduced into the file. If `api_key`, legacy `groq_api_key`, or `post_process_api_key` already exists in `config.json`, it is preserved when other fields are updated.

**Field table (`FieldSpec`)**

All user-facing config access is driven by one `FIELDS` table: `get_field`, `set_field`, the lenient `apply_json` loader, and the CLI `config list` (via `AppConfig::field_keys()`) derive from it, so adding a config field means adding exactly one table entry (plus the struct field itself).

- `get_field(key)` returns a string representation; `"*** (set)"` for secret fields; `None` for unknown keys. `groq_api_key` is a read alias for `api_key`.
- `set_field(key, value)` converts and validates: non-finite floats are rejected, `max_retries` ≤ 16, `convergence_timeout_secs` ≤ 3600, `max_parallel_transcriptions` in 1..=16. Secret fields cannot be set through the CLI; use the documented environment variables or edit `config.json` directly.
- `apply_json(json)` *(private)* loads `config.json` leniently: each present field is applied through the same setters, and an invalid or out-of-range value warns and keeps the default rather than discarding the whole file. Secrets have loader overrides (readable from the file even though the CLI setter rejects them). Backward compatibility: `"hotkey"` maps to `hold_hotkey`; `"groq_api_key"` maps to `api_key` (canonical keys win when both are present).

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
| `Local { action: LocalCommand }` | Local Gemma runtime lifecycle commands |
| `Convert { input: String, output: Option<String> }` | Transcribe a WAV file to text |

### `ConfigAction` Enum

| Variant | Description |
|---|---|
| `List` | Print all config fields and current values |
| `Get { key: String }` | Print a single field value |
| `Set { key: String, value: String }` | Update a field and save |

### `LocalCommand` Enum

| Variant | Description |
|---|---|
| `Install` | Create venv, install Python dependencies, download model, and verify install |
| `Start` | Force `local_mode = true`, start local server, then enter the normal listener loop |
| `Stop` | Stop the persisted local server process |
| `Status` | Print runtime state, pid, port, memory usage, and `/health` result |

---

## Orchestrator (`src/core/orchestrator.rs`)

### Purpose

`SessionOrchestrator` unifies the lifecycle of Hold and Toggle recording sessions, managing background transcription of audio chunks with convergence timeout and error handling.

### Key Concepts

- **Chunk State Machine**: `Flushed → Uploading → Transcribed / Failed`
- **Parallel Workers**: Each session runs `max_parallel_transcriptions` (default 3) worker threads sharing one bounded queue; results are merged in submission order via chunk indices regardless of completion order.
- **Convergence Wait**: `stop_session` waits on a condvar signalled at every terminal chunk transition (no polling); chunks still pending at the `convergence_timeout_secs` deadline are marked `Failed(Timeout)`.
- **Streaming Consumption**: `take_stable_texts()` hands out, exactly once and in submission order, the texts of the maximal terminal chunk prefix — the main loop feeds these to the LLM post-process preheat while recording. `stop_session` returns `SessionOutput { full_text, unconsumed_text }` so an incremental session only needs the remainder.
- **Structured Errors**: Workers record the transcriber's `TranscribeError` (`Api { status, body }` / `Network` / `Timeout`) directly; no error-string parsing.
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error
- **Bounded Queue**: Chunk submission is non-blocking. A full queue marks the chunk failed and deletes its temporary file rather than stalling session shutdown.
- **Cleanup Guarantee**: Processed, rejected, orphaned, timed-out queued, and panicking-transcriber chunk paths are cleaned up by the orchestrator.

### `SessionError` Enum

| Variant | Description |
|---|---|
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, failures }` | Some chunks succeeded, includes partial text |
| `ConvergenceTimeout { partial_text, pending }` | Timeout hit, includes what was completed |

## Main Integration Notes

Although the main event loop lives in `src/main.rs`, `core` owns the configuration and CLI abstractions that feed it:

- `run_listener_with_config(config)` is the common entry point for both default mode and `local start`
- when `config.local_mode` is true, startup first calls into the `local` module to ensure the runtime exists, always rewrites the transcription endpoint, and conditionally rewrites the post-process endpoint when `post_process_enabled` is on
- the same orchestrator pipeline is reused regardless of whether the backend is Groq/OpenAI-compatible cloud STT or the local Gemma service
