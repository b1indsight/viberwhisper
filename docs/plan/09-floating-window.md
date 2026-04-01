# 09 - Floating Window Overlay

## Goal

Add a floating overlay window that shows recording status and allows click-to-toggle recording. Addresses issue #15.

## Requirements

1. **Appearance**: 48×48 rounded rectangle with microphone icon
2. **Background**: Matches system theme (dark/light mode)
3. **Icon states**: White/dark mic when idle, red mic when recording
4. **Interaction**: Click to toggle recording on/off
5. **Window behavior**: Always on top, borderless, draggable, doesn't steal focus, visible on all spaces

## Implementation

### Module: `src/input/overlay/`

Platform-specific implementations behind `#[cfg]`:
- `macos.rs` — Cocoa NSWindow + custom NSView with CoreGraphics drawing
- `windows_impl.rs` — Stub (to be implemented)
- `stub.rs` — No-op fallback for other platforms

### Public API

```rust
pub struct OverlayManager { ... }

impl OverlayManager {
    pub fn new() -> Result<Self, Box<dyn Error>>;
    pub fn set_recording(&mut self, recording: bool);
    pub fn check_click(&self) -> bool;
    pub fn update(&self);
}
```

### macOS Details

- Borderless `NSWindow` at `NSStatusWindowLevel` (25)
- Custom `VWOverlayView` subclass with `drawRect:` override
- System theme detection via `NSApp.effectiveAppearance`
- `acceptsFirstMouse:` returns YES for click-without-activate
- `setMovableByWindowBackground:` for dragging
- `NSWindowCollectionBehaviorCanJoinAllSpaces` for all desktops
- Recording state via static `AtomicBool` (thread-safe, polled by main loop)

### Integration

Overlay plugs into the main event loop alongside tray and hotkeys:
- `overlay.check_click()` triggers toggle recording (same as F9)
- `overlay.set_recording(bool)` syncs with all tray/hotkey state changes
- `overlay.update()` pumps Cocoa events each loop iteration
