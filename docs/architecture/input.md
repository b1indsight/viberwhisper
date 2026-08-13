# Input Module Architecture

## Purpose

The `input` module (`src/input/`) provides target-neutral drivers and contracts for global hotkey
detection (`hotkey.rs`), text delivery (`typer.rs`), clipboard replacement (`clipboard.rs`), and
the system tray (`tray.rs`). Native policy is supplied by the compile-time-selected `platform`
backend. Input drivers report native events; recording lifecycle decisions belong to
`core::recording_session`.

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

**`start_hotkey_listener(config, filter, notify)`**

```rust
pub(crate) fn start_hotkey_listener<P, F>(
    config: &HotkeyConfig,
    filter: F,
    notify: impl Fn(HotkeyEvent) + Send + 'static,
) where
    P: HotkeyPolicy,
    F: Fn(EventType) -> Option<EventType> + Send + 'static;
```

Starts the process-lifetime `rdev::listen` thread, maps native events, and invokes the supplied
callback. `platform::PlatformRuntime` wraps the event in an opaque `PlatformEvent` and forwards it
through winit's `EventLoopProxy`, which wakes the main-thread event loop without polling.
Per-binding key-down state suppresses operating-system key repeat so one physical Toggle press
produces one action. `P` supplies target validation/warning policy. The filter is constructed by the
same selected backend that constructs the text writer: `Some(event)` reaches `EventMapper`, while
`None` drops the event and resets its key-down bookkeeping.

**`HotkeyConfig`**

The runtime boundary between persisted hotkey strings and native events. Validation resolves each
enabled string to one `rdev::Key` plus a canonical display label, rejects duplicate or
platform-unavailable bindings, and keeps empty strings disabled. Persisted strings are not
canonicalized: `config get/list` return the user's input, while listener output and logs use the
canonical runtime label.

### Recording Input Normalization

Hotkey and tray source details stop at `platform::PlatformRuntime`. It converts opaque native
payloads into Hold/Toggle/Exit `PlatformAction` values. `application::listener` then reads the
session machine's current state without mutating it and publishes only source-free core requests:

| Raw gesture | Idle | Recording | Transitional/shutdown state |
|---|---|---|---|
| Hold press | `StartRequested` | ignored | ignored |
| Hold release | ignored | `StopRequested` | ignored |
| Toggle press | `StartRequested` | `StopRequested` | ignored |
| Tray left click | `StartRequested` | `StopRequested` | ignored |

Because the core event carries no source, Hold release, Toggle, and tray input can stop a current
session regardless of which input started it. Input drivers and platform policies own native
classification, key-repeat suppression, and click debounce; they do not own or copy recording
state.

### Key Methods

**`start_hotkey_listener(config, filter, notify)`**

- `HotkeyConfig::validate::<P>(&InputSection)` parses and validates both bindings before
  construction; `platform::validate_hotkeys` supplies the selected `P` to `runtime_config`.
- Empty strings disable a binding, including both bindings for tray-only control; duplicate non-empty bindings are rejected.
- Non-function bindings log that `rdev::listen` observes rather than suppresses their native input.
- On Windows, a `LEFTCTRL` binding logs an additional warning because AltGr is reported as left Ctrl
  plus right Alt by layouts that use AltGr. Validation rejects a `LEFTCTRL`/`RIGHTALT` pair so one
  physical AltGr press cannot enqueue both configured recording actions.
- Spawns the listener thread without blocking application startup.
- Delivers mapped press/release events directly to the supplied non-blocking callback; the listener
  thread never runs recording state transitions itself.
- Moves the backend-supplied filter closure into the listener thread. macOS constructs it together
  with `MacTyper`; Windows and the fallback use `Some` as a stateless pass-through callback.

**`parse_key(s: &str) -> Option<Key>`**

Maps trimmed, ASCII-case-insensitive named physical keys to `rdev::Key` variants. The fixed lookup
covers `F1`–`F12`, letters, number-row keys, editing/whitespace, navigation, left/right modifiers,
locks/system keys, punctuation, and numeric-keypad keys. Explicit aliases normalize platform terms:
`ALTGR` and `RIGHTOPTION` map to canonical `RIGHTALT`/`Key::AltGr`; `ALT` and `OPTION` map to
`LEFTALT`/`Key::Alt`.

`parse_key` recognizes the shared vocabulary independently of platform. The selected
`HotkeyPolicy` then returns `hotkey.unsupported` when the current `rdev 0.5.3` backend cannot emit the named
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
affect the focused application or operating system. The macOS internal injection window described
below suppresses only ViberWhisper's mapping callback, not operating-system event delivery. Windows
AltGr also emits left Ctrl at the `rdev::Event` boundary, so a standalone `LEFTCTRL` binding may
fire from AltGr; the retained event type does not contain enough native information to filter that
event reliably.

