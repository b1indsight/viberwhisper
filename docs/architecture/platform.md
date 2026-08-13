# Platform Module Architecture

## Purpose

The `platform` module is the compile-time boundary for native desktop capabilities. It provides
one application-facing runtime for:

- global hotkey and tray input;
- status icon creation and recording-state updates;
- final text delivery to the focused control;
- platform-aware hotkey validation; and
- configuration-directory discovery.

Application and shared input code do not select an operating system, construct native writers, or
handle `rdev`/`tray-icon` payload types.

## Compile-Time Selection

`src/platform.rs` is the only desktop target-selection point:

```rust
#[cfg(target_os = "macos")]
use macos::MacBackend as SelectedBackend;

#[cfg(target_os = "windows")]
use windows::WindowsBackend as SelectedBackend;

pub(crate) type NativePlatform = PlatformRuntime<SelectedBackend>;
```

Unsupported development targets select `FallbackBackend`, which retains the existing mock text
delivery and generic tray behavior without being a supported release platform. Target-specific
native dependencies remain selected by Cargo target dependency tables.

`platform/backend.rs` defines the private `PlatformBackend` contract. Each backend supplies:

- a `HotkeyPolicy` implementation;
- a `TrayPolicy` implementation;
- its configuration directory; and
- one text writer paired with its native hotkey filter.

The compiler checks the selected backend's associated policies. There is no runtime OS enum,
feature flag, or OS-name match in the application layer.

## Application Interface

`platform/runtime.rs` implements the common `PlatformInterface`:

```rust
pub(crate) trait PlatformInterface: Sized {
    fn start(hotkeys, notify) -> Result<Self, Box<dyn Error>>;
    fn handle_event(&mut self, event: PlatformEvent) -> Option<PlatformAction>;
    fn set_recording(&mut self, recording: bool);
    fn text_typer(&self) -> Arc<dyn TextTyper>;
}
```

`NativePlatform::start` constructs the selected text writer/filter pair, starts the
process-lifetime `rdev` listener, creates the tray, and installs the native callbacks. The listener
application stores this single runtime instead of storing a tray and text writer separately.

`PlatformEvent` is an opaque wrapper around private hotkey or tray events. Native callbacks enqueue
it through winit, and the main-thread listener passes it back to `handle_event`. The runtime returns
only these semantic actions:

```rust
pub(crate) enum PlatformAction {
    HoldPressed,
    HoldReleased,
    ToggleRecording,
    ExitRequested,
}
```

Toggle hotkey releases, key repeats, unrelated tray/menu IDs, rejected clicks, and native
double-click tails produce no action. Recording-state decisions remain in
`application::listener`; platform actions are mapped there to source-free `SessionEvent` values.

## Ownership and Event Flow

```text
rdev callback ─┐
               ├─> opaque PlatformEvent ─> winit AppEvent::Platform
tray callback ─┘                                  │
                                                 v
                                  NativePlatform::handle_event
                                                 │
                                      optional PlatformAction
                                                 │
                                                 v
                                   RecordingSessionMachine

SessionEffect::SetTrayRecording ─> NativePlatform::set_recording
final transcription              ─> NativePlatform::text_typer ─> worker
```

`NativePlatform` and its tray stay on the winit/main thread. `text_typer()` clones only the
`Send + Sync` text writer handle for a finalization worker. AppKit/Win32 tray values never cross to
that worker, and text delivery never blocks the native event loop.

Raw tray events must return through winit before they are classified because click debounce and
own-ID filtering mutate the main-thread-owned `TrayManager`. Native callback threads only enqueue
events and never run recording transitions.

## Hotkey Policy

`platform::validate_hotkeys` resolves persisted names through the selected backend policy before a
listener starts. Shared parsing, aliases, duplicates, repeat suppression, and stable validation
codes remain in `input::hotkey`. Backends own the native differences:

- macOS rejects key variants the current `rdev` backend cannot emit and normalizes modifier
  direction with Core Graphics physical key state;
