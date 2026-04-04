# ViberWhisper Documentation

## Architecture

Module-level design docs covering structs, methods, and dependencies.

| Document | Description |
|---|---|
| [audio.md](architecture/audio.md) | Audio recording — `AudioRecorder`, cpal stream management, live chunking, WAV output |
| [core.md](architecture/core.md) | Config persistence (`AppConfig`), CLI argument parsing (`Cli`, `Commands`), `SessionOrchestrator` |
| [input.md](architecture/input.md) | Hotkey detection (`HotkeyManager`), text injection (`TextTyper`), system tray (`TrayManager`) |
| [transcriber.md](architecture/transcriber.md) | Transcription trait, `ApiTranscriber` (OpenAI-compatible API), chunking, retry, text merging |
| [platform.md](architecture/platform.md) | Platform text injection — `MacTyper` (osascript) and `WindowsTyper` (SendInput) |
| [postprocess.md](architecture/postprocess.md) | Post-processing — `TextPostProcessor` trait, LLM integration, preheat/conservative sessions |

## Examples

Tracked example files for local setup.

| File | Description |
|---|---|
| [../config.example.json](../config.example.json) | Example local config; copy to `config.json` and fill your own API key |

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
| [09-floating-window.md](plan/09-floating-window.md) | Done | Floating overlay window: draggable always-on-top mic overlay with click-to-toggle |
| [10-objc2-overlay-migration.md](plan/10-objc2-overlay-migration.md) | Planned | Migrate the macOS overlay from deprecated `cocoa` / `objc` to the `objc2` ecosystem |
