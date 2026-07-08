# Platform Module Architecture

## Purpose

The `platform` module (`src/platform/`) provides platform-specific implementations of the `TextTyper` trait from `src/input/typer.rs`. The correct implementation is selected at compile time via `#[cfg(target_os)]`.

---

## macOS: `MacTyper` (`src/platform/macos.rs`)

```rust
pub struct MacTyper;
```

### `type_text` Implementation

Uses the clipboard-paste approach to avoid osascript keystroke length limits and special character issues:

1. Sleeps 100 ms to let the target window regain focus.
2. Captures the current text clipboard via `pbpaste` (best effort; non-text content such as images cannot be captured).
3. Sets the clipboard to the transcribed text via `pbcopy` (no AppleScript escaping needed).
4. Simulates `Cmd+V` via `osascript` / `System Events`; on failure the previous clipboard is restored before returning the error.
5. If a previous text clipboard was captured, waits 300 ms for the target app to read the pasteboard, then restores it.

**Requirements:** macOS Accessibility permission must be granted to the running process in System Preferences → Privacy & Security → Accessibility.

---

## Windows: `WindowsTyper` (`src/platform/windows.rs`)

```rust
pub struct WindowsTyper;
```

### `type_text` Implementation

Uses the Win32 `SendInput` API to inject Unicode keystrokes directly:

1. Sleeps 100 ms to let the target window regain focus.
2. Encodes the text as UTF-16 code units.
3. Creates paired `INPUT` structs (keydown + keyup) for each code unit using `KEYEVENTF_UNICODE`.
4. Calls `SendInput` and verifies all events were sent.

**FFI:** The `ffi` submodule defines the `INPUT`, `KEYBDINPUT`, and `INPUT_UNION` C structs, links against `user32.dll`, and declares `SendInput` as `unsafe extern "system"`.

---

## Selecting an Implementation

In `src/main.rs`, the typer is selected conditionally:

```rust
#[cfg(target_os = "macos")]
let typer = MacTyper;

#[cfg(target_os = "windows")]
let typer = WindowsTyper;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
let typer = MockTyper;
```

All three types implement `TextTyper`, so `main.rs` calls `typer.type_text(text)` uniformly.
