# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust 2024 desktop utility named "viberwhisper". It runs as a background voice-to-text input app with global hotkeys, tray/overlay UI, chunked transcription, optional LLM cleanup, and cross-platform text injection.

### Project Background

ViberWhisper is a local-first voice-to-text typing tool. The app lets the user trigger recording from anywhere, transcribe speech through an OpenAI-compatible STT API, optionally clean up the result with an LLM, and inject the final text into the active application.

### Platform

This is a **cross-platform (macOS + Windows)** project:
- **macOS**: Text injection via osascript + clipboard (requires Accessibility permission)
- **Windows**: Text injection via Win32 SendInput API
- **Desktop UI**: System tray integration plus floating overlay window support
- **Packaging**: GitHub Actions build CI plus release packaging for macOS and Windows

### Core Functionality

1. **Dual-mode Voice Recording**: Hold F8 (hold mode) or toggle F9 (toggle mode) to record
2. **Voice Recognition**: Convert audio to text via OpenAI-compatible STT API, with Groq support in the transcriber layer
3. **Long Audio Chunking**: Automatically splits long recordings into chunks for parallel transcription
4. **Session Orchestrator**: Background transcription with convergence timeout and partial failure handling
5. **LLM Post-processing**: Optional text cleanup via LLM (punctuation, filler removal, interruption cleanup)
6. **Text Injection**: Output recognized text at the current cursor position on macOS and Windows
7. **System Tray and Overlay UI**: Status indicator (idle/recording) with tray menu and floating overlay window support
8. **CLI Utilities**: Config management and offline WAV transcription commands
9. **Packaging and Release Automation**: CI workflows plus app bundle / installer release support

### User Flow

1. User focuses any text input field
2. **Hold mode**: Hold F8 to record, release to stop
3. **Toggle mode**: Press F9 to start, press again to stop
4. The app records audio and processes it in the background
5. The final text is injected into the active input field

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
    overlay/                 — Floating overlay abstractions and platform implementations
  platform/
    macos.rs                 — MacTyper (osascript + clipboard)
    windows.rs               — WindowsTyper (SendInput API)
  transcriber/
    api.rs                   — API-backed transcriber implementation
    factory.rs               — create_transcriber factory function
    mod.rs                   — Transcriber traits and exports
  postprocess/
    mod.rs                   — TextPostProcessor/TextPostProcessorSession traits, NoopPostProcessor
    llm.rs                   — LlmPostProcessor with conservative and preheat sessions
    factory.rs               — create_post_processor factory function
docs/
  architecture/              — Module-level design docs
  plan/                      — Feature implementation plans
.github/workflows/           — CI, release, and PR automation workflows
assets/                      — App icons and bundle metadata
Cargo.toml                   — Project configuration and dependencies
config.example.json          — Example configuration template
changelog                    — Project changelog
```

## Development Principles

### 1. Feature Implementation Workflow

When implementing any feature, **strictly follow this order**:

1. Read the feature documentation in `./docs/plan/` directory (if exists)
2. Write an implementation plan (design doc with tech choices, file structure, implementation steps)
3. **Submit a PR with the plan only** — push the review bookmark/branch and notify the user to review
4. **Wait for user approval** before writing any code
5. Implement the feature according to the approved plan
6. Append code changes to the same review PR/bookmark flow (do NOT create a separate PR)
7. Update the corresponding feature doc and `changelog` to reflect the actual implementation

**Never skip the plan review step.** The user must approve the plan before any code is written.

### 2. Test-Driven Development (TDD)

This project follows TDD practices. Always:
1. Write tests first
2. Then implement the feature to make tests pass

### 3. Version Control Workflow (jj)

This repository uses **Jujutsu (`jj`)** for day-to-day development. Do not treat Git branches as the primary local workflow. Use `jj` changes plus bookmarks, then push bookmarks to GitHub when the work is ready for review.

Rules:
- Use `jj` as the default interface for local development history.
- Use Git branches only as remote review artifacts created from `jj` bookmarks.
- Do not start feature work with `git checkout -b`.
- Do not use `git commit` for normal development in this repository.
- Do not use `git push origin HEAD` or other ad hoc Git push flows for review branches.
- Every reviewable change must have a non-empty `jj` description before push.
- Every reviewable change must be pushed via a named `jj` bookmark.
- Reuse the same bookmark for the same review thread instead of creating multiple branch names for one change.

Recommended flow from local edits to pushing a review branch:

1. **Start from the latest remote state**
   - Run `jj git fetch --remote origin`
   - Confirm the current base with `jj status` or `jj log -r 'master|@'`
   - If the working copy should be based on `master`, make sure `@-` or the parent change is the latest `master`

2. **Implement the change in the working copy**
   - Edit files directly in the current working-copy change `@`
   - Use `jj status` to verify touched files
   - Use `jj diff` to review the exact patch before moving on

3. **Describe the change clearly**
   - Set the change description with `jj describe -m "type(scope): concise summary"`
   - Descriptions should be meaningful before code review or push
   - Do not leave a reviewable change as `(no description set)`

4. **Run validation before sharing**
   - Run the relevant checks such as `cargo test`, `cargo check`, `cargo clippy`, or targeted tests
   - Re-check the final diff with `jj diff`
   - If the change is split across logical steps, create additional child changes with `jj new`

5. **Create or update the review bookmark**
   - Attach a bookmark to the current change with `jj bookmark set <bookmark-name> -r @`
   - Use a stable, review-friendly name such as `feat/windows-overlay`, `fix/hotkey-timeout`, or `docs/readme-refresh`
   - Reuse the same bookmark while iterating on the same review

6. **Push the bookmark to GitHub**
   - Push with `jj git push --remote origin --bookmark <bookmark-name>`
   - This creates or updates the corresponding remote branch for GitHub PRs
   - If push safety checks fail, fetch again first with `jj git fetch --remote origin`

7. **Continue iteration without losing history**
   - Keep making edits in `@`, then update the description if needed
   - Re-push the same bookmark with `jj git push --remote origin --bookmark <bookmark-name>`
   - When one change is finished and a new one should start, run `jj new` to create the next working change

8. **After merge or when syncing with upstream**
   - Fetch again with `jj git fetch --remote origin`
   - Move local work onto the new base if needed before continuing
   - Keep bookmarks aligned with the active review change; do not accumulate stale review bookmarks unnecessarily
