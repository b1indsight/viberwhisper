# 14 - Tray-Only Recording Control

## Status

Implemented on macOS and Windows. The floating overlay module was removed; tray/hotkey input now feeds an input-independent recording state machine with structured recorder outcomes and end-to-end `SessionId` routing. Asynchronous finalization remains tracked separately in issue #73.

## Goal

Remove the floating overlay window and make the system tray/status bar icon the only on-screen recording control. A left click on the tray icon toggles recording, while the existing hotkeys and tray exit menu continue to work.

## User-Visible Behavior

1. Starting ViberWhisper creates no floating window on macOS or Windows.
2. A single left click on the tray icon while idle starts a Toggle-mode recording session.
3. A single left click on the tray icon while recording stops the session, transcribes it, and injects the result through the existing finalization path.
4. The tray icon and tooltip continue to show idle and recording states.
5. Right click continues to open the tray menu; the exit item remains available.
6. Hold and Toggle hotkeys retain their current behavior and update the same tray state.
7. Repeated tray clicks inside the effective debounce window are ignored, preventing a double click from immediately undoing the first action. The window is never shorter than 300 ms and respects the platform double-click interval when available.

## Non-Goals

- Redesigning the tray icon artwork or menu.
- Removing or changing the F8/F9 hotkeys.
- Adding a configuration option to restore the overlay.
- Changing audio capture, transcription, post-processing, or text injection behavior.
- Adding Linux tray-click support. `tray-icon` 0.21 does not emit tray icon click events on Linux, and Linux is not a supported desktop target for this repository.

## Pre-Implementation State

`run_listener_with_config` currently creates both `TrayManager` and `OverlayManager`. The overlay has four responsibilities:

- create and display a native floating window;
- publish overlay click events;
- reflect recording state;
- pump native UI events on macOS and Windows.

The tray currently reflects recording state and polls the exit menu, but it does not consume `TrayIconEvent` clicks. Its menu opens on left click by default, which conflicts with using left click as the recording control.

## Proposed Design

### 1. Make tray input explicit

Add a small application-facing tray action enum:

```rust
enum TrayAction {
    ToggleRecording,
    Exit,
}
```

Replace the exit-only polling surface with `TrayManager::check_action() -> Option<TrayAction>`. The manager will inspect both `TrayIconEvent::receiver()` and `MenuEvent::receiver()` without blocking.

Menu actions have priority over recording-toggle events. Each poll checks the Exit menu event before consuming tray click events, so a burst of left-click input cannot starve or delay an already-queued Exit action.

Only a tray event matching all of these conditions triggers `ToggleRecording`:

- event belongs to this `TrayManager`'s icon ID;
- event is `TrayIconEvent::Click`;
- mouse button is `MouseButton::Left`;
- mouse state is `MouseButtonState::Up`.

Filtering for the button-up event is required because `tray-icon` emits both button-down and button-up notifications on supported platforms. Filtering by icon ID prevents unrelated tray instances in the same process from triggering recording.

Build the icon with `with_menu_on_left_click(false)`. Left click is therefore reserved for recording; right click remains the context-menu gesture.

### 2. Debounce accepted tray toggles and suppress native double clicks

Apply a leading-edge debounce after native event classification and before publishing `TrayAction::ToggleRecording`. Compute the effective window once when the tray manager is created:

```text
effective_window = max(300 ms, platform_double_click_interval)
```

- macOS: read `NSEvent::doubleClickInterval()` and convert its seconds value to `Duration`;
- Windows: read `GetDoubleClickTime()` and convert its milliseconds value to `Duration`;
- tests/other targets/API failure: use the 300 ms baseline.

The event rules are:

- the first valid tray left-button-up event is accepted immediately;
- another valid tray toggle event strictly inside the effective window after the last accepted event is ignored;
- an event at or after the effective-window boundary is accepted;
- ignored events do not move the last-accepted timestamp, so repeated input cannot extend the blocked period indefinitely.

`tray-icon` exposes a separate `TrayIconEvent::DoubleClick` on Windows but not on macOS. A left-button `DoubleClick` for this tray icon never publishes a recording action. It refreshes a separate suppression deadline for one effective window so the trailing Windows `Click(Up)` is rejected even when the first and second clicks are more than 300 ms apart. Ordinary rejected click events do not refresh that deadline.

Use `std::time::Instant` so wall-clock changes cannot affect the debounce window. Keep the timestamp in a small tray-owned debounce helper that accepts an explicit `Instant`; production passes `Instant::now()`, while unit tests pass deterministic timestamps without sleeping.

