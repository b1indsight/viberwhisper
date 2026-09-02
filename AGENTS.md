# AGENTS.md

This file provides guidance to coding agents working with code in this repository.

## Project Overview

This is a Rust 2024 desktop utility named "viberwhisper". It runs as a background voice-to-text input app with global hotkeys, tray UI, chunked transcription, optional LLM cleanup, bounded local history, and cross-platform text injection.

### Project Background

ViberWhisper is a local-first voice-to-text typing tool. The app lets the user trigger recording from anywhere, transcribe speech through an OpenAI-compatible STT API, optionally clean up the result with an LLM, and inject the final text into the active application.

### Platform

This is a **cross-platform (macOS + Windows)** project:
- **macOS**: Native Accessibility selected-text insertion for ordinary controls, with AppKit/CoreGraphics paste for Chromium browsers and unsupported controls; paste leaves the transcription on the clipboard (requires Accessibility permission)
- **Windows**: Text injection via Win32 SendInput API
- **Desktop UI**: System tray/status-bar integration with click-to-toggle recording and recent-history copy actions
- **Packaging**: GitHub Actions build CI plus release packaging for macOS and Windows

### Core Functionality

1. **Dual-mode Voice Recording**: Hold F8 (hold mode) or toggle F9 (toggle mode) to record
2. **Voice Recognition**: Convert audio to text via OpenAI-compatible STT API, with Groq support in the transcriber layer
3. **Long Audio Chunking**: Automatically splits long recordings into chunks for parallel transcription
4. **Session Orchestrator**: Background transcription with convergence timeout and partial failure handling
5. **LLM Post-processing**: Optional text cleanup via LLM (punctuation, filler removal, interruption cleanup)
6. **Persistent Transcription History**: Append finalized text and timestamp metadata to bounded local JSONL, repair an invalid trailing record, and expose the newest five entries from the tray
7. **Text Injection**: Output recognized text at the current cursor position on macOS and Windows
8. **System Tray UI**: Status indicator (idle/recording), left-click recording toggle, and right-click history/exit menu
9. **CLI Utilities**: Config management and offline WAV transcription commands
10. **STT Prompt Regression**: Capture paired WAV/reference datasets, rerun all ready audio with a temporary prompt, and finalize JSON metrics with coding-agent semantic review
11. **Packaging and Release Automation**: CI workflows plus app bundle / installer release support

### User Flow

1. User focuses any text input field
2. **Hold mode**: Hold F8 to record, release to stop
3. **Toggle mode**: Press F9 to start, press again to stop
4. The app records audio and processes it in the background
5. The finalized text is saved to local history and injected into the active input field
6. The five newest entries are available from the right-click tray menu and copy their full text to the clipboard

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

### Version Control Commands

```bash
jj status                                 # Show current working copy changes
jj diff                                   # Review uncommitted changes
jj describe -m "feat: message"            # Set the current change description
jj new                                    # Start a fresh child change after finishing one
jj bookmark set feat/my-change -r @       # Point a bookmark at the current change
jj git fetch --remote origin              # Sync remote refs before push
jj git push --remote origin --bookmark feat/my-change  # Push the bookmark to GitHub
gh pr create --base master --head feat/my-change  # Open a GitHub PR for review
```

### Packaging Commands

```bash
# macOS
cargo install cargo-bundle    # Install bundler (one-time)
cargo bundle --release        # Create .app bundle
```

The canonical cross-platform packaging, dry-run, version/tag, publication, verification, and
recovery commands are maintained in [`docs/releasing.md`](docs/releasing.md). Do not generate
`wix/main.wxs` during a release or push a version tag before following that runbook.


## Project Structure

```
src/
  main.rs                    — Thin process entry point delegating to viberwhisper::run
  bin/
    viberwhisper-app.rs      — Feature-gated Windows GUI-subsystem desktop entry point
  lib.rs                     — Crate module root and public CLI/desktop application entry exports
  application.rs             — Logging, CLI dispatch, config/local/convert workflows
  application/
    listener.rs              — Platform-action loop, session effects, transcription delivery
    prompt_lab.rs            — Dataset capture, regression, and agent-review CLI assembly
  runtime_config.rs          — Application-level profile and consumer config assembly
  session.rs                 — Shared SessionId value type
  text.rs                    — Shared language-aware transcription text merge
  history.rs                 — Bounded JSONL transcription history persistence and tail repair
  prompt_lab.rs              — STT prompt-lab domain facade and exports
  prompt_lab/
    capture.rs               — Session WAV archive worker and raw-STT sample publication
    dataset.rs               — Versioned samples, correction, validation, and scoring identity
    metrics.rs               — Versioned WER alignment and proper-noun matching
    regression.rs            — Fresh full-dataset STT execution and canonical JSON reports
    review.rs                — Coding-agent review validation and final threshold gates
  core.rs                    — Core module entry and submodule declarations
  core/
    config.rs                — Config facade, errors, validation, and safe value types
    config/
      document.rs            — Strict v2 configuration document
      fields.rs              — Canonical field catalog
      store.rs               — Atomic configuration persistence
    cli.rs                   — Clap-based CLI (config, convert subcommands)
    orchestrator.rs          — SessionOrchestrator for session lifecycle
    recording_session.rs     — Recording lifecycle state machine and effects
  audio.rs                   — Audio config, chunk policy, and public exports
  audio/
    chunk.rs                 — In-memory WavChunk encoding and capacity policy
    recorder.rs              — AudioRecorder with cpal stream and live chunking
    wav_file.rs              — Streaming offline WAV chunk reader
  input.rs                   — Input module entry and submodule declarations
  input/
    hotkey.rs                — Target-neutral rdev listener, event mapper, and policy contract
    typer.rs                 — TextTyper trait + MockTyper
    tray.rs                  — Policy-driven TrayManager, embedded icons, and click debounce
  platform.rs                — Compile-time backend selection and common platform facade
  platform/
    backend.rs               — Private platform backend contract
    runtime.rs               — Opaque actions plus tray/text runtime ownership
    fallback.rs              — Unsupported-target development/test adapter
    macos.rs                 — macOS backend policies and serialized native text delivery
    macos/
      accessibility.rs      — Focused AX selected-text insertion and secure-control rejection
      application.rs        — Frontmost Chromium-family browser classification
      hotkey.rs             — rdev modifier normalization for the listener callback
      pasteboard.rs         — Clipboard replacement, CoreGraphics Cmd+V, and hotkey suppression
    windows.rs               — Windows backend policies and SendInput text delivery
    windows/
      clipboard.rs           — Windows Unicode clipboard adapter
  transcriber.rs             — Transcriber traits, errors, and exports
  transcriber/
    api.rs                   — API-backed transcriber implementation
  postprocess.rs             — PostProcessor facade, session traits, NoopPostProcessor
  postprocess/
    llm.rs                   — LlmPostProcessor with conservative and preheat sessions
  local.rs                   — Local runtime facade and public exports
  local/
    installer.rs             — Python environment, dependencies, and model installation
    service.rs               — Local inference service process lifecycle
docs/
  architecture/              — Module-level design docs
  plan/                      — Feature implementation plans
.github/workflows/           — CI, release, and PR automation workflows
assets/                      — App icons and bundle metadata
Cargo.toml                   — Project configuration and dependencies
config.example.json          — Example configuration template
changelog                    — Project changelog
```

## Code Changes

All code changes must be submitted through a GitHub pull request against `master`.
