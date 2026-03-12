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
2. Escapes backslashes and double quotes in the text.
3. Constructs an AppleScript that:
   - Sets the clipboard to the escaped text.
   - Simulates `Cmd+V` via `System Events`.
4. Runs the script via `osascript -e`.
5. Returns an error if the process exits non-zero.

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