The debounce applies only to tray-originated `ToggleRecording` actions. It must not delay or discard the Exit menu action, right-click menu behavior, or F8/F9 hotkey events. It prevents duplicate/rapid native input, but intentionally does not try to distinguish a genuine single click from a single accidental click.

### 3. Introduce an input-independent recording session machine

Move recording lifecycle decisions out of the hotkey and tray branches into a platform-independent `RecordingSessionMachine`. External input adapters only normalize native input into control events:

```rust
enum ControlSource {
    HoldHotkey,
    ToggleHotkey,
    Tray,
}

enum ControlAction {
    Start(SessionMode),
    Stop,
    Toggle(SessionMode),
}

struct ControlEvent {
    source: ControlSource,
    action: ControlAction,
}
```

The adapters map input without inspecting recorder state:

| External input | Control event |
|---|---|
| Hold hotkey pressed | `Start(Hold)` from `HoldHotkey` |
| Hold hotkey released | `Stop` from `HoldHotkey` |
| Toggle hotkey pressed | `Toggle(Toggle)` from `ToggleHotkey` |
| Accepted tray left click | `Toggle(Toggle)` from `Tray` |

The session machine owns the authoritative lifecycle state:

```rust
enum RecordingState {
    Idle,
    Starting { session_id, mode, source, phase },
    Recording { session_id, mode, source },
    Stopping { session_id, mode, source, phase },
    Recovering { session_id: Option<SessionId> },
    ShuttingDown { session_id: Option<SessionId> },
}
```

It consumes both external `ControlEvent`s and structured internal completion events. It emits declarative, session-tagged effects for the runtime to execute, such as `StartRecorder`, `StartOrchestrator`, `StopRecorder`, `SubmitChunk`, `FinishOrchestrator`, `AbortOrchestrator`, `CancelRecorder`, `SetTrayRecording`, and `ReadyToExit`.

`main.rs` is the effect executor, not the lifecycle authority:

1. dispatch one normalized event to the session machine;
2. execute the returned effect;
3. feed the effect result back as an internal session event;
4. continue until no immediate effects remain.

Neither the tray adapter nor hotkey adapter may branch on `recorder.is_recording()`, directly start/stop the orchestrator, or update the tray recording indicator. The tray indicator is derived only from successful session transitions.

#### Structured recorder outcomes

The current recorder API is not precise enough for state-machine transitions: duplicate start returns `Ok(())`, while stop can set its internal recording flag to false and then return an error while writing the final audio. Replace boolean success/failure assumptions with structured outcomes carrying the recorder's actual state and session identity:

```rust
enum RecorderStartOutcome {
    Started { session_id: SessionId },
    AlreadyRecording { active_session_id: SessionId },
}

enum RecorderStopOutcome {
    Stopped {
        session_id: SessionId,
        chunks: Vec<String>,
        warning: Option<RecorderStopError>,
    },
    StillRecording {
        session_id: SessionId,
        error: RecorderStopError,
    },
    NotRecording {
        requested_session_id: SessionId,
    },
}

struct ReadyChunk {
    session_id: SessionId,
    path: String,
}
```

`Stopped` means the audio stream is definitively inactive. Its `chunks` contain any stop-time tail/chunk paths that were successfully produced. A tail write failure is preserved as `warning` instead of being confused with `StillRecording`; the state machine still submits available chunks and finishes the matching orchestrator session so previously submitted live chunks are not leaked. `StillRecording` returns the machine to `Recording` with the tray indicator still on. `NotRecording` is an invariant mismatch: the machine finishes the matching orchestrator session once so any previously submitted live chunks can converge, records the mismatch, and returns to Idle without attempting a second recorder stop.

`start_recording(session_id)` stores the ID in the recorder, and `take_ready_chunk()` returns `ReadyChunk` with that ID. `AlreadyRecording` never counts as start success. The machine enters recovery, cancels the observed orphan recorder session, aborts any matching orchestrator session, and returns to Idle rather than adopting unknown audio.

Audio-file cleanup follows explicit ownership transfer. The recorder owns buffered audio and files until it emits a `ReadyChunk` or stop outcome. When a matching `SubmitChunk` effect is accepted by the orchestrator, ownership of that path transfers to the orchestrator worker; the recorder removes it from its cleanup set. Rejected/stale paths remain with the effect executor and are deleted immediately. Cancellation only deletes files still owned by that component, preventing double deletion and orphaned paths.

All recorder-generated paths are grouped under a unique `SessionId`-scoped directory. Recorder history cleanup does not recurse into these directories, so transferring a chunk out of the recorder's local ownership set cannot expose a queued or in-flight orchestrator file to age-based deletion. Every deletion path also attempts to remove the session directory after its last file is released.

