# Core Module Architecture

## Purpose

The `core` module owns process entry points, CLI parsing and command execution, strict v3 configuration persistence and typed field selection, recording lifecycle state, and transcription orchestration. Configuration callers declare the fields they need and supply their own constructors; `core::config` has no dependency on workflows or UI modules.

## Config (`src/core/config/`)

The config package intentionally has four files:

| File | Responsibility |
|---|---|
| `document.rs` | `ConfigDocument`, nested v3 serde schema, defaults, and bound secret source |
| `fields.rs` | one canonical catalog for typed field requests and CLI list/get/set |
| `store.rs` | platform path discovery, fail-closed load, explicit configuration preparation and atomic save |
| `src/core/config.rs` | facade, config errors, and secret-safe value types |

`ConfigDocument` accepts the canonical document with `schema_version: 3`. Missing or unknown fields,
wrong versions, invalid JSON, and non-finite floats are errors, except that optional
`audio.input_device` defaults to the system device and optional non-prompt `dictation` fields use
their typed defaults. Missing prompt fields remain missing on deserialization; they do not silently
acquire bundled text. Component construction rejects missing prompts required by the selected service.
Retired fields such as `chunking`, `session`, and `inference.api.provider` are not accepted. A
missing file is represented as `None` by `ConfigStore::load`; desktop setup uses the absence to
trigger the first-run flow, while ordinary conversion requires a saved configuration.

`ConfigStore::discover()` gets the application directory from its own `config_dir()` helper and appends
`config.json`. Reads and writes therefore use the same canonical path independent of the launch
directory. `ConfigStore::prepare` runs before desktop or ordinary conversion resolves recognition
settings. It strictly reads the existing document, fills absent optional Realtime fields and applies
the current bundled prompts, compares the actual JSON, atomically saves changes, then reloads the
saved document. It returns `None` for a missing file; parse or write failures cannot supply an
unsaved replacement to recognition. Identical content is not rewritten.

`ConfigDocument::apply_bundled_prompts` is confined to preparation and first-setup candidate
construction. It materializes the full HTTP `transcription.prompt`, full LLM system text in
`post_process.prompt`, and full normal/short Realtime texts in `dictation.prompt` and the new optional
`dictation.short_prompt`. `dictation.prompt_components` remains strictly parseable for old-file
compatibility, is set to null during preparation, and never selects runtime instructions. Existing
custom strings, empty strings, nulls and component overrides are replaced on disk. All other explicit
settings, including connections, disk secrets, audio, hotkeys and long-term vocabulary, are preserved.
Prompt wording and the short-segment threshold do not change. Runtime consumers use saved text
without bundled fallbacks or LLM wrapping.

`load` and `config path/get/list/check` remain read-only and expose saved values. Inspection does not
apply bundled prompts; `check` reports missing required prompt configuration until preparation is
completed. Ordinary field edits and tray device/gain saves update only their target fields and do
not trigger prompt preparation. The new optional short field is a schema-v3 extension that older
binaries may reject; older LLM code would also wrap the full system text again. Downgrades require
the corresponding old configuration as well as the old binary.
Writes use a temporary file in the destination directory followed by atomic
publication. `ConfigStore::load` is the general read and parse entry point: `None` distinguishes a missing
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
and merge language; WAV chunk limits remain audio-owned constants. Ordinary prompt-lab evaluation
builds desktop recognition settings from an isolated saved configuration; audio candidates are
written and reloaded before these settings or services are constructed.

Constructors propagate ordinary errors with `?`; there is no separate validation pass or
configuration-wide issue collection. The CLI's `config check` command builds a `ListenerConfig`
without starting services and reports the first construction error. It does not prove that an
endpoint, model, or credential will be accepted by an API. Loading and editing documents remain
independent of component construction and support incremental configuration.

## CLI (`src/core/cli.rs`)

