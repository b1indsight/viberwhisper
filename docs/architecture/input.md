# Input Module Architecture

## Purpose

The `input` module (`src/input/`) handles three concerns: global hotkey detection (`hotkey.rs`), text injection into the focused window (`typer.rs`), and system tray UI (`tray.rs`). Input adapters report user intent; recording lifecycle decisions belong to `core::recording_session`.

---

## Hotkey (`src/input/hotkey.rs`)

### Types

**`HotkeySource`**

```rust
pub enum HotkeySource { Hold, Toggle }
```

Identifies which configured hotkey triggered an event.

**`HotkeyEvent`**

```rust
pub enum HotkeyEvent {
    Pressed(HotkeySource),
    Released(HotkeySource),
}
```

Note: `Released` is only emitted for `Hold` source (toggle mode uses press-only semantics).

**`HotkeyManager`**

```rust
pub struct HotkeyManager {
    events: Receiver<HotkeyEvent>,
}
```

Spawns an `rdev::listen` thread that maps native events into an ordered channel. Per-binding key-down state suppresses operating-system key-repeat events so one physical toggle press produces one toggle action.

**`HotkeyConfig`**

The runtime boundary between persisted hotkey strings and native events. Validation resolves each
enabled string to one `rdev::Key` plus a canonical display label, rejects duplicate or
platform-unavailable bindings, and keeps empty strings disabled. Persisted strings are not
canonicalized: `config get/list` return the user's input, while listener output and logs use the
canonical runtime label.

### Recording Input Normalization

Hotkey and tray source details stop at the listener integration boundary in `application::listener`. The integration reads the session machine's current state without mutating it and publishes only source-free core requests:

| Raw gesture | Idle | Recording | Transitional/shutdown state |
|---|---|---|---|
| Hold press | `StartRequested` | ignored | ignored |
| Hold release | ignored | `StopRequested` | ignored |
| Toggle press | `StartRequested` | `StopRequested` | ignored |
| Tray left click | `StartRequested` | `StopRequested` | ignored |

Because the core event carries no source, Hold release, Toggle, and tray input can stop a current session regardless of which input started it. Input modules continue to own native classification, key-repeat suppression, and click debounce; they do not own or copy recording state.

### Key Methods

**`HotkeyManager::new(config: &HotkeyConfig) -> Self`**

- `HotkeyConfig::validate(&InputSection)` parses and validates both bindings before construction.
- Empty strings disable a binding, including both bindings for tray-only control; duplicate non-empty bindings are rejected.
- Non-function bindings log that `rdev::listen` observes rather than suppresses their native input.
- On Windows, a `LEFTCTRL` binding logs an additional warning because AltGr is reported as left Ctrl
  plus right Alt by layouts that use AltGr. Validation rejects a `LEFTCTRL`/`RIGHTALT` pair so one
  physical AltGr press cannot enqueue both configured recording actions.
- Spawns the listener thread without blocking application startup.

**`check_event(&self) -> Option<HotkeyEvent>`**

Non-blockingly receives the oldest pending event. Called from the main loop on each iteration; later events remain queued in their original order.

**`parse_key(s: &str) -> Option<Key>`**

Maps trimmed, ASCII-case-insensitive named physical keys to `rdev::Key` variants. The fixed lookup
covers `F1`–`F12`, letters, number-row keys, editing/whitespace, navigation, left/right modifiers,
locks/system keys, punctuation, and numeric-keypad keys. Explicit aliases normalize platform terms:
`ALTGR` and `RIGHTOPTION` map to canonical `RIGHTALT`/`Key::AltGr`; `ALT` and `OPTION` map to
`LEFTALT`/`Key::Alt`.

`parse_key` recognizes the shared vocabulary independently of platform. `HotkeyConfig::validate`
then returns `hotkey.unsupported` when the current `rdev 0.5.3` backend cannot emit the named
variant. On macOS this includes Caps Lock (a status change rather than a press/release pair), right
Ctrl, forward Delete/Insert/navigation-cluster keys, several lock/system keys, international
backslash, and numeric-keypad names. On Windows it includes right Meta, Function, and the numeric
keypad Enter key that `rdev` cannot distinguish from Enter. Unknown names use `hotkey.invalid`, and
aliases resolving to the same key use `hotkey.duplicate`. Windows additionally uses
`hotkey.altgr_conflict` when the two bindings are `LEFTCTRL` and `RIGHTALT` in either order.

