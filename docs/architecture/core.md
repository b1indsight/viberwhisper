# Core Module Architecture

## Purpose

The `core` module owns process entry points, CLI parsing and command execution, strict v3 configuration persistence and typed field selection, recording lifecycle state, and transcription orchestration. Configuration callers declare the fields they need and supply their own constructors; `core::config` has no dependency on workflows or UI modules.

## Config (`src/core/config/`)

The config package intentionally has four files:

| File | Responsibility |
|---|---|
| `document.rs` | `ConfigDocument`, nested v3 serde schema, defaults, and bound secret source |
| `fields.rs` | one canonical catalog for typed field requests and CLI list/get/set |
| `store.rs` | platform path discovery plus fail-closed load and atomic save |
| `src/core/config.rs` | facade, config errors, and secret-safe value types |

`ConfigDocument` accepts the canonical document with `schema_version: 3`. Missing or unknown fields,
wrong versions, invalid JSON, and non-finite floats are errors, except that optional
`audio.input_device` defaults to the system device when absent.
Retired fields such as `chunking`, `session`, and `inference.api.provider` are not accepted. A
missing file is represented as `None` by `ConfigStore::load`; ordinary callers explicitly select
the in-memory defaults while setup uses the absence to trigger the first-run flow.

`ConfigStore::discover()` gets the application directory from its own `config_dir()` helper and appends
`config.json`. Reads and writes therefore use the same canonical path independent of the launch
working directory. Writes use a temporary file in the destination directory followed by atomic
publication. `ConfigStore::load` is the single read and parse path: `None` distinguishes a missing
file, `Some` carries a loaded document, and malformed or unreadable files remain errors.

`ConfigDocument::new(source)` creates default settings with an owned secret source.
Default construction and loading bind `EnvironmentSecretSource`.
Clones share that source, which is omitted from serialization, debug output, and document
equality. Reads stay lazy: only requested API-key fields consult `TRANSCRIPTION_API_KEY` or
`POST_PROCESS_API_KEY`. Environment values override disk secrets without entering the persisted
fields; CLI output reports only `unset`, `disk`, `environment`, or `environment overrides disk`.
The setup wizard uses the same bound source to report credential overrides.

## Caller-declared field selection

`ConfigDocument::select(fields, build)` reads a typed field selector or a tuple of
selectors, then passes exactly those values to the caller's constructor. Its generic return
value can be a caller-owned configuration or a construction result; the configuration package
imports neither type. One field catalog generates the selectors, dotted names, writability,
and redacted CLI reads, so runtime requests and CLI field access share their mappings.

For example, a caller can request `(fields::AudioInputDevice, fields::AudioMicGain)` and use
`|(input_device, mic_gain)| ...` to build its own recording settings. Primitive field types
remain native Rust types. API-key selections resolve environment-over-disk precedence and
carry redacted authentication plus source status; CLI reads expose only the source status.

`AudioConfig`, `TranscriberConfig`, and `PostProcessConfig` declare their requirements in their
own modules. Post-processing requests its enabled flag first and reads LLM settings only when
enabled. Constructors parse URL values and require the fields needed for enabled cleanup.
HTTP protocol and empty-model errors are left to the request layer. Hotkey construction resolves
named keys and rejects unsupported or conflicting bindings.

Workflow settings are assembled in `core`. `core::listener::RecordingConfig` combines hotkeys,
audio, orchestrator, and STT settings for raw dataset capture. `ListenerConfig` adds cleanup
for normal delivery and setup verification. Offline `ConvertConfig` combines STT, cleanup,
and merge language; WAV chunk limits remain audio-owned constants. Prompt-lab evaluation
requests only `TranscriberConfig`, so invalid hotkey or cleanup settings do not block STT.

Constructors propagate ordinary errors with `?`; there is no separate validation pass or
configuration-wide issue collection. The CLI's `config check` command builds a `ListenerConfig`
without starting services and reports the first construction error. It does not prove that an
endpoint, model, or credential will be accepted by an API. Loading and editing documents remain
independent of component construction and support incremental configuration.

