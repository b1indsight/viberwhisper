# 28 - Winit Event Loop

## Status

**Implemented; local automated validation complete.** The main-thread winit loop,
callback forwarding, per-boundary audio readiness, cancellable background finalization, tests, and
current-truth documentation are implemented on PR #98. Local tests, formatting, Clippy, and Windows
cross-compilation pass after the simplification. Hosted checks passed before the simplification and
must rerun on the updated PR. Final interactive tray, recording, and text injection gesture checks
also remain before PR readiness.

### Post-review simplifications

`tray-icon 0.21` stores its process-global native event handlers in one-shot cells, so they cannot
be unregistered and replaced as originally described. The initial implementation added a
mutex-protected replaceable forwarding slot, but review confirmed that the application creates one
listener per process. `TrayManager::new` now installs the application callback directly for the
process lifetime.

The initial audio implementation also coalesced boundary notifications behind a pending flag and
added a re-arm/timer-retry protocol to close the resulting lost-wakeup race. Review removed that
self-induced complexity: the recorder now sends one lightweight wakeup whenever it observes a new
complete-chunk boundary, and the listener drains every ready chunk. The process-lifetime hotkey
thread is likewise started by a function instead of returning an ownership-free zero-sized manager.
Live WAV encoding stays behind `take_ready_chunk()`'s `Option` boundary because the listener cannot
recover from an in-memory fixed-format encoder failure. The recorder logs that internal failure and
leaves PCM buffered for stop-time recovery.

Finalization cancellation now uses two `AtomicBool` checkpoints instead of holding a mutex across
platform text injection. Shutdown therefore never waits for an injection already in progress;
cancellation observed before the final checkpoint suppresses delivery, while an injection that has
already started may complete.

## Context

The desktop listener currently owns a hand-written loop that, every 20 ms:

1. pumps pending AppKit or Win32 messages;
2. polls tray/menu receivers;
3. polls one hotkey event;
4. polls one ready audio chunk; and
5. sleeps until the next iteration.

This keeps the tray functional on macOS and Windows, but it has three structural costs:

- an idle application wakes roughly 50 times per second even when no work exists;
- input and audio latency is tied to the polling interval and one-item-per-iteration draining;
- `StopSession` waits for transcription convergence, post-processes text, and injects it on the
  listener thread, so the native tray message pump is unresponsive during finalization.

`tray-icon` supports forwarding tray and menu callbacks through a winit `EventLoopProxy`. Winit can
therefore own the required main-thread native event loop while existing producer threads wake it
only when application work is available.

## Goals

1. Replace the fixed 20 ms listener loop and platform-specific message pumping with a winit event
   loop running on the main thread in `ControlFlow::Wait` mode.
2. Normalize hotkey, tray, audio-ready, background-completion, and shutdown notifications into one
   application event boundary.
3. Keep `RecordingSessionMachine` as the sole recording lifecycle authority and preserve current
   Hold, Toggle, tray debounce, Session ID routing, rollback, and partial-result behavior.
4. Keep the native event loop responsive while transcription converges, optional LLM cleanup runs,
   and text is injected.
5. Preserve the realtime audio callback boundary: it may append PCM and send a lightweight wakeup,
   but it must not encode WAV, wait on a channel, access the network, or execute session effects.
6. Make shutdown prevent late background work from injecting text after exit was requested.

## Non-goals

- Adding a visible window, settings UI, overlay, dock icon, or new user-facing recording mode.
- Introducing Tokio or converting the blocking HTTP clients to async APIs.
- Replacing `rdev`, `cpal`, `tray-icon`, the session state machine, or platform text injection.
- Changing chunk size, request retry, convergence timeout, post-processing, or configuration
  policy.
- Making the detached `rdev::listen` thread independently stoppable; process shutdown remains its
  lifetime boundary.

## Design

### 1. Make winit the main-thread lifecycle owner

Add stable winit `0.30.x` as a direct dependency and build one `EventLoop<AppEvent>` for listener
mode. The loop is created and run on the process main thread, uses `ControlFlow::Wait`, and does not
create a window. Tray construction remains on that same thread and macOS retains Accessory
activation policy.

The listener will implement `ApplicationHandler<AppEvent>` through a small application object that
owns the current runtime components: `RecordingSessionMachine`, `AudioRecorder`, tray manager,
orchestrator, post-processor, typer, and any active finalization cancellation handle. Its
`user_event` callback dispatches one event and drains any immediately generated state-machine
events before returning control to winit.

The existing `pump_platform_events()` implementations, `tray.update()`, the heartbeat counter, the
fixed sleep, and polling loop are removed. Winit becomes the only owner of AppKit/Win32 event
dispatch for listener mode. CLI-only workflows continue to run without constructing a desktop
event loop.

### 2. Use one application event boundary

The application layer introduces a private event type along these lines:

