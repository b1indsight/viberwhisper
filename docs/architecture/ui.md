# Desktop UI Architecture

## Purpose

`ui` owns the desktop listener and setup interaction. `core` owns process entry points, CLI
workflows, recording settings, configuration, the recording state machine, and transcription
orchestration. The entry points connect these modules directly.

## Modules

| File | Responsibility |
| --- | --- |
| `src/ui.rs` | Desktop interaction module declarations |
| `src/ui/listener.rs` | Listener service assembly and platform-action mapping |
| `src/ui/listener/event_loop.rs` | Winit events, session effects, and background finalization |
| `src/ui/preview.rs` | Shared preview protocol, confirmation state, cancellation, history deduplication and guarded delivery |
| `src/ui/preview_demo.rs` | Feature-gated native preview demo and held-Enter smoke fixture |
| `src/ui/setup.rs` | Setup wizard, native dialogs, and test-recording workflow |
| `src/ui/setup/hotkey.rs` | Short-lived hotkey helper processes and their protocol |

## Listener

`ui::listener` accepts the existing `core::listener::RecordingConfig` or `ListenerConfig`.
It creates the native platform, recorder, orchestrator, and delivery or dataset-capture output.
The event-loop handler owns these resources and executes the existing state machine's effects.
The listener's private `AppEvent` carries platform events, audio readiness, native preview actions,
shared preview updates, and session completion. `ui::preview` owns `PreviewUpdate`, worker commands,
the cancellation gate and confirmation state; it does not depend on the listener or winit.
The demo uses its own event envelope and shares the same preview worker and pending state.
Each pending candidate stores one session ID and a single confirmation phase: ready, processing,
or retry blocked. Matching-session cancellation remains available after retries are blocked,
and queued callbacks are rejected while an accepted command is in progress.

Finalization prepares text and captures the destination on its worker before displaying the
main-thread native preview. Only text, destination labels, session IDs and commands cross the
thread boundary. The worker waits for confirmation or cancellation and retains the native target.
The recording machine stays `Stopping` until that worker returns; the microphone and tray already
show stopped. Pending recording requests focus the current preview. Confirmation accepts one
transaction at a time, carries the current edited text, saves each confirmed revision once,
restores the target and validates it after the platform
focus delay. Failures preserve the candidate; partial SendInput acceptance disables automatic retry.
Cancel drops the candidate without saving history; exit cancels the gate and disconnects the worker.

The prepared preview also retains an optional opaque Realtime confirmation capability. History,
native injection and feedback use the same edited text. Only successful injection consumes the
capability; failed attempts retain it for the next edit/retry, and cancellation drops it. The UI
does not interpret model candidates or write recognition memory. Learning/persistence failures
are logged after delivery and never trigger another injection.
Feedback submission does not wait for a persistence acknowledgement; the recognition worker
commits each feedback message before processing the next recording. On normal exit the listener
explicitly stops the orchestrator, even if a delivery thread still holds a reference. Realtime
shutdown rejects unconfirmed feedback, cancels network I/O and joins after accepted feedback is saved.

The UI directly uses the existing business interfaces. Recorder rollback, ordered tail-chunk
submission, background finalization, and cancellation remain in the same call chain. The state
machine and orchestrator do not depend on UI or winit types. `input` and `platform` continue to
own hotkey mapping, tray components, clipboard operations, and native text injection.

## Setup

The wizard stays together with its existing `SetupUi` and `SetupVerifier` boundaries and test
doubles. `NativeVerifier` handles one complete hotkey-driven recording and verifies it through
the existing audio, STT, and post-processing interfaces. `core::config` owns configuration
loading and atomic persistence; the wizard selects when to load, retry, cancel, or save.

Windows text and password input use in-process Win32 dialogs in `ui/setup/windows.rs`, with
Unicode controls and a native modal message loop. They do not launch VBScript/HTA input scripts
or poll for another process's window. Text/password methods distinguish an accepted value,
cancellation, and a native error; failures propagate without saving the candidate configuration.
Windows confirmations and informational messages also call `MessageBoxW` directly, using no
external owner window. They never select console input based on `SSH_CLIENT`/`DISPLAY`.
Confirmation/message creation errors abort the current workflow instead of becoming a No
answer; failure of the final saved notification does not undo an already completed save.
The CLI reports errors through its existing error return and the desktop launcher shows its
existing startup-error dialog. Other platforms keep their existing tinyfiledialogs inputs.

Windows tests drive real native inputs in a bounded child process. Release packaging also runs
`scripts/test-windows-setup.ps1` against both release executables on a clean runner account,
checking the initial confirmation, text/password windows, and cancellation without writing
config. It covers explicit setup, both first-run entries, and direct/ShellExecute GUI launches
with and without the environment that previously selected console-only confirmations.

The process entry points check for helper mode before tracing and CLI initialization. Normal
listener startup and the explicit setup command use the same wizard. CLI-only workflows run
without creating a desktop event loop.