The no-subcommand listener and the no-console desktop entry point pass through
`ui::setup` before constructing runtime services. Existing parseable configuration is prepared,
saved if changed and reloaded before listener resolution. A valid listener document bypasses the
wizard; missing or invalid configuration opens the same modal flow on macOS and Windows. Declining
setup exits without an unsaved default listener or an unconfirmed replacement of a damaged file.
First setup applies bundled prompts to its candidate before recording verification; final confirmation
saves and reloads it before normal listening. Cancelling the wizard adds no save and does not roll
back updates already committed by preparation. Preparation write failures abort startup.
The explicit `setup` command reruns the wizard without starting the listener after saving. Interactive hotkey
editing launches a short-lived helper instance of the same executable so the existing `rdev`
listener can capture one physical key and terminate without leaving a second global listener in the
main process. Ordinary `convert` also prepares and reloads configuration before constructing
recognition services, and requires setup when no configuration exists.

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

`config set` parses the canonical field type and saves the updated document without running cross-field business validation. This permits incremental configuration; `config check` or the command that consumes the API configuration reports incomplete runtime configuration. Secret and schema fields, plus `transcription.prompt` and `post_process.prompt`, are read-only. Legacy aliases are rejected.

---

## Orchestrator (`src/core/orchestrator.rs`)

### Purpose

`SessionOrchestrator` owns active-session identity, routing and start/finish/abort for both HTTP and
Realtime. Its backend creates either an HTTP chunk worker or recording-scoped Realtime I/O; both
return the existing session result/error contract. Finishing detaches the session and closes input
before collecting results; abort and orchestrator drop cancel active work. The HTTP chunk tracking
and convergence rules below apply equally to fixed raw capture and already-segmented desktop input.
Realtime connection/audio details live in
[dictation](dictation.md).

Desktop `finish_for_delivery` retains an optional single-use Realtime confirmation handle with
the text. The preview worker returns final edited text through that handle only after successful
delivery. The public string-only `finish_session` discards the handle; it cannot silently confirm
learning. HTTP results and partial failures carry no learning capability.

### HTTP Worker Concepts

- **Chunk State Machine**: `Flushed → Uploading → Transcribed / Failed`
- **Session-owned Results**: Each session exclusively owns `chunk_entries: Vec<ChunkEntry>`, tracking records rather than WAV data. Every complete chunk is registered at intake, including recorder stop-time tails. The worker reports `UploadStarted` and `Completed` through a session-specific channel; events update existing entries only.
- **Convergence Timeout**: Recorder stop and tail submission precede finalization. Closing the HTTP input starts the module-owned 30-second convergence budget for queued chunks and requests. An all-terminal registry proves no chunk results remain outstanding; the worker has no unregistered audio or tail. Timeout cancels remaining work and reports the number of pending chunks.
- **Partial Failure**: If some chunks succeed and others fail, returns partial text with an error
- **Bounded Queue**: Both HTTP workflows use the original capacity-two complete-chunk queue and one serial STT worker. Nonblocking submission marks a full/disconnected queue's rejected chunk as Failed; later chunks may still be accepted. Finalization reports these failures with partial text. It does not wait for queue space, expand capacity or introduce another queue.
- **Input Integrity**: Recorder overflow wakes the UI through the existing audio-readiness event. Desktop delivery records the first recorder error in the session-owned `input_failure: Option<String>`, under the existing session mutex. Stop warnings also reach this field when stop precedes readiness handling. Finalization returns `InputFailure` with any completed text, including when no chunk was produced; raw prompt-lab recorder warnings retain their original log-only behavior. HTTP has no short-frame decoder, shared fault notifier or separate EOF protocol.
- **Memory Ownership**: Queued chunks are immutable shared WAV bytes. Rejected, stale, cancelled, or completed chunks are released by normal ownership drops; the orchestrator performs no chunk-file cleanup.
- **Strict Session Routing**: start, chunk, finish, and abort operations carry `SessionId`; duplicate starts and mismatched IDs are rejected without replacing active work.

`on_chunk_ready` is a notification returning `()`: invalid session IDs are logged and rejected
internally, and callers do not repeat that logging. Input first attempts non-blocking submission,
then registers either a pending or failed entry before consuming available worker events.
Submission failures remain in the session for `finish_session` to report alongside partial text.
`apply_worker_event` owns the shared upload/completion transitions, including terminal-state
protection. During shutdown,
`finish_session` closes the bounded input sender and waits on the result receiver with the fixed
convergence deadline. Timeout or abort drops session-owned chunk state immediately; a detached
worker can finish synchronous transcription, but starts no more requests after cancellation and
late events cannot mutate the ended session or reach a newer session. Existing synchronous
request/retry calls cannot be withdrawn. Desktop HTTP shares the preview's `Progress` cancellation
token even after stop detaches finalization, without publishing provisional HTTP text.