```rust
enum AppEvent {
    Hotkey(HotkeyEvent),
    Tray(TrayAction),
    AudioChunkAvailable { session_id: SessionId },
    FinalizationFinished { session_id: SessionId },
}
```

The exact private representation may be simplified during implementation, but every cross-thread
producer must ultimately call `EventLoopProxy::send_event`; shared component APIs will accept
narrow callbacks rather than depend directly on winit. This keeps `input` and `audio` independent
of the application framework and lets tests substitute a deterministic event collector.

Application events are normalized into the existing source-free `SessionEvent` values before
entering `RecordingSessionMachine`. Each routed event retains its `SessionId`; a completion or
audio notification from an older session cannot mutate a newer session.

Sending after event-loop shutdown is expected and harmless. Producers may discard the closed-loop
error, but operational failures before shutdown must continue to be logged at the layer that can
act on them.

### 3. Forward hotkey and tray callbacks instead of polling

`start_hotkey_listener` retains the current `rdev::listen` thread, event mapper, press/release
ordering, repeat suppression, and platform normalization. Instead of storing an `mpsc::Receiver`
for `check_event()`, it invokes an application callback for each mapped `HotkeyEvent`. The callback
only forwards to the winit proxy.

`TrayManager` will register `TrayIconEvent::set_event_handler` and
`MenuEvent::set_event_handler`. Raw tray events remain filtered by tray ID and normalized through
the existing debounce rules before producing `TrayAction`; Exit is never debounced. The one-shot
global handlers directly retain the callback for the process lifetime, matching the single-listener
application lifecycle.

Removing batch polling also removes the old artificial rule that a queued Exit event is inspected
before a queued icon event. Under the event loop, callbacks are handled in delivered order; once
Exit is accepted, the existing `ShuttingDown` state rejects later recording controls.

### 4. Turn audio chunk readiness into a wakeup

The CPAL callback already publishes PCM before advancing its ready-chunk count. Extend that boundary
with a lightweight notifier that fires when a new complete chunk boundary becomes available. The
notification contains only the active `SessionId`; WAV encoding and buffer draining stay on the
listener thread through `take_ready_chunk()`.

On `AudioChunkAvailable`, the listener drains all complete chunks in order rather than taking at
most one per loop iteration. The callback notifies only when the complete-chunk count advances, not
for every audio sample buffer. Each advanced boundary can therefore queue an independent wakeup;
an event published while the listener drains remains queued, and duplicate wakeups are harmless
because every handler drains until no complete chunk remains. The final implementation keeps the
fixed-format in-memory WAV encoder's failure internal to the recorder: it logs the failure, returns
`None`, and leaves the affected PCM buffered for stop-time recovery.

Stopping the recorder continues to synchronously detach the CPAL stream and encode the final
complete slices/tail before those chunks are submitted in order. A readiness notification that
arrives after stop is harmless because recorder/session routing rejects stale work.

### 5. Finalize stopped sessions away from the event-loop thread

After recorder stop and final chunk submission, launch one session-scoped background finalization
task. That task performs the currently blocking sequence:

```text
SessionOrchestrator::finish_session
  -> optional post-processing
  -> platform text injection
  -> AppEvent::FinalizationFinished
```

The listener remains in the existing `Stopping { session_id }` state until the matching completion
event feeds `SessionStopped` back into the state machine. Recording controls remain ignored while
stopping, matching current observable behavior, but tray Exit and native application events remain
responsive.

The orchestrator, post-processor, and typer will be shared with the finalization task only through
the minimum thread-safe ownership needed by their existing contracts. This does not add an async
runtime or a general worker pool: at most one recording session can be active, so one scoped
finalization thread is sufficient.

Each finalization task owns a cancellation/delivery gate. The task checks cancellation before
post-processing, then holds the gate across the final text injection; `ShutdownRequested` acquires
the same gate before marking the task cancelled. This gives shutdown and delivery a defined order:
an injection already underway finishes before shutdown is accepted, while shutdown accepted first
prevents later injection. Late completion events carry the old Session ID and are ignored after
shutdown. The process is not required to wait for a blocking network request to finish in order to
exit.

Implementation deviation: the final code replaces that mutex protocol with an atomic cancellation
flag checked before post-processing and immediately before injection. This keeps shutdown
non-blocking even after injection begins, accepting that an already-started platform injection may
finish.

### 6. Keep event-loop glue thin and testable

Split the current listener integration so platform/framework glue does not further enlarge one
file:

```text
src/application/listener.rs
  - configuration and component construction
  - EventLoop/AppEventProxy setup
  - source callback wiring

src/application/listener/event_loop.rs
  - private AppEvent and ApplicationHandler implementation
  - application-event normalization
  - state-machine effect execution
  - audio draining and finalization lifecycle
```

The existing state machine remains in `core`; winit types do not cross into `core`, `audio`, or
`input`. Small callback interfaces are preferred over a repository-wide event bus or generic actor
abstraction.

