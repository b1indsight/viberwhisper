# Input Module Architecture

## Purpose

The `input` module (`src/input/`) handles four concerns: global hotkey detection (`hotkey.rs`), text injection into the focused window (`typer.rs`), system tray UI (`tray.rs`), and the floating overlay window (`overlay/`).

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

### Key Methods

**`HotkeyManager::new(hold_hotkey: &str, toggle_hotkey: &str) -> Result<Self>`**

- Parses both hotkey strings via `parse_key()`.
- Treats an empty string as a disabled binding and rejects any non-empty invalid binding.
- Requires at least one enabled key and rejects assigning the same key to both modes.
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

Builds the tray icon with a menu containing: title item, status item, separator, and exit item.

**`set_recording(&mut self, recording: bool)`**

Switches the icon, tooltip, and menu status text based on recording state. Native icon/tooltip update failures are logged rather than silently discarded.

**`check_exit(&self) -> bool`**

Non-blocking check of the menu event channel; returns `true` if the exit item was clicked.

---

## Overlay (`src/input/overlay/`)

### Purpose

Provides an always-on-top, draggable recording affordance separate from the tray icon. A click on the overlay acts like the toggle hotkey: start recording when idle, stop when recording.

### Platform Selection

| Target | Implementation |
|---|---|
| macOS | `overlay/macos.rs` |
| Windows | `overlay/windows_impl.rs` |
| Other | `overlay/stub.rs` |

### Public API

`main.rs` interacts with the overlay through a platform-specific `OverlayManager` with a shared interface:

- `OverlayManager::new() -> Result<Self>`: create window/resources
- `set_recording(recording: bool)`: update visual state
- `check_click() -> bool`: poll whether the overlay was clicked since last check
- `update()`: pump any pending UI work from the main loop

### Main-loop Behavior

- overlay clicks are checked on every tick, after hotkey polling
- when clicked, the overlay follows the same start/stop flow as toggle mode
- overlay state is kept in sync with tray state during all record transitions
