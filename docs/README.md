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
| [input.md](architecture/input.md) | Target-neutral hotkey/tray drivers and the thread-safe text-delivery contract |
| [local.md](architecture/local.md) | Local Gemma runtime: installer, Python FastAPI service, process lifecycle, health/status management |
| [transcriber.md](architecture/transcriber.md) | Transcription trait, `ApiTranscriber` (OpenAI-compatible API), chunking, retry, text merging |
| [platform.md](architecture/platform.md) | Compile-time desktop interface for native input, status/history menus, config paths, text delivery, and clipboard copy |
| [postprocess.md](architecture/postprocess.md) | Post-processing — concrete processor facade, LLM integration, preheat/conservative sessions |
| [prompt-lab.md](architecture/prompt-lab.md) | STT prompt dataset capture, deterministic local metrics, agent review, and JSON report lifecycle |

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
| [13-full-hotkey-support.md](plan/13-full-hotkey-support.md) | Implemented | Expand Hold/Toggle configuration to named single keys, including standalone right Alt/Option |
| [14-tray-recording-control.md](plan/14-tray-recording-control.md) | Done | Tray-only control, input-independent recording state, and strict SessionId routing |
| [16-session-owned-chunk-results.md](plan/16-session-owned-chunk-results.md) | Done | Keep chunk state and transcription results owned by one session; workers return events instead of mutating shared chunk storage |
| [17-shared-text-merge.md](plan/17-shared-text-merge.md) | Done | Centralize transcription text merging for offline chunks and session orchestration |
| [18-config-architecture-refactor.md](plan/18-config-architecture-refactor.md) | Done | Strict v2-only config, module-owned validation, minimal runtime views, and explicit API/Local profiles |
| [24-rust-2018-module-layout.md](plan/24-rust-2018-module-layout.md) | Done | Replace every `mod.rs` module entry with the Rust 2018-style sibling module file layout |
| [25-test-suite-pruning.md](plan/25-test-suite-pruning.md) | Done | Remove redundant tests and make retained coverage deterministic, fast, and proportional to risk |
| [26-symbol-icon-refresh.md](plan/26-symbol-icon-refresh.md) | Done | Replace placeholder bundle artwork and generated tray dots with one cross-platform voice-input symbol |
| [27-release-path-hardening.md](plan/27-release-path-hardening.md) | Implemented | Make API-mode macOS and Windows packages reproducible, manually testable, and safe to publish |
| [28-winit-event-loop.md](plan/28-winit-event-loop.md) | Implemented | Replace fixed listener polling with a main-thread winit event loop and non-blocking finalization |
| [29-native-macos-text-injection.md](plan/29-native-macos-text-injection.md) | Implemented | Replace macOS osascript injection with direct AX insertion and a clipboard-replacing native paste fallback |
| [30-compile-time-platform-interface.md](plan/30-compile-time-platform-interface.md) | Implemented | Hide native icon, hotkey, and text-delivery details behind one compile-time-selected platform interface |
| [31-macos-chromium-paste-fallback.md](plan/31-macos-chromium-paste-fallback.md) | Implemented | Route identified Chromium browsers through recoverable native paste without activating web accessibility |
| [32-transcription-history.md](plan/32-transcription-history.md) | In progress | Persist a bounded transcription history and expose the newest five entries as exact clipboard-copy actions in the tray menu |
| [33-stt-prompt-regression-suite.md](plan/33-stt-prompt-regression-suite.md) | Implemented | Capture corrected audio datasets and rerun STT prompts with local metrics plus agent-reviewed LLM scoring in JSON |
| [34-prompt-lab-scoring-hardening.md](plan/34-prompt-lab-scoring-hardening.md) | Implemented | Preserve report validation across JSON float round trips and score Latin proper nouns next to Han text |
| [35-silent-audio-hallucination-suppression.md](plan/35-silent-audio-hallucination-suppression.md) | Implemented | Suppress effectively silent WAV chunks before STT upload and preserve empty no-output behavior |
| [36-ci-platform-quality-gates.md](plan/36-ci-platform-quality-gates.md) | Implemented | Enforce formatting and Windows-specific Clippy in normal hosted CI |
| [37-github-actions-node24.md](plan/37-github-actions-node24.md) | Implemented | Upgrade workflow Actions to Node 24-compatible, full-SHA-pinned releases with Dependabot maintenance |
| [38-release-environment-approval.md](plan/38-release-environment-approval.md) | Implemented | Pause tagged publication behind a protected GitHub Environment approval |
| [39-release-notes-automation.md](plan/39-release-notes-automation.md) | Implemented | Prepend validated distribution guidance to automatically generated Release Notes |
| [40-windows-gui-launcher.md](plan/40-windows-gui-launcher.md) | Implemented | Launch the Windows tray application without a console while preserving the existing CLI executable |
| [42-structured-runtime-logging.md](plan/42-structured-runtime-logging.md) | Proposed | Route runtime diagnostics through structured tracing while preserving intentional CLI stdout |