`SessionStartError` is a struct containing the requested and active IDs. Result collection and
shared text merging accept language configuration as `Option<String>`; callers clone configuration
that must remain available for later sessions.

### `SessionError` Enum

| Variant | Description |
|---|---|
| `Routing(SessionRoutingError)` | No active session or a mismatched session ID |
| `NoChunks` | Recording too short to produce any audio |
| `PartialFailure { partial_text, errors }` | One or more chunks failed; includes any successful text |
| `InputFailure { partial_text, error }` | Recorder input failed; completed text remains available with an incomplete-recording warning and no learning capability |
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
- `Stopping`: recorder stop, tail-chunk submission, convergence and selected output finalization are in progress; desktop delivery includes awaiting confirmation
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
output. Normal delivery performs post-processing, shows a confirmation preview, and waits for
user confirmation before text history persistence and injection. Prompt-lab capture instead finalizes its archived WAV and stores the raw merged STT
result plus credential-free request metadata in the matching sample sidecar; it constructs no
post-processor or preview. The confirmation worker emits one `HistorySaved` event after a successful
append and reuses that save when the same edited text is retried; another confirmed edit saves
its own revision. The main-thread tray prepends that text to its five-entry cache. The machine
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

CLI workflows in `core` and desktop setup in `ui` obtain a `ConfigDocument` through the read or
preparation boundary described above; configuration constructors request their typed fields
through `ConfigDocument::select`, and the workflow passes each narrow value to its runtime consumer. Listener mode then creates one main-thread winit
`EventLoop<AppEvent>` in `ControlFlow::Wait` mode. Opaque platform input, audio-readiness, and
background completion producers use `EventLoopProxy` to wake that loop; winit owns AppKit/Win32
dispatch, including the lazily created native confirmation window. CLI-only workflows do not construct the event loop.

`src/ui/listener/event_loop.rs` passes opaque native payloads back through
`NativePlatform::handle_event`, normalizes returned semantic actions, executes state-machine
effects, drains ready chunks, coordinates background finalization, and handles history-copy actions
without routing them through the recording state machine. Winit types remain in `ui`.
The existing business interfaces expose only domain values or narrow callbacks; platform-specific
values remain behind the platform facade. Candidate destinations are retained on the finalization
worker, while native preview windows stay on the main thread.
The shared confirmation protocol and cancellation gate belong to `ui::preview`; the listener's
event loop only adapts those updates to its application events. The integration demo uses the
same worker without depending on listener implementation types.
Finalization uses an atomic cancellation flag checked before post-processing, history persistence,
and final text injection. History persistence appends one timestamped record to `history.jsonl`;
before loading or appending, the store validates only the trailing record and truncates that record
if its JSON or typed metadata is invalid. Normal writes append one line; crossing 5 MiB keeps the
newest complete suffix through a temporary-file replacement. An invalid older record stops menu
loading at that boundary without being repaired. Exit does not wait for persistence or injection
already in progress.

The configurable API backend is shared by the recording/orchestration, offline conversion, setup,
and prompt-lab workflows. Their runtime assembly consumes configured prompts without rewriting
endpoints or substituting bundled instructions. Both process entry files only delegate startup to
explicit library entry points.

See [UI architecture](ui.md) for the desktop interaction boundary. The existing listener and setup
call chains live there intact and directly use core, audio, and platform services.

Prompt-lab evaluation is a CLI-only workflow: it does not construct winit, normal history, or native
typing. It saves an isolated configuration in its existing temporary directory, prepares that copy,
then saves any audio-prompt candidate and reloads it before constructing the production recognition
owners, including configured HTTP cleanup. `ConfigStore::at` is crate-internal so the copy uses the
same storage logic. Realtime literal/no-prompt candidates clear the short branch; HTTP no-prompt
continues to omit the prompt parameter. Baseline and candidate runs leave the main configuration and
source memory untouched; only the private memory copy learns. Verified WAVs use the production
recording path, and one JSON report records the actual prompts. Coding-agent review is a separate
local JSON validation/rewrite step and makes no inference request.
