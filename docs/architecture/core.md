# Core Module Architecture

## Purpose

The `core` module contains strict v2 configuration persistence, CLI parsing, recording lifecycle state, and transcription orchestration. Application-level configuration assembly lives in `src/runtime_config.rs` so business consumers never receive the full persisted document.

## Config (`src/core/config/`)

The config package intentionally has four files:

| File | Responsibility |
|---|---|
| `document.rs` | `ConfigDocument`, nested v2 serde schema, and defaults |
| `fields.rs` | one canonical `ConfigKey` catalog used by list/get/set |
| `store.rs` | platform path discovery plus fail-closed load and atomic save |
| `src/core/config.rs` | facade, config errors, validation report, secret-safe value types |

`ConfigDocument` accepts only the current complete canonical document with `schema_version: 2`.
Missing or unknown fields, wrong versions, invalid JSON, and non-finite floats are errors. Retired
fields such as `chunking`, `session`, and `inference.api.provider` are not accepted. A missing file
alone returns the in-memory defaults.

`ConfigStore::discover()` gets the application directory from `platform::config_dir()` and appends `config.json`. Reads and writes therefore use the same canonical path independent of the launch working directory. Writes use a temporary file in the destination directory followed by atomic publication.

`EnvironmentSecretSource` reads only `TRANSCRIPTION_API_KEY` and `POST_PROCESS_API_KEY` through the runtime assembly layer. Environment values override disk secrets but are never copied into `ConfigDocument`; CLI output reports only `unset`, `disk`, `environment`, or `environment overrides disk`.

## Runtime assembly (`src/runtime_config.rs`)

`runtime_config` selects the API or Local profile, constructs module-owned consumer configs, and aggregates construction errors into `ListenerConfig` or `BackendConfig`. Profile selection is consumed during assembly: `BackendConfig` stores the common transcriber and post-process values directly, plus an optional Local service, rather than duplicating common fields across enum variants. It contains no generic validator registry and no duplicated raw DTO layer.

Each consumer receives a type owned by its module: `HotkeyConfig`, `AudioConfig`,
`OrchestratorConfig`, `TranscriberConfig`, `PostProcessConfig`, `LocalPaths`, or
`LocalServiceConfig`. Hotkey resolution enters through `platform::validate_hotkeys`, so the
compile-time-selected backend supplies native key availability without adding target branches to
runtime assembly. Local mode uses `ApiAuth::None`; API mode uses a redacted `SecretValue` when
configured and `ApiAuth::None` otherwise. `local start` selects Local for one invocation without
mutating the persisted profile.

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
- **Convergence Timeout**: A module-owned 30-second deadline marks chunks still pending as `Failed(Timeout)`
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error
- **Bounded Queue**: The capacity-two in-memory `WavChunk` queue is non-blocking. A full queue marks the chunk failed rather than stalling session shutdown.
- **Memory Ownership**: Queued chunks are immutable shared WAV bytes. Rejected, stale, cancelled, or completed chunks are released by normal ownership drops; the orchestrator performs no chunk-file cleanup.
- **Strict Session Routing**: start, chunk, finish, and abort operations carry `SessionId`; duplicate starts and mismatched IDs are rejected without replacing active work.

`on_chunk_ready` opportunistically drains completed worker events while recording. During shutdown,
`finish_session` closes the bounded input sender and waits on the result receiver with the fixed
convergence deadline. Timeout or abort drops session-owned chunk state immediately; a detached
worker can finish synchronous transcription, but late events cannot retain or mutate the ended
session or reach a newer session.

### `SessionError` Enum

| Variant | Description |
|---|---|
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, failures }` | Some chunks succeeded, includes partial text |
| `ConvergenceTimeout { partial_text, pending }` | Timeout hit, includes what was completed |

---

## Recording Session (`src/core/recording_session.rs`)

`RecordingSessionMachine` is the sole authority for recording lifecycle transitions. The selected
platform runtime maps native hotkey and tray events into Hold/Toggle/Exit `PlatformAction` values;
the listener integration then maps those actions into source-free `StartRequested`,
`StopRequested`, and `ShutdownRequested` events before they enter core.

### States

- `Idle`
- `Starting`: the composite recorder/orchestrator startup is in progress
- `Recording`: one session is accepting audio chunks
- `Stopping`: the composite recorder stop, tail-chunk submission, and orchestrator convergence is in progress
- `ShuttingDown`: new controls are ignored while cleanup effects run

The active states carry only a monotonically increasing `SessionId`; input source, interaction mode, and lower-layer phases are not lifecycle state. The shared identity type is defined in `src/session.rs`, while this state machine remains responsible for allocating IDs. The ID is propagated through recorder operations, ready chunks, orchestrator routing, effects, and completion events. Stale chunks and stale Session results cannot mutate the current session.

### Explicit Transition Table

`RecordingSessionMachine::handle` is the only state-writing entry point. It first compares a result event's routing `SessionId` with the active state once, then delegates matching events to one private `(RecordingState, SessionEvent)` match. The allowed paths are `Idle -> Starting -> Recording -> Stopping -> Idle`, plus chunk submission, failure recovery, and shutdown. Events rejected by either layer leave the state unchanged, emit no effects, and produce one compact debug record containing only the current state, event name, and optional routing ID.

### Event/Effect Boundary

The machine consumes source-free requests plus `SessionStarted`, `SessionStartFailed`, `SessionStopped`, and `SessionStopFailed` results. It emits composite `StartSession` and `StopSession` effects instead of exposing recorder/orchestrator startup phases.

`application::listener` executes `StartSession` as an all-or-nothing recorder/orchestrator
acquisition with rollback. `StopSession` stops the recorder and submits tail chunks in order, then
starts a session-scoped background task for orchestrator convergence, post-processing, and text
history persistence followed by injection. `HistoryTyper` emits one `HistorySaved` event after a
successful append, and the main-thread tray prepends that text to its five-entry cache. The machine
remains in `Stopping` until the separate finalization event returns.

Exit is represented as `ShutdownRequested`. It cancels recorder/orchestrator work, resets tray state, suppresses final text injection, and exits only after `ReadyToExit` is emitted.

## Main Integration Notes

`application` loads one `ConfigDocument`, asks `runtime_config` for a typed workflow configuration,
and passes each narrow value to its consumer. Listener mode then creates one main-thread winit
`EventLoop<AppEvent>` in `ControlFlow::Wait` mode. Opaque platform input, audio-readiness, and
background completion producers use `EventLoopProxy` to wake that loop; winit owns AppKit/Win32
dispatch and no window is created. CLI-only workflows do not construct the event loop.

`src/application/listener/event_loop.rs` passes opaque native payloads back through
`NativePlatform::handle_event`, normalizes returned semantic actions, executes state-machine
effects, drains ready chunks, coordinates background finalization, and handles history-copy actions
without routing them through the recording state machine. Winit types remain in the application
layer. `core`, `audio`, and `input` expose only domain values or narrow callbacks; platform-specific
values remain behind `NativePlatform`.
Finalization uses an atomic cancellation flag checked before post-processing, history persistence,
and final text injection. History persistence appends one timestamped record to `history.jsonl`;
before loading or appending, the store validates only the trailing record and truncates that record
if its JSON or typed metadata is invalid. Normal writes append one line; crossing 5 MiB keeps the
newest complete suffix through a temporary-file replacement. An invalid older record stops menu
loading at that boundary without being repaired. Exit does not wait for persistence or injection
already in progress.

API and Local backends reuse the same recording/orchestration pipeline; no endpoint rewriting or
persisted-document mutation occurs in the application layer. `main.rs` only delegates process
startup to the library entry point.