## CLI (`src/core/cli.rs`)

The no-subcommand listener and the no-console desktop entry point pass through
`ui::setup` before constructing runtime services. A valid listener document bypasses the
wizard; missing or invalid configuration opens the same modal flow on macOS and Windows. The
explicit `setup` command reruns it without starting the listener after saving. Interactive hotkey
editing launches a short-lived helper instance of the same executable so the existing `rdev`
listener can capture one physical key and terminate without leaving a second global listener in the
main process. Other CLI workflows keep their existing configuration behavior.

No subcommand runs the recording listener. Other commands are:

| Command | Description |
|---|---|
| `setup` | Run the modal configuration and verification flow on demand |
| `config path` | Print the canonical file path |
| `config check` | Resolve the listener API configuration and report construction issues |
| `config list/get/set` | Use canonical dotted keys from the single field catalog |
| `convert <wav>` | Resolve the configured API backend and transcribe a WAV file |
| `prompt-lab record` | Reuse native recording controls to archive WAV/raw-STT sample pairs |
| `prompt-lab sample list/show/correct` | Inspect and curate human references and proper nouns |
| `prompt-lab dataset validate` | Verify dataset schemas, WAV readability, paths, and digests |
| `prompt-lab evaluate` | Freshly transcribe every ready WAV and write local metrics to JSON |
| `prompt-lab report apply-review` | Validate coding-agent scores and finalize all three gates |

`config set` parses the canonical field type and saves the updated document without running cross-field business validation. This permits incremental configuration; `config check` or the command that consumes the API configuration reports incomplete runtime configuration. Secret and schema fields are read-only. Legacy aliases are rejected.

---

## Orchestrator (`src/core/orchestrator.rs`)

### Purpose

`SessionOrchestrator` unifies the lifecycle of Hold and Toggle recording sessions, managing background transcription of audio chunks with convergence timeout and error handling.

### Key Concepts

- **Chunk State Machine**: `Flushed → Uploading → Transcribed / Failed`
- **Session-owned Results**: Each active session exclusively owns its `chunk_entries: Vec<ChunkEntry>`. These are tracking records, not WAV data. The worker never reads or mutates chunk state; it reports `UploadStarted` and `Completed` events through a session-specific result channel.
- **Convergence Timeout**: A module-owned 30-second deadline marks chunks still pending as `Failed(Timeout)`
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error
- **Bounded Queue**: The capacity-two in-memory `WavChunk` queue is non-blocking. A full queue marks the chunk failed rather than stalling session shutdown.
- **Memory Ownership**: Queued chunks are immutable shared WAV bytes. Rejected, stale, cancelled, or completed chunks are released by normal ownership drops; the orchestrator performs no chunk-file cleanup.
- **Strict Session Routing**: start, chunk, finish, and abort operations carry `SessionId`; duplicate starts and mismatched IDs are rejected without replacing active work.

`on_chunk_ready` is a notification returning `()`: invalid session IDs are logged and rejected
internally, and callers do not repeat that logging. It first attempts non-blocking submission,
then registers either a pending or failed entry before consuming available worker events.
Submission failures remain in the session for `finish_session` to report alongside partial text.
`apply_worker_event` owns the shared upload/completion transitions, including terminal-state
protection. During shutdown,
`finish_session` closes the bounded input sender and waits on the result receiver with the fixed
convergence deadline. Timeout or abort drops session-owned chunk state immediately; a detached
worker can finish synchronous transcription, but late events cannot retain or mutate the ended
session or reach a newer session.

`SessionStartError` is a struct containing the requested and active IDs. Result collection and
shared text merging accept language configuration as `Option<String>`; callers clone configuration
that must remain available for later sessions.

### `SessionError` Enum

