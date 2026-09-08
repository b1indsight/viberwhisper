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
| `src/ui/setup.rs` | Setup wizard, native dialogs, and test-recording workflow |
| `src/ui/setup/hotkey.rs` | Short-lived hotkey helper processes and their protocol |

## Listener

`ui::listener` accepts the existing `core::listener::RecordingConfig` or `ListenerConfig`.
It creates the native platform, recorder, orchestrator, and delivery or dataset-capture output.
The event-loop handler owns these resources and executes the existing state machine's effects.
`AppEvent` carries platform events, audio readiness, history updates, and session completion.

The UI directly uses the existing business interfaces. Recorder rollback, ordered tail-chunk
submission, background finalization, and cancellation remain in the same call chain. The state
machine and orchestrator do not depend on UI or winit types. `input` and `platform` continue to
own hotkey mapping, tray components, clipboard operations, and native text injection.

## Setup

The wizard stays together with its existing `SetupUi` and `SetupVerifier` boundaries and test
doubles. `NativeVerifier` handles one complete hotkey-driven recording and verifies it through
the existing audio, STT, and post-processing interfaces. `core::config` owns configuration
loading and atomic persistence; the wizard selects when to load, retry, cancel, or save.

The process entry points check for helper mode before tracing and CLI initialization. Normal
listener startup and the explicit setup command use the same wizard. CLI-only workflows run
without creating a desktop event loop.