#### End-to-end session identity

`SessionId` is a monotonically increasing newtype owned by the session domain. It must travel through every session-specific API and effect, not only final completion events:

```rust
start_recording(session_id)
take_ready_chunk() -> Option<ReadyChunk>
stop_recording(session_id) -> RecorderStopOutcome

start_session(session_id, mode) -> Result<(), SessionStartError>
on_chunk_ready(session_id, path) -> Result<usize, SessionRoutingError>
finish_session(session_id) -> Result<String, SessionError>
abort_session(session_id) -> Result<(), SessionRoutingError>
```

`SessionOrchestrator::start_session` must no longer silently replace an active session. Start, chunk, finish, and abort calls reject a mismatched ID. A rejected/stale chunk is deleted and cannot be attached to the current session. Every recorder/orchestrator completion event carries its originating ID; the state machine ignores stale completions and may emit cleanup effects, but never mutates the active session from them.

The normal start chain is `StartRecorder -> RecorderStarted -> StartOrchestrator -> OrchestratorStarted -> Recording`. If orchestrator start fails after the recorder started, the machine cancels that recorder session and returns to Idle. The normal stop chain is `StopRecorder -> RecorderStopOutcome -> SubmitChunk* -> FinishOrchestrator -> FinalizeCompleted -> Idle`; each effect and acknowledgement uses the same ID.

#### Shutdown is a session event

The tray Exit action becomes `SessionEvent::ShutdownRequested` instead of breaking directly out of the main loop. Once accepted, the machine ignores new control events and enters `ShuttingDown`.

Shutdown policy for this change is explicit cancellation, not transcription:

- Idle emits `ReadyToExit` immediately;
- Starting/Recording/Stopping cancels the matching recorder stream without serializing a final WAV;
- Recovering continues its existing cleanup under shutdown semantics and then emits `ReadyToExit`;
- repeated Shutdown while already `ShuttingDown` is an idempotent no-op and cannot emit a second exit effect;
- the matching orchestrator session is aborted without waiting for transcription convergence;
- recorder-owned buffers/files are cleared immediately; queued orchestrator chunks are marked cancelled and deleted without transcription; an already in-flight request retains ownership of its file and must delete it when the configured request timeout/completion is reached;
- the tray is reset to idle;
- no final transcription or text injection is produced after Exit;
- cleanup errors are logged but cannot prevent `ReadyToExit` indefinitely.

Add a dedicated `cancel_recording(session_id)` path so Exit does not pay the normal stop-time sleep/WAV-write cost. `main.rs` exits only after the state machine emits `ReadyToExit`; it never bypasses session cleanup with a direct `break`.

Required transition behavior:

- `Idle + Start(Hold)` starts a Hold session;
- `Idle + Toggle(Toggle)` starts a Toggle session;
- `Recording + Toggle(Toggle)` requests one stop regardless of whether F9 or the tray produced it;
- a Hold release stops only the active Hold session;
- a Hold release after the tray/F9 already stopped that Hold session is an `Idle + Stop` no-op, not an error;
- a new Start while `Starting`, `Recording`, or `Stopping` is ignored rather than calling the recorder twice;
- start success starts exactly one orchestrator session and turns the tray recording indicator on;
- start failure returns to Idle and keeps the tray indicator off;
- stop success returns to Idle, turns the tray indicator off, and finalizes exactly once;
- stop failure follows the structured recorder outcome without guessing from a generic error;
- a ready chunk is routed only when its session ID matches the active session;
- Shutdown from every state produces cancellation/cleanup effects and exactly one `ReadyToExit`.

The current implementation remains synchronous, but the session-tagged event/effect boundary prevents stale completions and chunks from mutating a newer session and prepares the state machine for the asynchronous finalization work tracked in #73.

### 4. Preserve the native event pump

Deleting `OverlayManager::update()` without replacement would break or destabilize native tray delivery. Move the non-blocking platform event pump behind `TrayManager::update()`:

- macOS: drain pending AppKit events on the main thread and dispatch them through `NSApp`;
- Windows: drain the current thread's Win32 queue with `PeekMessageW`, `TranslateMessage`, and `DispatchMessageW`;
- tests/other targets: no-op.

Call `tray.update()` once per main-loop iteration before polling tray actions. Tray creation and event pumping stay on the listener's main thread.

The minimum compatibility contract for this migration is that, during the normal idle and recording loop, right click always opens the native tray menu and its Exit item remains actionable. The per-iteration order is therefore:

