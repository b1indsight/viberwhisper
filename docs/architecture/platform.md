# Platform Module Architecture

## Purpose

The `platform` module is the compile-time boundary for native desktop capabilities. It provides
one application-facing runtime for:

- global hotkey and tray input;
- status icon creation and recording-state updates;
- final text delivery to the focused control;
- history-menu refresh and explicit clipboard replacement; and
- platform-aware hotkey construction.

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
- a `TrayPolicy` implementation; and
- one text writer, one native clipboard function, and the native hotkey filter.

The compiler checks the selected backend's associated policies. There is no runtime OS enum,
feature flag, or OS-name match in the desktop listener.

## Application Interface

`platform/runtime.rs` exposes the application capabilities as inherent methods on
`PlatformRuntime<B, D>`. The listener uses the concrete `NativePlatform` alias selected for the
build target. `PlatformBackend` supplies native policy and operations; `RuntimeDrivers` provides
the injection boundary used by runtime tests.

| Method | Capability |
| --- | --- |
| `start(hotkeys, notify)` | Construct the runtime and connect native callbacks; returns `anyhow::Result<Self>` |
| `handle_event(event)` | Handle an opaque event and return an optional recording or exit action |
| `set_recording(recording)` | Update the tray recording indicator |
| `set_history(entries)` | Populate the recent-history menu |
| `push_history(text)` | Add a newly saved transcription to the menu |
| `text_typer()` | Clone the thread-safe text-delivery handle |

`NativePlatform::start` constructs the selected text writer and hotkey filter, starts the
process-lifetime `rdev` listener, creates the tray, and installs the native callbacks. The listener
application stores this single runtime instead of storing native drivers separately.

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