- Windows rejects `RIGHTMETA`, `FUNCTION`, and keypad Enter, rejects a
  `LEFTCTRL`/`RIGHTALT` pair as `hotkey.altgr_conflict`, and warns that AltGr can emit left Ctrl;
- macOS supplies the synthetic-paste suppression filter paired with `MacTyper`;
- Windows and the fallback backend supply an identity event filter.

Non-function pass-through warnings remain shared because passive `rdev` observation does not stop
the key from reaching the focused application or operating system.

## Tray and Status Icon Policy

`input::tray::TrayManager<P>` retains the common icon decoding, menu construction, callback
installation, own-ID filtering, click debounce, tooltip, and menu status behavior. The selected
`TrayPolicy` supplies only native differences:

| Capability | macOS | Windows |
|---|---|---|
| application preparation | AppKit Accessory activation on main thread | no-op |
| idle icon | template image | committed explicit color |
| recording icon | committed explicit red | committed explicit red |
| double-click interval | `NSEvent::doubleClickInterval` | `GetDoubleClickTime` |
| icon update | `set_icon_with_as_template` | `set_icon` |

Both platforms retain the 300 ms minimum debounce window and embedded 32×32 RGBA icon assets.

## Configuration Directory

`platform::config_dir()` delegates through the selected backend. macOS appends
`com.b1indsight.viberwhisper`; Windows appends `ViberWhisper`; the fallback appends
`viberwhisper`. `ConfigStore` alone appends `config.json`, so platform code does not know the
configuration schema or persistence errors.

## macOS Text Delivery

`MacBackend` constructs `MacTyper` together with the filter returned by its
`NativePasteWriter`. That shared suppression state is private to the platform boundary.

`MacTyper` serializes non-empty deliveries, sleeps 100 ms so the target can regain focus, enters an
Objective-C autorelease pool, and uses two native routes:

1. `macos/accessibility.rs` resolves the focused AX element, rejects `AXSecureTextField`, checks
   whether `AXSelectedText` is settable, and assigns the transcription to that attribute. An empty
   selection inserts at the caret; a non-empty selection is replaced. This path does not touch the
   clipboard or emit keyboard events.
2. A present, non-secure control that explicitly lacks settable selected text uses the native paste
   fallback. Missing Accessibility trust, no focused element, secure controls, invalid AX objects,
   type mismatches, and messaging failures are hard errors and do not paste into an unknown target.

The implementation never reads/writes `AXValue`, builds AppleScript, or launches a subprocess.
Text uses `CFString`/`NSString`, preserving multiline content, quotes, backslashes, CJK, and emoji.

### Native paste fallback

`macos/pasteboard.rs` clears the general pasteboard, writes the transcription as
`NSPasteboardTypeString`, and intentionally leaves it on the clipboard. It posts left Command down,
V down, V up, and left Command up through a HID-system Core Graphics event source.

An atomic RAII suppression scope filters ViberWhisper's own mapping callback across event posting
and a fixed 100 ms asynchronous-event grace. The operating system and focused application still
receive every event. A filtered callback resets mapper key-down state; outside the scope,
`macos/hotkey.rs` normalizes modifier direction and passes ordinary events unchanged.

Accessibility permission is required. Missing permission is a hard input error and leaves the
clipboard untouched.

## Windows Text Delivery

`WindowsBackend` constructs the zero-state `WindowsTyper`. `type_text`:

1. sleeps 100 ms so the target can regain focus;
2. expands the text into UTF-16 code units;
3. creates key-down/key-up `INPUT` pairs using `KEYEVENTF_UNICODE`;
4. calls Win32 `SendInput`; and
5. verifies that every event was accepted.

Surrogate pairs therefore produce one down/up pair for each UTF-16 code unit. The private FFI
module owns the C layouts, flags, `SendInput`, and `GetDoubleClickTime` declarations linked from
`user32.dll`.