1. pump pending native UI events;
2. check and prioritize the Exit menu action;
3. classify and debounce tray recording clicks;
4. normalize tray/hotkey input into control events and dispatch them to the recording session machine;
5. execute session effects and feed their results back as internal events.

This change does not claim menu responsiveness while the existing synchronous stop/finalize path is blocking the main thread during transcription convergence. Providing that stronger guarantee requires moving finalization off the UI loop and is outside this overlay-removal scope.

### 5. Remove overlay code and dependencies

Remove the overlay module export, all `OverlayManager` construction/state/click/update calls, and the platform overlay implementations. Retain only the native bindings/features needed by the tray event pump; remove AppKit drawing/window features that become unused.

## Planned File Changes

| File | Change |
|---|---|
| `src/main.rs` | Remove overlay lifecycle; normalize external input; execute effects emitted by the recording session machine |
| `src/core/recording_session.rs` | Add the input-independent session state machine, event/effect types, session IDs, and transition tests |
| `src/core/mod.rs` | Export the recording session module |
| `src/audio/recorder.rs` | Tag recordings/chunks with `SessionId`; return structured start/stop outcomes; add fast cancellation for shutdown |
| `src/core/orchestrator.rs` | Make start/chunk/finish/abort APIs session-aware; reject replacement/mismatched IDs; add bounded abort cleanup |
| `src/input/tray.rs` | Add tray click classification, platform-aware debounce/double-click suppression, action polling, left-click menu policy, and native event pumping |
| `src/input/mod.rs` | Remove the overlay module export |
| `src/input/overlay/` | Delete macOS, Windows, and stub overlay implementations |
| `Cargo.toml` / `Cargo.lock` | Remove overlay-only native features or dependencies while retaining event-pump requirements |
| `README.md` | Replace floating-window instructions with tray-click behavior |
| `docs/architecture/core.md` | Document the recording session state machine and effect execution boundary |
| `docs/architecture/input.md` | Document tray actions/event pumping and remove overlay architecture |
| `docs/plan/09-floating-window.md` | Mark the overlay feature as superseded by this plan |
| `changelog` | Record removal of the overlay and tray click-to-toggle behavior |

## TDD and Verification

Implementation will follow this order:

1. Add table-driven state-machine tests for every Idle/Starting/Recording/Stopping/Recovering/ShuttingDown control transition before implementing the machine.
2. Add recorder contract tests for `Started`, `AlreadyRecording`, `Stopped` with and without warnings/chunks, `StillRecording`, `NotRecording`, ready-chunk IDs, and cancellation cleanup.
3. Add orchestrator contract tests proving duplicate start and mismatched chunk/finish/abort IDs are rejected without replacing or mutating the active session.
4. Add effect-result tests covering structured start/stop outcomes, exactly-once orchestrator/finalize effects, stale session/chunk IDs, and tray-state derivation.
5. Add shutdown transition tests from every state: new controls are ignored, recorder/orchestrator cleanup is requested, finalization is not requested, and `ReadyToExit` is emitted exactly once despite cleanup errors.
6. Add mixed-source tests: F9 start/tray stop, tray start/F9 stop, Hold start/tray stop/Hold release, duplicate starts, and conflicting events while a transition is in progress.
7. Implement the structured recorder/orchestrator APIs and recording session machine until those tests pass.
8. Add unit tests for tray click classification: own-icon left-button up toggles; left-button down, right click, double click, and another icon ID do not.
9. Add deterministic debounce tests: first click is accepted, a click inside the effective window is ignored, the ignored click does not extend the window, and clicks at/after the boundary are accepted.
10. Add platform-window tests using injected durations: a 500 ms system interval overrides the 300 ms baseline, while an unavailable or shorter interval uses 300 ms.
11. Add Windows-sequence tests: first `Click(Up)` toggles, a `DoubleClick` more than 300 ms later does not toggle, and its trailing `Click(Up)` is suppressed.
12. Add macOS-sequence tests: two `Click(Up)` events more than 300 ms apart but inside an injected system double-click interval produce only the first toggle.
13. Add unit tests confirming tray debounce does not suppress Exit and does not apply to hotkey-driven toggles.
14. Add unit tests confirming a queued Exit action wins over queued or repeated tray toggle events.
15. Add unit tests for tray action draining where the native types can be isolated from the backend.
16. Implement tray action polling and connect normalized input to the session machine.
17. Remove overlay integration and move the event pump.
18. Update user and architecture documentation.
19. Run `cargo fmt --check`, `cargo check`, `cargo test`, and `cargo clippy --all-targets`.

Manual verification on both supported platforms:

1. Launch the app and confirm no floating window appears.
2. Left-click the tray icon once and confirm recording starts exactly once.
3. Left-click again and confirm recording stops, transcription completes, and text is injected.
4. Confirm the icon/tooltip changes for tray-started, Toggle-hotkey, and Hold-hotkey sessions.
5. While idle and while recording, right-click the icon and confirm the menu opens and Exit terminates the app.
6. Confirm a double click and rapid clicks inside the configured system double-click interval produce only the first toggle.
7. Confirm a deliberate second click after the effective debounce window toggles recording normally.
8. Confirm the right-click menu and Exit respond during the tray debounce window and after a burst of left clicks.
9. On macOS, verify tray clicks while another app is full screen.
10. Cross-control sessions with F8, F9, and tray clicks and confirm one start, one stop, one finalization, and correct tray state per accepted session.
11. Exit while idle and recording; confirm shutdown is prompt, no text is injected, the microphone closes, and no session-owned temporary audio remains indefinitely.

## Risks

### Duplicate click delivery

Native backends report multiple mouse phases. Strictly accepting only the left-button-up `Click` event prevents a normal click from toggling on both phases. The platform-aware debounce covers macOS, where `tray-icon` exposes no separate double-click event. On Windows, the separate `DoubleClick` event also refreshes a suppression deadline so its trailing button-up cannot reverse the first action.

### Debounce responsiveness

A debounce window that is too long makes an intentional quick stop feel unresponsive; one that is too short may not absorb a platform-recognized double click. Using the larger of 300 ms and the user-configured platform interval follows native double-click expectations, but users with a long system interval will also wait longer before a deliberate second tray toggle. The window remains limited to tray input and is covered at its exact time boundary. It is not exposed as separate application configuration unless platform testing demonstrates a concrete need.

### Event-loop regression after overlay deletion

The overlay currently pumps native events as a side effect. Moving that responsibility into `TrayManager` before deleting the overlay avoids a tray icon that renders but no longer responds. Right-click menu opening and Exit handling in both idle and recording states are the minimum release gate for this migration.

### Recording lifecycle drift

The recorder, orchestrator, and tray can drift if input handlers each mutate them directly or if effect failures create a second source of truth. The input-independent state machine prevents source-specific lifecycle branches, while end-to-end session IDs and structured recorder/orchestrator outcomes make reconciliation testable. The remaining implementation risk is keeping the effect executor thin: it must only normalize effect results into internal events and must not add lifecycle decisions outside the state machine.

### Shutdown cleanup

Immediate Exit must not wait for transcription convergence, but a blocking in-flight HTTP request cannot be forcibly terminated by the current worker. `AbortOrchestrator` therefore marks the session cancelled, causes queued chunks to be deleted without transcription, and lets an in-flight request finish within its configured request timeout before deleting its owned file. The UI does not wait for that worker, and all cleanup ownership must be explicit so detached work cannot inject text or mutate a new session.

## Acceptance Criteria

- No floating overlay window or overlay module remains in the runtime.
- One tray left click produces one recording toggle on macOS and Windows.
- Tray toggle clicks inside `max(300 ms, platform double-click interval)` are ignored without extending the debounce window.
- A Windows `DoubleClick` and its trailing `Click(Up)` do not publish a second recording toggle.
- Exit and hotkey actions remain responsive during the tray debounce window.
- During the idle and recording main loop, right click opens the menu and Exit remains actionable even when tray toggle events are queued.
- A queued Exit menu action takes priority over tray recording-toggle actions.
- Hotkeys and tray clicks only emit normalized control events; they do not inspect or mutate recording lifecycle state directly.
- The recording session machine is the sole lifecycle authority and drives recorder, orchestrator, finalization, and tray effects.
- Recorder start/stop outcomes distinguish actual recorder state from operation success, including errors after the stream has already stopped.
- `SessionId` is propagated through recorder start/stop, ready chunks, orchestrator routing, effects, and completion events.
- The orchestrator rejects duplicate starts and mismatched session IDs instead of replacing the active session.
- Mixed input sources cannot produce duplicate starts, duplicate stops, or more than one finalization for a session ID.
- A Hold release after another source stopped the Hold session is a no-op rather than an error.
- Stale internal events and chunks with an old session ID cannot mutate the active session or be routed into it.
- Exit is dispatched as `ShutdownRequested`; it cancels session work, produces no final text, and the main loop exits only after `ReadyToExit`.
- Tray visual state matches the recorder after successful transitions and failures.
- Hold and Toggle hotkeys continue to pass existing tests and manual checks.
- Native UI events continue to be pumped after overlay removal.
- Automated formatting, build, test, and lint checks pass.
