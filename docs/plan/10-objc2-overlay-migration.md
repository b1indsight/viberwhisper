# 10 - Overlay objc2 Migration

## Goal

Migrate the macOS floating overlay implementation from the deprecated `cocoa` + `objc` crates to the modern `objc2` ecosystem, while preserving current user-visible behavior.

This plan covers only the overlay window in `src/input/overlay/macos.rs`. It does not attempt a repo-wide AppKit migration.

## Background

The current overlay implementation works, but it now carries two maintenance costs:

1. `cocoa` / `objc` are the old binding stack for AppKit interop.
2. The overlay module produces a large number of deprecation and macro-related warnings under current Rust toolchains.

The current warning suppression is acceptable as a short-term containment measure, but it should not become the long-term platform bridge strategy.

## Goals

1. Keep the current overlay behavior unchanged:
   - 48x48 floating window
   - click-to-toggle recording
   - draggable, always-on-top, visible on all spaces
   - dark/light theme-aware drawing
   - recording state reflected in icon color
2. Replace `cocoa` / `objc` usage in the overlay module with `objc2`, `objc2-foundation`, and `objc2-app-kit`.
3. Remove the local `#![allow(deprecated)]` workaround from the overlay implementation.
4. Reduce compile-time warning noise from the macOS overlay path to near-zero.
5. Keep Windows and non-macOS overlay stubs unchanged.

## Non-goals

- Do not migrate tray, typer, or other macOS integration points in this phase.
- Do not redesign the overlay UI or change its public behavior.
- Do not introduce a cross-platform GUI framework.
- Do not implement the Windows overlay in this plan.

## Current State

Current implementation lives in:

- `src/input/overlay/macos.rs`

Current stack:

- `cocoa`
- `objc`
- manual `msg_send!`, `class!`, `sel!`
- custom `NSView` subclass registration via `ClassDecl`

Current integration points that must remain stable:

- `OverlayManager::new()`
- `OverlayManager::set_recording(&mut self, bool)`
- `OverlayManager::check_click(&self) -> bool`
- `OverlayManager::update(&self)`

## Proposed Tech Choice

Use the `objc2` family of crates:

- `objc2`
- `objc2-foundation`
- `objc2-app-kit`

These crates are the modern Rust bindings for Objective-C / AppKit interop and are the direct replacement direction for new macOS-native work.

## API Mapping

Expected migration mapping:

| Current | Target |
|---|---|
| `objc::msg_send!` | `objc2` messaging APIs |
| `objc::declare::ClassDecl` | `define_class!` |
| `cocoa::foundation::{NSRect, NSPoint, NSSize, NSString}` | `objc2_foundation` equivalents |
| `cocoa::appkit::{NSApp, NSWindow, NSColor, NSView}` | `objc2_app_kit` equivalents |
| raw `id` handles | typed `Retained<T>` / typed object references where possible |

The main code-shape shift is:

- from untyped Objective-C object handles
- to typed wrappers plus explicit main-thread constraints

## Design Constraints

### 1. Keep the current `OverlayManager` surface

The rest of the application should not need to know the binding stack changed.

`src/main.rs` should keep the same overlay call pattern:

```rust
let mut overlay = OverlayManager::new()?;
overlay.set_recording(true);
if overlay.check_click() { ... }
overlay.update();
```

### 2. Keep subclass-based drawing

The current overlay relies on a custom view class for:

- `drawRect:`
- `mouseDown:`
- `acceptsFirstMouse:`

That is still the correct structural approach after migration. The migration should replace the subclass machinery, not remove it.

### 3. Preserve click state and recording state flow

Current static atomics are acceptable and can remain if they still fit the new implementation cleanly:

- `CLICKED`
- `IS_RECORDING`

If `objc2` offers a cleaner state channel without expanding complexity too much, that can be considered, but it is not required for this migration.

## Proposed File Changes

| File | Change | Notes |
|---|---|---|
| `Cargo.toml` | Modify | Replace `cocoa` / `objc` macOS deps with `objc2` family |
| `src/input/overlay/macos.rs` | Rewrite | Main migration target |
| `docs/plan/10-objc2-overlay-migration.md` | New | This plan |
| `docs/plan/09-floating-window.md` | Optional | Add note that initial implementation used deprecated bridge |

## Migration Strategy

### Phase 1 - Dependency and type scaffolding

1. Add `objc2`, `objc2-foundation`, `objc2-app-kit` as macOS-only dependencies.
2. Remove `cocoa` and `objc` from the overlay path.
3. Introduce typed imports and basic window/view creation scaffolding.
4. Keep the overlay module compiling even if drawing is temporarily minimal during the transition.

**Exit criteria**:

- macOS build compiles with the new dependency stack
- `OverlayManager::new()` still returns a valid manager

### Phase 2 - Rebuild custom overlay view

1. Recreate `VWOverlayView` using `define_class!`.
2. Port `drawRect:`.
3. Port `mouseDown:`.
4. Port `acceptsFirstMouse:`.

**Exit criteria**:

- overlay window draws background and mic icon
- mouse click still flips the click flag

### Phase 3 - Restore full window behavior

1. Port borderless window creation.
2. Port transparent background and shadow.
3. Port draggable background behavior.
4. Port all-spaces behavior.
5. Port theme lookup.
6. Port redraw on `set_recording`.

**Exit criteria**:

- behavior matches current overlay implementation
- no visible regression in drag/click/theme/state behavior

### Phase 4 - Cleanup and warning reduction

1. Remove deprecated `cocoa` / `objc` imports.
2. Remove `#![allow(deprecated)]` from the overlay module.
3. Remove now-obsolete warning workarounds if no longer needed.
4. Run `cargo test`.
5. Run `cargo clippy` on macOS if practical.

**Exit criteria**:

- overlay module no longer relies on deprecated bridge crates
- warning count is materially lower than the current baseline

## Testing Plan

### Automated

- `cargo test`
- targeted unit test to keep static flag defaults correct
- build verification on macOS

### Manual

1. Start the app on macOS.
2. Confirm the overlay appears.
3. Click overlay:
   - idle -> recording
   - recording -> idle
4. Toggle recording with hotkeys and confirm overlay state updates.
5. Drag the overlay and confirm window movement still works.
6. Switch dark/light system appearance and confirm background/icon contrast remains correct.
7. Verify overlay remains visible across Spaces.

## Risks

### Risk 1 - `objc2` requires stricter main-thread handling

This is expected. AppKit objects should be created and touched only from the main thread. The migration should make this explicit instead of relying on looser old bindings.

### Risk 2 - Custom subclass migration is the hardest part

The biggest implementation risk is not the window itself; it is recreating the custom `NSView` subclass cleanly with `define_class!`.

Mitigation:

- migrate window creation first
- then port subclass methods one by one
- avoid rewriting state flow and drawing logic at the same time

### Risk 3 - Drawing APIs may differ slightly

The migration should preserve the current icon geometry first. Fine visual cleanup can be handled separately if needed.

## Acceptance Criteria

1. `src/input/overlay/macos.rs` no longer depends on `cocoa` or `objc`.
2. Overlay behavior remains equivalent to the current implementation.
3. The existing `OverlayManager` interface remains stable.
4. `cargo test` passes.
5. The macOS overlay path no longer needs broad deprecation suppression.

## PR Strategy

Recommended implementation PR sequence:

1. PR 1: Add plan and dependency scaffolding.
2. PR 2: Migrate `macos.rs` to `objc2` with behavior parity.
3. PR 3: Follow-up polish if any visual or lifecycle issues remain.
