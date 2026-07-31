# Core Module Architecture

## Purpose

The `core` module contains strict v2 configuration persistence, CLI parsing, recording lifecycle state, and transcription orchestration. Application-level configuration assembly lives in `src/runtime_config.rs` so business consumers never receive the full persisted document.

## Config (`src/core/config/`)

The config package intentionally has four files:

| File | Responsibility |
|---|---|
| `document.rs` | `ConfigDocument`, nested v2 serde schema, and defaults |
| `fields.rs` | one canonical `ConfigKey`/`FieldSpec` catalog used by list/get/set |
| `store.rs` | platform path discovery plus fail-closed load and atomic save |
| `mod.rs` | facade, config errors, validation report, secret-safe value types |

`ConfigDocument` accepts only a complete document with `schema_version: 2`. Missing or unknown fields, wrong versions, invalid JSON, and non-finite floats are errors. A missing file alone returns the in-memory defaults.

`ConfigStore::discover()` gets the application directory from `platform::config_dir()` and appends `config.json`. Reads and writes therefore use the same canonical path independent of the launch working directory. Writes use a temporary file in the destination directory followed by atomic publication.

`EnvironmentSecretSource` reads only `TRANSCRIPTION_API_KEY` and `POST_PROCESS_API_KEY` through the runtime assembly layer. Environment values override disk secrets but are never copied into `ConfigDocument`; CLI output reports only `unset`, `disk`, `environment`, or `environment overrides disk`.

## Runtime assembly (`src/runtime_config.rs`)

`runtime_config` selects the API or Local profile, constructs module-owned consumer configs, and aggregates construction errors into `ListenerConfig` or `BackendConfig`. Profile selection is consumed during assembly: `BackendConfig` stores the common transcriber and post-process values directly, plus an optional Local service, rather than duplicating common fields across enum variants. It contains no generic validator registry and no duplicated raw DTO layer.

Each consumer receives a type owned by its module: `HotkeyConfig`, `AudioConfig`, `OrchestratorConfig`, `TranscriberConfig`, `PostProcessConfig`, `LocalPaths`, or `LocalServiceConfig`. Local mode uses `ApiAuth::None`; API mode uses a redacted `SecretValue` when configured and `ApiAuth::None` otherwise. `local start` selects Local for one invocation without mutating the persisted profile.

## CLI (`src/core/cli.rs`)

No subcommand runs the recording listener. Other commands are:

| Command | Description |
|---|---|
| `config path` | Print the canonical file path |
| `config check` | Resolve the active listener profile and report construction issues |
| `config list/get/set` | Use canonical dotted keys from the single field catalog |
| `local install/start/stop/status` | Manage the Local runtime; stop only requires Local paths |
| `convert <wav>` | Resolve the persisted backend and transcribe a WAV file |

`config set` parses the canonical field type and saves the updated document without running cross-field business validation. This permits incremental configuration; `config check` or the command that consumes a profile reports incomplete runtime configuration. Secret and schema fields are read-only. Legacy aliases are rejected.

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

`main.rs` loads one `ConfigDocument`, asks `runtime_config` for a typed workflow configuration, and passes each narrow value to its consumer. API and Local backends reuse the same recording/orchestration pipeline; no endpoint rewriting or persisted-document mutation occurs in `main`.
