# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust project named "viberwhisper" using the 2024 edition.

### Project Background

ViberWhisper is a local/web-based recreation of [Typeless](https://typeless.ai/) - a voice-to-text input tool. The goal is to enable fast, hands-free text input anywhere via voice.

### Platform

This is a **Windows-only** project that uses Windows Desktop APIs for system integration (global hotkeys, simulated keyboard input, audio capture).

### Core Functionality

The application provides these fundamental features:

1. **Hotkey-triggered Voice Input**: Hold a configured keyboard shortcut to start voice recording
2. **Voice Recognition**: Convert spoken audio to text using speech recognition
3. **Cursor Integration**: Output the recognized text at the current cursor position in any active input field

### User Flow

1. User focuses any text input field (browser, editor, chat app, etc.)
2. User holds the configured hotkey (e.g., Ctrl+Shift+Space) to start recording
3. User speaks their message
4. User releases the hotkey to stop recording
5. Recognized text is automatically typed at the cursor position

## Common Commands

```bash
# Build the project
cargo build

# Build for release
cargo build --release

# Run the project
cargo run

# Run tests
cargo test

# Run a specific test
cargo test <test_name>

# Check for linting errors
cargo clippy

# Format code
cargo fmt
```

## Project Structure

- `src/main.rs` - Main entry point
- `Cargo.toml` - Project configuration and dependencies
- `doc/` - Feature documentation directory
- `changelog` - Project changelog file

## Development Principles

### 1. Feature Documentation Workflow

When implementing any feature:
1. First, read the feature documentation in `./doc` directory
2. After implementation is complete, update the corresponding feature doc to reflect the actual implementation
3. Add a line to the `changelog` file in the simplest language describing the updated feature

### 2. Test-Driven Development (TDD)

This project follows TDD practices. Always:
1. Write tests first
2. Then implement the feature to make tests pass

## Development Phases

### Phase 1: MVP (Current)

Implement a minimal working version with:
- Global hotkey registration (e.g., Ctrl+Shift+Space)
- Basic audio recording while hotkey is held
- Speech-to-text conversion
- Simulated keyboard output to active window

Focus on core functionality over configuration or polish.
