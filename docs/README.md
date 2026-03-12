# ViberWhisper Documentation

## Architecture

Module-level design docs covering structs, methods, and dependencies.

| Document | Description |
|---|---|
| [audio.md](architecture/audio.md) | Audio recording — `AudioRecorder`, cpal stream management, WAV output |
| [core.md](architecture/core.md) | Config persistence (`AppConfig`) and CLI argument parsing (`Cli`, `Commands`) |
| [input.md](architecture/input.md) | Hotkey detection (`HotkeyManager`), text injection (`TextTyper`), system tray (`TrayManager`) |
| [transcriber.md](architecture/transcriber.md) | Transcription trait, `GroqTranscriber` (Groq API), `MockTranscriber` |
| [platform.md](architecture/platform.md) | Platform text injection — `MacTyper` (osascript) and `WindowsTyper` (SendInput) |

## Feature Plans

Implementation plans and technical specs for each feature.

### Hotkey Recording
- [01-global-hotkey-recording.md](features/hotkey-recording/01-global-hotkey-recording.md) — Global hotkey (F8) triggered audio recording with WAV output

### Toggle Recording
- [toggle_recording_plan.md](features/toggle-recording/toggle_recording_plan.md) — Dual-mode recording: hold-to-record (F8) and toggle (F9)

### Cross-Platform
- [cross_platform_plan.md](features/cross-platform/cross_platform_plan.md) — macOS + Windows support via platform-specific `TextTyper` implementations
