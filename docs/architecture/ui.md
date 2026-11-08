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

Realtime starts this same delivery worker when recording starts, captures the destination before
any preview can take focus, and waits for the recorder's stop signal before finalization. Native
target objects stay on that worker. A recording-scoped latest-text mailbox wakes the UI without
queuing every token. Provisional text is read-only/selectable, Enter cannot confirm it, and no
`PendingPreview` command channel exists until final validation. During recording the bubble is
non-activating; stop focuses it once. Esc closes progress, cancels the recognition token even
after finalizer detachment, and drives session-scoped recorder/orchestrator cleanup. The machine
stays `Stopping` until cleanup and destination restoration finish, preventing restart races.

Ready closes the mailbox before enabling editing, so old progress cannot overwrite manual edits.
The same native window replaces only a word-aligned changed tail; unchanged text is not replaced
and pure additions append. Shared `platform::preview::TextPatch` uses Chinese segmentation and
Unicode word/grapheme boundaries; native adapters convert byte offsets to UTF-16 (including
Windows CRLF), preserve prefix selection and scroll, and clear programmatic undo/held-key state
at the ready transition. Failure discards the current unvalidated tail; only validated partial
results can enter the editable warning/confirmation path. HTTP and dataset capture keep their
existing finalization path.

Recognition failure or convergence timeout with no usable text retains an empty, read-only preview
and a distinct cancellation warning. Existing retry-blocked state rejects Enter but allows Esc;
the worker keeps waiting until cancellation, then restores its saved destination. Its initial empty
body cannot acquire delivery capability from later confirmation text, so error previews never inject,
save history, or submit learning. Nonempty partial results remain editable with their warning;
empty success, silence, and no-chunk results stay quiet. The integration render fixture uses the
same finalization adapter and checks both replacing progress and opening a fresh error preview.

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
the selected Realtime or HTTP recognition path without reading or updating personal memory.
`configure_recognition` first selects ordinary HTTP STT or Realtime, defaulting to the loaded
mode. Only the selected connection is edited: HTTP retains optional LLM setup, while Realtime
asks for URL, model, and an environment-variable name (empty disables authentication).
Inactive connections and all advanced prompt, temperature, preheat, and memory fields stay
unchanged. Existing LLM enablement and unavailable microphone names remain the defaults.
Unchanged input retains the exact original value even when tinyfiledialogs needs a sanitized
display default. HTTP password inputs start empty; environment secrets never enter saved JSON.
`core::config` owns configuration
loading and atomic persistence; the wizard selects when to load, retry, cancel, or save.
Cancelling an input, declining the final save, or a UI error before that save leaves the file intact.

Windows text and password input use in-process Win32 dialogs in `ui/setup/windows.rs`, with
Unicode controls and a native modal message loop. They do not launch VBScript/HTA input scripts
or poll for another process's window. Text/password methods distinguish an accepted value,
cancellation, and a native error; failures propagate without saving the candidate configuration.
Windows confirmations and informational messages reuse the input dialog's aligned Win32
template, bypassing `MessageBoxW`'s observed system-aware window creation on a PMv2 thread.
They use no external owner and never select console input based on `SSH_CLIENT`/`DISPLAY`.
Their scrollable text and Chinese buttons match the input dialogs, without system message-box
icons. Yes/no questions honor the configured default and ignore Esc/Close; informational
messages accept OK, Esc, or Close.
Confirmation/message creation errors abort the current workflow instead of becoming a No
answer; failure of the final saved notification does not undo an already completed save.
The CLI reports errors through its existing error return and the desktop launcher shows its
existing startup-error dialog. Other platforms keep their existing tinyfiledialogs inputs.

Both Windows process entries establish and check Per-Monitor V2 DPI awareness before the first
dialog or hotkey helper, rather than waiting for winit's event-loop construction. Standard
9pt Segoe UI setup dialogs retain dialog units and Windows' automatic PMv2 scaling.
Windows tests drive real native inputs and preview controls in a bounded child process. They
check actual HWND DPI contexts and fonts, then inject synthetic 96/144/192 DPI notifications,
including a notification re-entering layout, without claiming a physical high-DPI display test.
CI and release packaging run `scripts/test-windows-setup.ps1` against both executables on a clean
runner account, checking the initial confirmation, both recognition paths, text/password
windows, PMv2 contexts, required-input messages, and cancellation without writing config. It
covers explicit setup, both first-run entries, and direct/ShellExecute GUI launches with and
without the environment that previously selected console-only confirmations. CI also requests non-password window
captures from that isolated account for visual inspection; hosted-display images do not prove
real 4K or mixed-monitor clarity.

The process entry points check for helper mode before tracing and CLI initialization. Normal
listener startup and the explicit setup command use the same wizard. CLI-only workflows run
without creating a desktop event loop.
