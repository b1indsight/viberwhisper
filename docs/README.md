# ViberWhisper Documentation

## Architecture

Module-level design docs covering structs, methods, and dependencies.

| Document | Description |
|---|---|
| [audio.md](architecture/audio.md) | Audio recording — `AudioRecorder`, cpal stream management, WAV output |
| [core.md](architecture/core.md) | Config persistence (`AppConfig`) and CLI argument parsing (`Cli`, `Commands`) |
| [input.md](architecture/input.md) | Hotkey detection (`HotkeyManager`), text injection (`TextTyper`), system tray (`TrayManager`) |
| [transcriber.md](architecture/transcriber.md) | Transcription trait, `ApiTranscriber` (OpenAI-compatible API), `MockTranscriber` |
| [platform.md](architecture/platform.md) | Platform text injection — `MacTyper` (osascript) and `WindowsTyper` (SendInput) |

## Examples

Tracked example files for local setup.

| File | Description |
|---|---|
| [../config.example.json](../config.example.json) | Example local config; copy to `config.json` and fill your own API key |

## Feature Plans

Implementation plans and technical specs for each feature.

| Document | Description |
|---|---|
| [01-hotkey-recording.md](plan/01-hotkey-recording.md) | Global hotkey (F8) triggered audio recording with WAV output |
| [02-toggle-recording.md](plan/02-toggle-recording.md) | Dual-mode recording: hold-to-record (F8) and toggle (F9) |
| [03-cross-platform.md](plan/03-cross-platform.md) | macOS + Windows support via platform-specific `TextTyper` implementations |
| [04-multiple-models.md](plan/04-multiple-models.md) | Provider + model config abstraction for future multi-provider expansion |