| Variant | Description |
|---|---|
| `Routing(SessionRoutingError)` | No active session or a mismatched session ID |
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, errors }` | One or more chunks failed; includes any successful text |
| `ConvergenceTimeout { partial_text, pending_count }` | Timeout hit, includes what was completed |

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

The transition function receives a session ID value and never modifies the allocation counter.
The machine advances that counter only when accepting an `Idle -> Starting` transition, so rejected
events do not consume IDs and a failed startup cannot reuse its ID.

### Event/Effect Boundary

The machine consumes source-free requests plus `SessionStarted`, `SessionStartFailed`, `SessionStopped`, and `SessionStopFailed` results. It emits composite `StartSession` and `StopSession` effects instead of exposing recorder/orchestrator startup phases.

`ui::listener` executes `StartSession` as an all-or-nothing recorder/orchestrator
acquisition with rollback. `StopSession` stops the recorder and submits tail chunks in order, then
starts a session-scoped background task for orchestrator convergence and the selected application
output. Normal delivery performs post-processing and text history persistence followed by
injection. Prompt-lab capture instead finalizes its archived WAV and stores the raw merged STT
result plus credential-free request metadata in the matching sample sidecar; it constructs no
post-processor or history typer. `HistoryTyper` emits one `HistorySaved` event after a successful
normal append, and the main-thread tray prepends that text to its five-entry cache. The machine
remains in `Stopping` until the separate finalization event returns.

Exit is represented as `ShutdownRequested`. It cancels recorder/orchestrator work, resets tray state, suppresses final text injection, and exits only after `ReadyToExit` is emitted.

## Main Integration Notes

`src/main.rs` remains the console process entry and delegates to `core::run`, which parses
CLI commands and preserves stdout, stderr, waiting, and exit behavior. Windows release packages
also build the feature-gated `src/bin/viberwhisper-app.rs` as a GUI-subsystem executable. That
entry delegates to `core::run_desktop`, bypasses CLI parsing, and enters the same configured
listener directly. Before tracing starts, the Windows GUI entry redirects stdout and stderr to
`NUL`, providing valid handles without allocating a console. Fatal desktop startup errors cross
the Windows platform boundary into a native error dialog because ordinary diagnostics are not
visible. macOS packaging continues to expose only the established `.app` entry.

CLI workflows in `core` and desktop setup in `ui` load a `ConfigDocument`; configuration constructors request their typed fields
through `ConfigDocument::select`, and the workflow passes each narrow value to its runtime consumer. Listener mode then creates one main-thread winit
`EventLoop<AppEvent>` in `ControlFlow::Wait` mode. Opaque platform input, audio-readiness, and
background completion producers use `EventLoopProxy` to wake that loop; winit owns AppKit/Win32
dispatch and no window is created. CLI-only workflows do not construct the event loop.

`src/ui/listener/event_loop.rs` passes opaque native payloads back through
`NativePlatform::handle_event`, normalizes returned semantic actions, executes state-machine
effects, drains ready chunks, coordinates background finalization, and handles history-copy actions
without routing them through the recording state machine. Winit types remain in `ui`.
The existing business interfaces expose only domain values or narrow callbacks; platform-specific
values remain behind `NativePlatform`.
Finalization uses an atomic cancellation flag checked before post-processing, history persistence,
and final text injection. History persistence appends one timestamped record to `history.jsonl`;
before loading or appending, the store validates only the trailing record and truncates that record
if its JSON or typed metadata is invalid. Normal writes append one line; crossing 5 MiB keeps the
newest complete suffix through a temporary-file replacement. An invalid older record stops menu
loading at that boundary without being repaired. Exit does not wait for persistence or injection
already in progress.

The configurable API backend is shared by the recording/orchestration, offline conversion, setup,
and prompt-lab workflows; their assembly does not rewrite endpoints or persist temporary request
overrides. Both process entry files only delegate startup to explicit library entry points.

See [UI architecture](ui.md) for the desktop interaction boundary. The existing listener and setup
call chains live there intact and directly use core, audio, and platform services.

Prompt-lab evaluation is a CLI-only workflow: it does not construct winit, a post-processor,
history, or native typing. It resolves the configured backend, replaces only the consumed
`TranscriberConfig` prompt in memory, sequentially feeds verified historical WAVs through the
production offline chunk reader and transcriber, and writes one JSON report. Coding-agent review is
a separate local JSON validation/rewrite step and makes no inference request.