On macOS, `rdev 0.5.3` derives modifier press/release direction from aggregate modifier flags.
That direction can be wrong when the listener starts while a modifier is held or when both sides
of one modifier overlap. Before `EventMapper` sees a macOS modifier event, the listener therefore
uses Core Graphics `CGEventSourceKeyState` with the physical modifier key code to normalize it to
the current press/release state. Ordinary key events still flow directly through `rdev`, preserving
their existing repeat suppression.

### macOS Paste Filter

The Chromium route and native-control fallback post a fixed left-Command/V sequence through
CoreGraphics. Without a shared boundary, `rdev` could interpret those synthetic events as
configured `V`, `LEFTMETA`, or `RIGHTMETA` recording hotkeys.

`MacBackend` constructs `MacTyper` and the filter returned by
`src/platform/macos/pasteboard.rs` together, so their atomic suppression flag never leaves the
platform boundary. The paste transaction raises that flag immediately before
constructing and posting Cmd+V and keeps it raised through a fixed 100 ms synthetic-event filter
grace. The closure returns `None` for every event observed in that short scope, so the synthetic sequence and
any interleaved physical input still reach macOS and the focused application but do not enter
ViberWhisper's mapper. Each `None` resets Hold/Toggle key-down bookkeeping. A private RAII guard
clears the flag after success, early error, or unwinding.

Outside that scope, the callback delegates macOS modifier events to
`src/platform/macos/hotkey.rs`, which normalizes press/release direction from physical key state,
and passes ordinary events through unchanged. There is no sequence acknowledgement protocol: the
filter protects ViberWhisper's mapper, while CoreGraphics posting and the target application
determine paste delivery independently. Every paste route leaves the transcription on the
clipboard for manual recovery because event posting cannot confirm target consumption.

---

## Typer (`src/input/typer.rs`)

### `TextTyper` Trait

```rust
pub trait TextTyper: Send + Sync {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}
```

Platform implementations are in `src/platform/`. `NativePlatform::text_typer` returns the selected
thread-safe handle without exposing its concrete type. See [platform.md](platform.md).
The thread-safety contract allows final transcription delivery to run outside the native event-loop
thread.

### `MockTyper`

A no-op implementation selected by the explicit fallback backend on unsupported development/test
targets. It logs text at `INFO` instead of injecting it.

---

## Tray (`src/input/tray.rs`)

### `TrayManager`

```rust
pub struct TrayManager<P: TrayPolicy> {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
    status_item: MenuItem,
    history_items: Vec<MenuItem>,
    history: Vec<String>,
    exit_item_id: MenuId,
    click_debounce: ClickDebounce,
    policy: PhantomData<P>,
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

`MacTray` treats the idle icon as an AppKit template so the system selects a legible menu-bar color
for light and dark appearances. Recording switches the icon and template flag together, preserving
the asset's explicit red. `WindowsTray` uses the committed colors directly.

### Key Methods

**`TrayManager::<P>::new(notify) -> Result<Self>`**

Decodes the embedded idle and recording PNGs, then builds the tray icon with title, status, a
disabled `最近识别` heading, five fixed history slots, separators, and Exit.
Corrupt or non-RGBA assets fail tray construction with an error, while tests enforce the committed
32×32 dimensions and transparency. Left-click menu opening is disabled so left click can toggle
recording; right click retains the native context menu.
Before creating the native icon, `P` prepares the native application and construction installs the
process-global `tray-icon` and menu callbacks with the platform runtime's event callback.
`tray-icon 0.21` stores those handlers in one-shot
global cells, matching the application's single listener and process-lifetime event loop.

**`set_recording(&mut self, recording: bool)`**

Switches the icon, backend-selected template/color mode, tooltip, and menu status text based on
recording state. Native icon/tooltip update failures are logged rather than silently discarded.

**`set_history(&mut self, entries: Vec<String>)` / `push_history(&mut self, text: String)`**

Startup moves in up to five entries; each later append prepends one string and truncates the cache
to five. The fixed menu slots update their label and enabled state in place. Labels collapse
whitespace, escape mnemonic ampersands, and truncate after 40 Unicode characters; the parallel
`history` vector retains the unmodified text.

**`handle_event(&mut self, event: TrayEvent) -> Option<TrayAction>`**

Filters raw events by tray/menu ID and maps a matching left-button-up event to `ToggleRecording`.
A matching fixed history-item ID maps to `CopyHistory(full_text)`. `PlatformRuntime` performs
the native copy immediately; unrelated IDs and mouse phases are discarded.

The main-thread winit loop owns AppKit/Win32 dispatch in `ControlFlow::Wait` mode. `TrayManager` no
longer exposes a polling receiver or a manual platform event pump. `MacTray` still enforces
Accessory activation policy and creates no application window.

### Click Protection

- effective debounce window is `max(300 ms, platform double-click interval)`
- `MacTray` reads `NSEvent::doubleClickInterval()`
- `WindowsTray` reads `GetDoubleClickTime()` and suppresses the button-up following a native `DoubleClick`
- the suppression is one-shot; ordinary ignored clicks do not extend the window
- debounce applies only to tray recording toggles, never history, Exit, or hotkeys
