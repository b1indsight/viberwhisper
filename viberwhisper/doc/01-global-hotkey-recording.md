# Feature: Global Hotkey Recording

## Goal
Implement global hotkey (Right Alt) triggered audio recording that saves to a temp file.

## Requirements

1. **Global Hotkey Registration**
   - Hardcoded to F8 key (AltRight not supported by global-hotkey on Windows)
   - Works system-wide regardless of focused window
   - No GUI needed for MVP

2. **Audio Recording**
   - Start recording when Right Alt is pressed
   - Stop recording when Right Alt is released
   - Save recording as WAV file to current directory
   - Filename: `recording_<timestamp>.wav`

3. **Technical Details**
   - Use Windows Desktop APIs
   - Audio format: 16-bit PCM, 16kHz, mono (good for speech recognition)

## Implementation

### Dependencies
- `global-hotkey`: Cross-platform global hotkey (Windows backend uses Win32)
- `cpal`: Cross-platform audio I/O
- `hound`: WAV file writer

### Key Components

1. **HotkeyManager** (`src/hotkey.rs`): Registers and listens for global hotkey events
   - Uses `GlobalHotKeyEvent::receiver()` to receive events
   - Hardcoded to F8 key (AltRight not supported on Windows by global-hotkey)

2. **AudioRecorder** (`src/audio.rs`): Manages audio capture and file writing
   - Uses cpal for audio input
   - Saves as 16-bit PCM WAV at 16kHz mono
   - Filename format: `recording_<timestamp>.wav`

3. **Main** (`src/main.rs`): Coordinates hotkey events with recording state

### Platform
- Using `x86_64-pc-windows-gnu` toolchain (MinGW) to avoid MSVC SDK dependencies

## Testing

- `test_audio_recorder_creation`: AudioRecorder can be created
- `test_recorder_not_recording_initially`: Initial state is not recording
- `test_hotkey_manager_creation`: HotkeyManager API works (may fail in headless env)
- `test_audio_module_loads`: Integration test for audio module

## Status
- ✅ Implemented: Global hotkey registration (Right Alt)
- ✅ Implemented: Audio recording start/stop
- ✅ Implemented: WAV file output to current directory
- ✅ TDD: Tests written first, then implementation
