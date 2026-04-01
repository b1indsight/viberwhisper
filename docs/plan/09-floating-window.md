# 09 - Floating Window Overlay

## Goal

Add a floating overlay window that shows recording status and allows click-to-toggle recording. Addresses issue #15.

## Requirements

1. **Appearance**: 48×48 rounded rectangle with microphone icon
2. **Background**: Matches system theme (dark/light mode)
3. **Icon states**: White/dark mic when idle, red mic when recording
4. **Interaction**: Click to toggle recording on/off
5. **Window behavior**: Always on top, borderless, draggable, doesn't steal focus, visible on all spaces

## Tech Choice

Use `cocoa` (0.26) + `objc` (0.2) crates to create a native macOS NSWindow. This matches the project's existing pattern of direct platform API calls (osascript for typing, rdev for hotkeys) without introducing a GUI framework.

No `core-graphics` crate needed — drawing is done via `NSBezierPath` which is available through `cocoa`/`objc`.

### Why not other options?
- **egui/iced/winit**: Too heavyweight for a single 48×48 overlay; adds large dependency tree
- **Tauri**: Project is pure Rust CLI, not a web app
- **tray-icon window**: tray-icon crate doesn't support custom floating windows

## Implementation

### Module: `src/input/overlay/`

Platform-specific implementations behind `#[cfg]`:
- `macos.rs` — Cocoa NSWindow + custom NSView with NSBezierPath drawing
- `windows_impl.rs` — Stub (to be implemented later)
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

- Borderless `NSWindow` at level 25 (NSStatusWindowLevel)
- Custom `VWOverlayView` NSView subclass with `drawRect:` override
- System theme detection via `NSApp.effectiveAppearance`
- `acceptsFirstMouse:` returns YES for click-without-activate
- `setMovableByWindowBackground:` for dragging
- `NSWindowCollectionBehaviorCanJoinAllSpaces` for all desktops
- Recording state via static `AtomicBool` (thread-safe, polled by main loop)
- Microphone icon drawn with `NSBezierPath` (body, arc, stand, base)

### Cargo.toml Changes

```toml
[target.'cfg(target_os = "macos")'.dependencies]
cocoa = "0.26"
objc = "0.2"
```

### Integration with main.rs

Overlay plugs into the existing main event loop alongside tray and hotkeys:
- `overlay.check_click()` triggers toggle recording (same logic as F9)
- `overlay.set_recording(bool)` syncs with all tray/hotkey state changes
- `overlay.update()` pumps Cocoa events each loop iteration

### Implementation Steps

1. Add `cocoa` and `objc` to `Cargo.toml` (macOS-only)
2. Create `src/input/overlay/mod.rs` with platform dispatch
3. Create `src/input/overlay/macos.rs` with full implementation
4. Create `src/input/overlay/windows_impl.rs` and `stub.rs` as no-ops
5. Add `pub mod overlay;` to `src/input/mod.rs`
6. Integrate into `run_listener()` in `src/main.rs`
7. Add unit test for static flag defaults
8. Verify: `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`
9. Update CLAUDE.md implemented features list
10. Add changelog entry