Configuration persistence intentionally does not invoke this runtime validation. `config set`
stores the string; `config check` and listener startup call the existing `runtime_config` resolution
path and report hotkey issues together with other active-listener configuration issues.

Key names identify `rdev::Key` values rather than produced characters. The macOS backend maps
hardware key codes, while the Windows backend maps virtual-key values; letter and punctuation
bindings can therefore follow the active Windows layout rather than a fixed physical QWERTY
position. The listener is passive, so printable, editing, navigation, and modifier keys continue to
affect the focused application or operating system. Windows AltGr also emits left Ctrl at the
`rdev::Event` boundary, so a standalone `LEFTCTRL` binding may fire from AltGr; the retained event
type does not contain enough native information to filter that event reliably.

On macOS, `rdev 0.5.3` derives modifier press/release direction from aggregate modifier flags.
That direction can be wrong when the listener starts while a modifier is held or when both sides
of one modifier overlap. Before `EventMapper` sees a macOS modifier event, the listener therefore
uses Core Graphics `CGEventSourceKeyState` with the physical modifier key code to normalize it to
the current press/release state. Ordinary key events still flow directly through `rdev`, preserving
their existing repeat suppression.

---

## Typer (`src/input/typer.rs`)

### `TextTyper` Trait

```rust
pub trait TextTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}
```

Platform implementations are in `src/platform/`. See [platform.md](platform.md).

### `MockTyper`

A no-op implementation used in tests and non-GUI environments. Logs the text at `INFO` level instead of injecting it.

---

## Tray (`src/input/tray.rs`)

### `TrayManager`

```rust
pub struct TrayManager {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
    status_item: MenuItem,
    exit_item_id: MenuId,
    click_debounce: ClickDebounce,
}
```

### Icon States

| State | Windows color | macOS rendering | Tooltip |
|---|---|---|---|
| Idle | Charcoal `#34373d` | Template tint | `"ViberWhisper - 空闲"` |
| Recording | Red `#ef3340` | Explicit red | `"ViberWhisper - 录音中"` |

Both states use the same five rounded voice-wave bars whose lower endpoints form a `V`. The 32×32
RGBA PNGs are embedded in the executable and decoded during tray construction, so runtime loading
does not depend on the current directory. Bundle icons use the same geometry from
`assets/icon-source.svg` on a solid gray-blue `#282c34` rounded-square field.

macOS treats the idle icon as an AppKit template so the system selects a legible menu-bar color for
light and dark appearances. Recording switches the icon and template flag together, preserving the
asset's explicit red. Windows ignores the template flag and displays the committed colors.

### Key Methods

**`TrayManager::new() -> Result<Self>`**

Decodes the embedded idle and recording PNGs, then builds the tray icon with a menu containing:
title item, status item, separator, and exit item. Corrupt or non-RGBA assets fail tray construction
with an error, while tests enforce the committed 32×32 dimensions and transparency. Left-click menu
opening is disabled so left click can toggle recording; right click retains the native context menu.

**`set_recording(&mut self, recording: bool)`**

Switches the icon, macOS template mode, tooltip, and menu status text based on recording state.
Native icon/tooltip update failures are logged rather than silently discarded.

**`check_action(&mut self) -> Option<TrayAction>`**

Drains menu events before icon events so Exit has priority. A matching left-button-up event produces `ToggleRecording`; unrelated icon IDs and mouse phases are discarded. Events are drained as one batch using a shared handling timestamp.

**`update(&self)`**

Pumps pending AppKit or Win32 events without creating a window. On macOS tray setup also enforces Accessory activation policy. This event pump is required for right-click menu and icon event delivery.

### Click Protection

- effective debounce window is `max(300 ms, platform double-click interval)`
- macOS reads `NSEvent::doubleClickInterval()`
- Windows reads `GetDoubleClickTime()` and suppresses the button-up following a native `DoubleClick`
- the suppression is one-shot; ordinary ignored clicks do not extend the window
- debounce applies only to tray recording toggles, never Exit or hotkeys
