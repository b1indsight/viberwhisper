# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust project named "viberwhisper" using the 2024 edition.

### Project Background

ViberWhisper is a local voice-to-text input tool. The goal is to enable fast, hands-free text input anywhere via voice.

### Platform

This is a **cross-platform (macOS + Windows)** project:
- **macOS**: Text injection via osascript + clipboard (requires Accessibility permission)
- **Windows**: Text injection via Win32 SendInput API

### Core Functionality

1. **Dual-mode Voice Recording**: Hold F8 (hold mode) or toggle F9 (toggle mode) to record
2. **Voice Recognition**: Convert audio to text via OpenAI-compatible STT API (default: Groq Whisper)
3. **Long Audio Chunking**: Automatically splits long recordings into chunks for parallel transcription
4. **Session Orchestrator**: Background transcription with convergence timeout and partial failure handling
5. **LLM Post-processing**: Optional text cleanup via LLM (punctuation, filler removal, interruption cleanup)
6. **Cursor Integration**: Output recognized text at the current cursor position
7. **System Tray**: Status indicator (idle/recording) with menu

### User Flow

1. User focuses any text input field
2. **Hold mode**: Hold F8 to record, release to stop
3. **Toggle mode**: Press F9 to start, press again to stop
4. Audio is transcribed (chunked if long), optionally post-processed by LLM
5. Final text is typed at the cursor position

## Common Commands

```bash
cargo build            # Build the project
cargo build --release  # Build for release
cargo run              # Run the project
cargo test             # Run tests
cargo test <test_name> # Run a specific test
cargo clippy           # Check for linting errors
cargo fmt              # Format code
```

### Packaging Commands

```bash
# macOS
cargo install cargo-bundle    # Install bundler (one-time)
cargo bundle --release        # Create .app bundle
hdiutil create -volname ViberWhisper -srcfolder target/release/bundle/osx/ViberWhisper.app -ov -format UDZO ViberWhisper.dmg  # Create DMG

# Windows
cargo install cargo-wix       # Install WiX tooling (one-time)
cargo wix init                # Generate wix/main.wxs template (one-time)
cargo wix                     # Build .msi installer

# Release
git tag v0.2.0 && git push origin v0.2.0  # Trigger CI release
```


## Project Structure

```
src/
  main.rs                    — Entry point, hotkey event loop, CLI dispatch
  core/
    config.rs                — AppConfig with all configuration fields
    cli.rs                   — Clap-based CLI (config, convert subcommands)
    orchestrator.rs          — SessionOrchestrator for session lifecycle
  audio/
    recorder.rs              — AudioRecorder with cpal stream and live chunking
    splitter.rs              — WAV file splitting utilities
  input/
    hotkey.rs                — HotkeyManager with rdev
    typer.rs                 — TextTyper trait + MockTyper
    tray.rs                  — TrayManager for system tray icon
  platform/
    macos.rs                 — MacTyper (osascript + clipboard)
    windows.rs               — WindowsTyper (SendInput API)
  transcriber/
    api.rs                   — Transcriber trait, ApiTranscriber, MockTranscriber
    factory.rs               — create_transcriber factory function
  postprocess/
    mod.rs                   — TextPostProcessor/TextPostProcessorSession traits, NoopPostProcessor
    llm.rs                   — LlmPostProcessor with conservative and preheat sessions
    factory.rs               — create_post_processor factory function
docs/
  architecture/              — Module-level design docs
  plan/                      — Feature implementation plans
Cargo.toml                   — Project configuration and dependencies
config.example.json          — Example configuration template
changelog                    — Project changelog
```

## Development Principles

### 1. Feature Implementation Workflow

When implementing any feature, **strictly follow this order**:

1. Read the feature documentation in `./docs/plan/` directory (if exists)
2. Write an implementation plan (design doc with tech choices, file structure, implementation steps)
3. **Submit a PR with the plan only** — notify the user to review
4. **Wait for user approval** before writing any code
5. Implement the feature according to the approved plan
6. Append code commits to the same PR (do NOT create a separate PR)
7. Update the corresponding feature doc and `changelog` to reflect the actual implementation

**Never skip the plan review step.** The user must approve the plan before any code is written.

### 2. Test-Driven Development (TDD)

This project follows TDD practices. Always:
1. Write tests first
2. Then implement the feature to make tests pass

## Implemented Features

- Global hotkey recording with dual modes (Hold F8 / Toggle F9)
- Cross-platform support (macOS + Windows)
- OpenAI-compatible STT API integration with configurable endpoint
- Automatic long audio chunking with parallel background transcription
- Session orchestrator with convergence timeout and partial failure handling
- Optional LLM text post-processing (preheat + conservative modes)
- System tray with recording status indicator
- CLI config management and offline WAV transcription
- Floating overlay window with click-to-toggle recording (macOS)
