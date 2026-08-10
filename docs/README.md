# ViberWhisper Documentation

## Maintainer Guides

| Document | Description |
|---|---|
| [releasing.md](releasing.md) | Prepare, dry-run, publish, verify, and recover macOS/Windows releases |

## Architecture

Module-level design docs covering structs, methods, and dependencies.

| Document | Description |
|---|---|
| [audio.md](architecture/audio.md) | Audio recording — `AudioRecorder`, cpal stream management, live chunking, WAV output |
| [core.md](architecture/core.md) | Strict v2 config persistence, runtime assembly, CLI parsing, and `SessionOrchestrator` |
| [input.md](architecture/input.md) | Hotkey detection (`HotkeyManager`), text injection (`TextTyper`), system tray (`TrayManager`) |
| [local.md](architecture/local.md) | Local Gemma runtime: installer, Python FastAPI service, process lifecycle, health/status management |
| [transcriber.md](architecture/transcriber.md) | Transcription trait, `ApiTranscriber` (OpenAI-compatible API), chunking, retry, text merging |
| [platform.md](architecture/platform.md) | Platform text injection — `MacTyper` (osascript) and `WindowsTyper` (SendInput) |
| [postprocess.md](architecture/postprocess.md) | Post-processing — concrete processor facade, LLM integration, preheat/conservative sessions |

## Examples

Tracked example files for local setup.

| File | Description |
|---|---|
| [../config.example.json](../config.example.json) | Canonical secret-free v2 configuration example |

## Feature Plans

Implementation plans and technical specs for each feature.

| Document | Status | Description |
|---|---|---|
| [01-hotkey-recording.md](plan/01-hotkey-recording.md) | Done | Global hotkey (F8) triggered audio recording with WAV output |
| [02-toggle-recording.md](plan/02-toggle-recording.md) | Done | Dual-mode recording: hold-to-record (F8) and toggle (F9) |
| [03-cross-platform.md](plan/03-cross-platform.md) | Done | macOS + Windows support via platform-specific `TextTyper` implementations |
| [04-multiple-models.md](plan/04-multiple-models.md) | Done | Provider + model config abstraction (evolved to URL-based config) |
| [05-long-audio-streaming.md](plan/05-long-audio-streaming.md) | Done | Long audio chunking, offline split, retry with exponential backoff, and text merge |
| [06-end-to-end-stream-recognition.md](plan/06-end-to-end-stream-recognition.md) | Done | Session orchestrator: unified Hold/Toggle lifecycle, chunk state machine, convergence |
| [08-llm-post-processing.md](plan/08-llm-post-processing.md) | Done | LLM text post-processing: punctuation, filler removal, preheat/conservative modes |
| [09-floating-window.md](plan/09-floating-window.md) | Superseded | Historical cross-platform floating overlay design, removed by plan 14 |
| [10-objc2-overlay-migration.md](plan/10-objc2-overlay-migration.md) | Historical | Historical macOS overlay binding migration |
| [11-packaging-and-ci.md](plan/11-packaging-and-ci.md) | Superseded | Original cross-platform CI and unexercised tag-release design; hardened by plan 27 |
| [12-local-gemma-service.md](plan/12-local-gemma-service.md) | Done | Local Gemma inference service, lifecycle management, and CLI integration |
| [14-tray-recording-control.md](plan/14-tray-recording-control.md) | Done | Tray-only control, input-independent recording state, and strict SessionId routing |
| [16-session-owned-chunk-results.md](plan/16-session-owned-chunk-results.md) | Done | Keep chunk state and transcription results owned by one session; workers return events instead of mutating shared chunk storage |
| [17-shared-text-merge.md](plan/17-shared-text-merge.md) | Done | Centralize transcription text merging for offline chunks and session orchestration |
| [18-config-architecture-refactor.md](plan/18-config-architecture-refactor.md) | Done | Strict v2-only config, module-owned validation, minimal runtime views, and explicit API/Local profiles |
| [24-rust-2018-module-layout.md](plan/24-rust-2018-module-layout.md) | Done | Replace every `mod.rs` module entry with the Rust 2018-style sibling module file layout |
| [25-test-suite-pruning.md](plan/25-test-suite-pruning.md) | Done | Remove redundant tests and make retained coverage deterministic, fast, and proportional to risk |
| [26-symbol-icon-refresh.md](plan/26-symbol-icon-refresh.md) | Done | Replace placeholder bundle artwork and generated tray dots with one cross-platform voice-input symbol |
| [27-release-path-hardening.md](plan/27-release-path-hardening.md) | Implemented | Make API-mode macOS and Windows packages reproducible, manually testable, and safe to publish |