## Expected File Changes

| File | Planned change |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Add stable winit `0.30.x`; remove native event-pump-only dependency features where no longer needed. |
| `src/application/listener.rs` | Construct and run the winit loop, wire producers to its proxy, and remove the fixed polling loop. |
| `src/application/listener/event_loop.rs` | Add the private application event handler, effect executor, audio draining, cancellation, and background finalization coordination. |
| `src/input/hotkey.rs` | Deliver mapped hotkeys through a supplied callback instead of a polled receiver. |
| `src/input/tray.rs` | Forward native tray/menu callbacks for the process lifetime, retain debounce/filtering, and remove manual AppKit/Win32 pumping. |
| `src/input/typer.rs` and platform typers | Express the thread-safety contract required for background delivery without changing injection behavior. |
| `src/audio/recorder.rs` | Emit one lightweight wakeup when the complete-chunk count advances while keeping callback work realtime-safe. |
| `docs/architecture/input.md` | Describe callback delivery, winit ownership, tray handler lifecycle, and removal of polling. |
| `docs/architecture/audio.md` | Describe per-boundary wakeups and main-thread chunk draining. |
| `docs/architecture/core.md` | Describe the application event boundary and non-blocking stop/finalization flow. |
| `README.md` | Add winit to the dependency inventory; user workflow remains unchanged. |
| `docs/README.md` | Index this plan and track its implementation status. |
| `changelog` | Record the event-driven listener and responsive background finalization. |

No configuration/example change is expected because the feature adds no user setting or schema
change. Packaging and release documentation are unaffected because artifact names, supported
targets, and release procedures do not change.

## Test-First Implementation Order

After explicit plan approval:

1. Add deterministic tests for application-event normalization and forwarding, including ordered
   Hold press/release delivery and rejection of stale Session IDs.
2. Add recorder tests proving that each observed chunk boundary wakes the listener and draining
   returns all ready chunks in order.
3. Add tray reducer/handler tests proving left-click debounce, double-click suppression, unrelated
   ID filtering, and Exit delivery without constructing a native tray.
4. Add finalization tests with fake processor/typer behavior proving the event-loop-facing call
   returns without waiting, matching completion reaches `Idle`, and shutdown cancellation prevents
   late text injection.
5. Add winit and the private application event handler, then wire the tested hotkey, tray, audio,
   and completion producers through `EventLoopProxy`.
6. Remove the old polling receivers, native pump code, heartbeat, and fixed sleep only after every
   event source has an explicit wakeup path.
7. Synchronize architecture docs, dependency inventory, plan status, index, and changelog with the
   final implementation.
8. Run focused tests throughout, then the full cross-platform validation set and inspect the final
   diff for stale polling descriptions or retired native-pump symbols.

Tests will use fake callbacks, channels, clocks, processors, and typers. They will not create a
real winit loop, tray icon, microphone stream, network request, or platform input event in unit-test
processes.

## Validation

```bash
cargo fmt --check
cargo test application::listener
cargo test input::hotkey::tests
cargo test input::tray::tests
cargo test audio::recorder::tests
cargo test
cargo clippy -- -D warnings
git diff --check
```

The PR must also pass the existing macOS build/test/Clippy CI and Windows build/test CI. Before PR
readiness, perform a macOS smoke run that confirms tray left-click, right-click Exit, Hold, Toggle,
idle waiting, chunk submission, and post-stop text injection remain functional. Windows-native
behavior is validated by the Windows CI build/tests unless an interactive Windows runner is
available.

## Acceptance Criteria

- Listener mode uses one main-thread winit event loop with `ControlFlow::Wait` and creates no
  application window.
- There is no fixed-rate listener sleep, tray/hotkey polling receiver, or hand-written AppKit/Win32
  message pump.
- Hotkey, tray/menu, audio-ready, and finalization producers wake the loop through custom events.
- The CPAL callback performs no WAV encoding, blocking send, network access, state transition, or
  text processing.
- Every published ready audio chunk is drained in order without depending on periodic polling;
  boundary notifications published during draining remain independently queued, and stale events
  cannot mutate another session.
- Live WAV encoding failures are logged explicitly and retain PCM for stop-time recovery rather than
  entering a timer-driven retry loop.
- Existing Hold, Toggle, tray debounce, startup rollback, stop failure, chunk ordering, partial
  transcription, and Session ID routing behavior remains intact.
- Transcription convergence, optional LLM cleanup, and text injection do not block native event
  dispatch.
- Exit remains responsive during finalization; cancellation observed before the injection checkpoint
  prevents delivery, while an injection already underway may finish.
- macOS remains an Accessory application with a functional status item; Windows retains a
  functional tray icon and context menu.
- Focused tests, full tests, formatting, Clippy, diff checks, and cross-platform CI pass.