`TrayAction::CopyHistory` stays inside the runtime and calls the selected backend directly rather
than entering the recording state machine. Toggle hotkey releases, key repeats, unrelated tray/menu
IDs, rejected clicks, and native double-click tails produce no action. Recording-state decisions remain in
`ui::listener`; platform actions are mapped there to source-free `SessionEvent` values.

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
startup history (at most five)   ─> NativePlatform::set_history  ─> tray
successful appended entry        ─> NativePlatform::push_history ─> tray
TrayAction::CopyHistory(full text) ─> backend clipboard function
```

`NativePlatform` and its tray stay on the winit/main thread. `text_typer()` clones only the
`Send + Sync` text writer handle for the finalization worker, where history persistence and text
delivery run. History copy calls the selected backend's native clipboard function directly.

Raw tray events must return through winit before they are classified because click debounce and
own-ID filtering mutate the main-thread-owned `TrayManager`. Native callback threads only enqueue
events and never copy text or run recording transitions.

## Hotkey Policy

`platform::hotkey_config` resolves persisted names through the selected backend policy before a
listener starts. Shared parsing, aliases, duplicates, repeat suppression, and construction errors
remain in `input::hotkey`. Backends own the native differences:

- macOS rejects key variants the current `rdev` backend cannot emit and normalizes modifier
  direction with Core Graphics physical key state;
- Windows rejects `RIGHTMETA`, `FUNCTION`, and keypad Enter, rejects a
  `LEFTCTRL`/`RIGHTALT` pair, and warns that AltGr can emit left Ctrl;
- macOS supplies the synthetic-paste suppression filter paired with `MacTyper`;
- Windows and the fallback backend supply an identity event filter.

Non-function pass-through warnings remain shared because passive `rdev` observation does not stop
the key from reaching the focused application or operating system.

## Tray and Status Icon Policy

`input::tray::TrayManager<P>` retains common icon decoding, menu construction, recent-five entries,
callback installation, click debounce, tooltip, and status behavior. The selected `TrayPolicy`
supplies only native differences:

| Capability | macOS | Windows |
|---|---|---|
| application preparation | AppKit Accessory activation on main thread | no-op |
| idle icon | template image | committed explicit color |
| recording icon | committed explicit red | committed explicit red |
| double-click interval | `NSEvent::doubleClickInterval` | `GetDoubleClickTime` |
| icon update | `set_icon_with_as_template` | `set_icon` |

Both platforms retain the 300 ms minimum debounce window and embedded 32×32 RGBA icon assets.

## Configuration Directory

The configuration storage module owns `core::config::config_dir()` and obtains the base
from `dirs::config_dir()`. macOS appends `com.b1indsight.viberwhisper`; Windows appends
`ViberWhisper`; the fallback appends `viberwhisper`. `ConfigStore` appends `config.json` and
`HistoryStore` appends `history.jsonl`. Directory lookup does not depend on the desktop backend.

## macOS Text Delivery

`MacBackend` constructs `MacTyper` and `MacClipboard` together with the filter returned by its
`NativePasteWriter`. Paste hotkey suppression and a shared clipboard/delivery mutex remain private
to the platform boundary.

`MacTyper` serializes non-empty deliveries, sleeps 100 ms so the target can regain focus, enters an
Objective-C autorelease pool, classifies the frontmost bundle with
`macos/application.rs`, and uses two native routes:

1. Ordinary applications use `macos/accessibility.rs` to resolve the focused AX element, reject
   `AXSecureTextField`, check whether `AXSelectedText` is settable, and assign the transcription to
   that attribute. An empty selection inserts at the caret; a non-empty selection is replaced.
   This path does not touch the clipboard or emit keyboard events. A present, non-secure control
   that explicitly lacks settable selected text uses native paste instead.
2. Identified Chrome, Chromium, Edge, Brave, Arc, Vivaldi, and Opera bundles always use native
   paste after checking Accessibility trust and rejecting a secure focused element when Chromium
   exposes one. `NoFocusedElement` is accepted only on this route because Chromium can hide a DOM
   editor from macOS while it retains keyboard focus. Browser delivery never assigns
   `AXSelectedText`: Chromium can accept that AX call while only queueing an asynchronous renderer
   action that does not update the page.

Missing Accessibility trust, secure controls exposed through AX, invalid AX objects, type
mismatches, and messaging failures are hard errors. A missing focused element remains a hard error
outside an identified Chromium-family browser. When Chromium hides its entire focused web subtree,
macOS cannot distinguish an ordinary DOM editor from a password editor without activating renderer
accessibility; the paste route then has the same destination boundary as a user-issued Cmd+V.

ViberWhisper does not set Chromium's undocumented `AXEnhancedUserInterface` attribute. It therefore
avoids the browser's delayed, process-wide screen-reader mode and the CPU/memory cost of maintaining
web accessibility trees solely for text delivery.

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

CoreGraphics posting has no destination acknowledgement. Success means the pasteboard write and
event posting completed, not that the target consumed the command. Logs describe a posted paste,
and the retained transcription provides a manual Cmd+V recovery path.

Accessibility permission is required for both routes. Missing permission is a hard input error and
leaves the clipboard untouched.

The desktop UI checks `AXIsProcessTrusted` on startup and shows macOS-owned native guidance
on the main thread when trust is missing. Typed permission errors from full or partial delivery
also enqueue an application event, covering permission revoked after startup. One per-launch
flag suppresses repeated alerts, and the main thread rechecks trust before showing a queued
message. Canceled finalization does not request guidance. Capture, config, and offline conversion
flows do not show this text-delivery alert. The alert offers settings instructions, preview-copy
recovery, and an explicit Open Settings button; it does not change permission or retry delivery.

### History clipboard copy

`MacClipboard` shares `MacTyper`'s delivery mutex and reuses the existing AppKit string replacement.
It does not inspect Accessibility, post keyboard events, or enter hotkey suppression.

## Windows Text Delivery

`WindowsBackend` constructs the zero-state `WindowsTyper`. `type_text`:

1. sleeps 100 ms so the target can regain focus;
2. expands the text into UTF-16 code units;
3. creates key-down/key-up `INPUT` pairs using `KEYEVENTF_UNICODE`;
4. calls Win32 `SendInput`; and
5. verifies that every event was accepted.

`type_text_checked` validates the saved destination after the focus delay. A partially accepted
SendInput batch returns `PartialTextDelivery`; the UI retains the candidate for copying and
disables confirmation retry, because replaying the full text could duplicate an accepted prefix.

Surrogate pairs therefore produce one down/up pair for each UTF-16 code unit. The private FFI
module owns the C layouts, flags, `SendInput`, and `GetDoubleClickTime` declarations linked from
`user32.dll`.

`windows/clipboard.rs` writes `CF_UNICODETEXT` through Win32 using an invisible message-only owner
window and movable global memory. Small RAII guards close the clipboard, destroy the window, and
free memory that has not transferred to Windows. This path emits no `SendInput` events.

## Confirmation Preview

`platform/preview.rs` selects `NativePreview` and `NativeTarget` at compile time. AppKit windows
and Win32 borderless rounded bubbles with editable native text controls run on the winit thread. They consume
Enter/Esc key-down and repeat events and emit an action on matching key-up. Only a fresh key-down
arms that action; losing focus disarms it, and inherited repeats cannot re-arm it. Native
composition keeps Enter/Esc until candidate selection finishes;
Shift+Enter adds a newline. Confirmation reads the editor's current text into the session-scoped
event. Normal bubbles contain only the editor; operational errors reveal a compact inline message.
Closing clears visible text.

Both bubbles keep a fixed wrapping width and top edge while editing changes their height.
macOS listens to the text delegate and measures the native text layout. Windows handles the
edit control's `EN_CHANGE` notification, measures its wrapped line count with its actual font,
and resizes the editor and inline message together.
The editor viewport ranges from 28 to 196 native units; longer content scrolls inside it.
Message visibility survives edits and contributes a separate footer to the bubble height.

macOS registers standard Edit-menu key equivalents against the responder chain for Cmd+A/C/X/V.
Windows consumes ordinary Return/Escape character messages even when a repeat did not arm an
action, while composition and modified Return remain with the native editor.

The finalization worker captures a target before the preview requests focus and retains it on
that same worker. macOS saves the running application, AX window and available focused AX element;
Windows saves the top-level HWND, process/thread identity and focused HWND. Restoring a target
does not bypass system foreground restrictions. Both backends wait on the worker for activation
to settle, retrying validation at 10 ms intervals for up to 500 ms. Validation rechecks window and available control
identity before history and again immediately before injection. Missing/closed targets and
failed restoration preserve the candidate for copying. Browsers without exposed editor identity
only provide window-level verification; the code does not claim DOM-level destination identity.
macOS target operations preserve the canonical typed AX permission denial, and the shared permission
policy recognizes both raw target errors and wrapped typer errors before requesting recovery guidance.

With the existing `integration` feature, `viberwhisper-preview` opens sample text without API or
microphone access. It captures a destination after a three-second focus window and uses the same
confirmation worker and native typer as dictation, without persistent demo history. Its `--smoke`
mode edits the native control, sends held-Enter events and verifies zero callbacks before release,
one confirmation with edited text after release, and successful delivery to the selected target.
