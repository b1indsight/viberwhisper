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

### Recording Input Normalization

Hotkey and tray source details stop at the listener integration boundary in `main.rs`. The integration reads the session machine's current state without mutating it and publishes only source-free core requests:

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
- Spawns the listener thread without blocking application startup.

**`check_event(&self) -> Option<HotkeyEvent>`**

Non-blockingly receives the oldest pending event. Called from the main loop on each iteration; later events remain queued in their original order.

**`parse_key(s: &str) -> Option<Key>`**

Maps trimmed key name strings (`"F1"`–`"F12"`, case-insensitive) to `rdev::Key` variants.

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

| State | Color | Tooltip |
|---|---|---|
| Idle | Gray `(128, 128, 128)` | `"ViberWhisper - 空闲"` |
| Recording | Red `(220, 50, 50)` | `"ViberWhisper - 录音中"` |

Icons are 32×32 RGBA bitmaps generated at runtime as filled circles.

### Key Methods

**`TrayManager::new() -> Result<Self>`**

Builds the tray icon with a menu containing: title item, status item, separator, and exit item. Left-click menu opening is disabled so left click can toggle recording; right click retains the native context menu.

**`set_recording(&mut self, recording: bool)`**

Switches the icon, tooltip, and menu status text based on recording state. Native icon/tooltip update failures are logged rather than silently discarded.

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
