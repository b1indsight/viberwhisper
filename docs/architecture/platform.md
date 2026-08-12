# Platform Module Architecture

## Purpose

The `platform` module provides platform-specific text injection and the platform-specific application configuration directory. Implementations are selected at compile time via `#[cfg(target_os)]`.

## Configuration directory

`platform::config_dir()` delegates to the current OS module. macOS appends `com.b1indsight.viberwhisper`; Windows appends `ViberWhisper`. `core::config::ConfigStore` alone appends `config.json`, so platform code does not depend on config schema or errors.

---

## macOS: `MacTyper` (`src/platform/macos.rs`)

```rust
pub struct MacTyper {
    paste: NativePasteWriter,
    delivery: Mutex<()>,
}
```

### `type_text` Implementation

`MacTyper` serializes non-empty deliveries and sleeps 100 ms so the target window can regain focus.
It then enters an Objective-C autorelease pool and uses two native routes:

1. `macos/accessibility.rs` resolves the system-wide focused AX element, rejects
   `AXSecureTextField`, checks whether `AXSelectedText` is settable, and assigns the transcription
   to that selected-text attribute. An empty selection inserts at the caret; a non-empty selection
   is replaced. This success path does not read the clipboard or emit keyboard events.
2. A present, non-secure control that explicitly lacks settable selected text uses the native paste
   fallback. Missing Accessibility trust, no focused element, secure controls, invalid AX objects,
   type mismatches, and messaging failures are hard errors and do not paste into an unknown target.

The implementation never reads or writes `AXValue`, does not construct AppleScript, and does not
launch a subprocess. Text is carried as `CFString`/`NSString`, so multiline content, quotes,
backslashes, CJK, and emoji require no shell escaping.

### Native paste fallback

`macos/pasteboard.rs` clears the general pasteboard and writes the transcription as
`NSPasteboardTypeString`. It intentionally leaves that text on the clipboard after delivery; the
fallback does not read, snapshot, or restore the previous contents. This avoids materializing
arbitrary pasteboard representations in process memory and avoids racing a later clipboard owner.

The same module creates one HID-system `CGEventSource` and posts left Command down, V down, V up,
and left Command up.

`NativePasteWriter` owns an atomic flag and returns a listener filter closure together with the
writer. A private RAII scope raises the flag across CoreGraphics posting and a fixed 100 ms
synthetic-event filter grace; the closure returns `None` during that scope, which keeps
ViberWhisper from treating its own Cmd+V
as configured hotkeys. The generic listener resets mapper key-down state whenever a callback drops
an event. Outside the paste scope, `macos/hotkey.rs` normalizes modifier direction through
`CGEventSourceKeyState` and the callback returns the normalized event. No acknowledgement or
sequence-matching protocol is involved. The grace only gives the asynchronous `rdev` callback time
to observe generated events; it is not a clipboard-consumption or delivery acknowledgement.

AppKit rejection of the transcription write and failure to construct the CoreGraphics event source
or events are returned as paste errors. If event construction fails after the write, the
transcription still remains on the clipboard.

**Requirements:** grant Accessibility permission to the running terminal or ViberWhisper in System
Settings → Privacy & Security → Accessibility. Missing permission is a hard input error and leaves
the clipboard untouched.

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

In `src/application/listener.rs`, the typer is selected conditionally:

```rust
#[cfg(target_os = "macos")]
let (typer, hotkey_filter) = MacTyper::new();

start_hotkey_listener(&config.hotkeys, hotkey_filter, notify);

#[cfg(target_os = "macos")]
let typer = Arc::new(typer);

#[cfg(target_os = "windows")]
let typer = WindowsTyper;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
let typer = MockTyper;
```

All three types implement `TextTyper`, so the listener calls `typer.type_text(text)` uniformly.
Only macOS receives a stateful filter callback from its paste writer; other platforms pass events
through with `Some`, and Windows `SendInput` behavior is unchanged.
