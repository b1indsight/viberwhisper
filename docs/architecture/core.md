# Core Module Architecture

## Purpose

The `core` module (`src/core/`) contains configuration persistence (`config.rs`), CLI argument parsing (`cli.rs`), recording lifecycle state (`recording_session.rs`), and transcription orchestration (`orchestrator.rs`). It also serves as the boundary where local-mode runtime settings are carried into the main loop.

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
| `post_process_temperature` | `0.0` |
| `local_mode` | `false` |
| `local_server_port` | `17265` |
| `local_quantization` | `"int8"` |

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

**`get_field(&self, key: &str) -> Option<String>`**

Returns a string representation of the named field. Supported keys: `api_key`, `groq_api_key`, `transcription_api_url`, `provider`, `model`, `hold_hotkey`, `toggle_hotkey`, `temperature`, `mic_gain`, `language`, `prompt`, `max_chunk_duration_secs`, `max_chunk_size_bytes`, `max_retries`, `convergence_timeout_secs`, `post_process_enabled`, `post_process_streaming_enabled`, `post_process_api_url`, `post_process_api_key`, `post_process_model`, `post_process_prompt`, `post_process_temperature`, `local_mode`, `local_data_dir`, `local_server_port`, `local_quantization`. Returns `"*** (set)"` for API key fields if present; `None` for unknown keys.

**`set_field(&mut self, key: &str, value: &str) -> Result<(), String>`**

Sets a field by name with type conversion and validation. Non-finite floats are rejected, `max_retries` is capped at 16, and `convergence_timeout_secs` is capped at 3600. Secret fields cannot be set through this command; use the documented environment variables or edit `config.json` directly. Local runtime keys can also be mutated from CLI.

**`apply_json(&mut self, json: &Value)`** *(private)*

Applies partial JSON overrides. Backward compatibility:
- Old `"hotkey"` key maps to `hold_hotkey`
- Old `"groq_api_key"` key maps to `api_key` (if `api_key` not already set)
- Local runtime keys (`local_mode`, `local_data_dir`, `local_server_port`, `local_quantization`) are also deserialized from `config.json`

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
- **Session-owned Results**: Each active session exclusively owns its `Vec<ChunkEntry>`. The worker never reads or mutates chunk state; it reports `UploadStarted` and `Completed` events through a session-specific result channel.
- **Convergence Timeout**: Configurable via `convergence_timeout_secs`; chunks still pending at the deadline are marked `Failed(Timeout)`
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error
- **Bounded Queue**: Chunk submission is non-blocking. A full queue marks the chunk failed and deletes its temporary file rather than stalling session shutdown.
- **Cleanup Guarantee**: Processed, rejected, orphaned, cancelled, result-disconnected, and panicking-transcriber chunk paths are cleaned up by the orchestrator. A disconnected result receiver does not stop the worker from draining paths it already owns.
- **Strict Session Routing**: start, chunk, finish, and abort operations carry `SessionId`; duplicate starts and mismatched IDs are rejected without replacing active work.

`on_chunk_ready` opportunistically drains completed worker events while recording. During shutdown, `finish_session` closes the bounded input sender and waits on the result receiver with the configured convergence deadline. Timeout or abort drops the session-owned chunk vector immediately; a detached worker can finish synchronous transcription and file cleanup, but late events cannot retain or mutate the ended session or reach a newer session.

### `SessionError` Enum

| Variant | Description |
|---|---|
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, failures }` | Some chunks succeeded, includes partial text |
| `ConvergenceTimeout { partial_text, pending }` | Timeout hit, includes what was completed |

---

## Recording Session (`src/core/recording_session.rs`)

`RecordingSessionMachine` is the sole authority for recording lifecycle transitions. Tray and hotkey adapters emit source-tagged `ControlEvent`s and never inspect recorder state directly.

### States

- `Idle`
- `Starting`: recorder/orchestrator startup is in progress
- `Recording`: one session is accepting audio chunks
- `Stopping`: recorder stop or orchestrator convergence is in progress
- `Recovering`: an inconsistent lower-layer state is being cancelled
- `ShuttingDown`: new controls are ignored while cleanup effects run

Every accepted start receives a monotonically increasing `SessionId`. The ID is propagated through recorder operations, ready chunks, orchestrator routing, effects, and completion events. Stale chunks are deleted and stale completions cannot mutate the current session.

### Event/Effect Boundary

The machine consumes external controls plus structured recorder/orchestrator outcomes and emits declarative effects. `main.rs` executes those effects and feeds results back as internal events. Tray state changes only after successful lifecycle transitions.

Exit is represented as `ShutdownRequested`. It cancels recorder/orchestrator work, resets tray state, suppresses final text injection, and exits only after `ReadyToExit` is emitted.

## Main Integration Notes

Although the main event loop lives in `src/main.rs`, `core` owns the configuration and CLI abstractions that feed it:

- `run_listener_with_config(config)` is the common entry point for both default mode and `local start`
- when `config.local_mode` is true, startup first calls into the `local` module to ensure the runtime exists, always rewrites the transcription endpoint, and conditionally rewrites the post-process endpoint when `post_process_enabled` is on
- the same orchestrator pipeline is reused regardless of whether the backend is Groq/OpenAI-compatible cloud STT or the local Gemma service
