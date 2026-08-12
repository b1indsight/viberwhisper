# Compile-Time Platform Interface

## Status

**Implemented; automated validation complete.** The common runtime, compile-time backends, opaque
event boundary, target-neutral input drivers, listener integration, tests, and current-truth
documentation are complete. Local checks and hosted macOS/Windows CI pass. The native
macOS/Windows smoke matrix remains required before PR readiness.

## Implementation Result

The implementation follows the approved ownership and capability boundary. `NativePlatform` is a
compile-time alias for `PlatformRuntime<SelectedBackend>`; listener code contains no target
selection and receives only opaque `PlatformEvent` values plus semantic `PlatformAction` results.
Hotkey and tray policies now live in their selected platform backends, while shared parsing,
mapping, icon/menu construction, and debounce mechanics remain in `input`.

Two proportional implementation refinements were made:

- startup retains the existing boxed error boundary instead of adding a forwarding-only
  `PlatformStartError` enum; and
- an injected driver seam lets the platform-runtime contract test exercise startup callbacks,
  opaque event routing, tray state, and text delivery without installing process-global `rdev`
  or `tray-icon` handlers. macOS and Windows backend policy tests cover their native differences
  directly.

Local validation completed with the full 117-test macOS suite, strict Clippy, native build/check,
an Intel `x86_64-apple-darwin` check, and strict `x86_64-pc-windows-msvc` check/Clippy. The
source-boundary search confirms no `target_os` conditions remain in `application`, `input`, or
`runtime_config`. Hosted macOS, Windows, and Python CI also pass on the pushed implementation.

## Goal

Give the application layer one platform-neutral interface for the desktop capabilities it uses:

1. receive global Hold/Toggle and tray input;
2. create and update the status icon;
3. deliver final text to the focused control; and
4. discover the platform configuration directory.

The Rust compiler selects the macOS or Windows implementation. Application and session objects
call only the common interface and do not contain `#[cfg(target_os)]`, name `MacTyper` or
`WindowsTyper`, supply a native hotkey filter, or handle `rdev`/`tray-icon` event types.

This is an architecture refactor. It preserves the current user-visible icon, hotkey, tray, text
injection, configuration, event-loop, and packaging behavior.

## Current Problem

Compile-time selection exists, but it is incomplete and spread across layers:

- `application::listener` selects `MacTyper`, `WindowsTyper`, `MockTyper`, and the macOS hotkey
  filter with several target `cfg` blocks.
- `input::hotkey` owns shared parsing and mapping but also embeds macOS/Windows support tables,
  Windows AltGr rules, and platform warnings.
- `input::tray` embeds AppKit activation, macOS template-icon updates, native double-click lookup,
  and Win32 `GetDoubleClickTime` behind target `cfg` blocks.
- raw `TrayEvent` and `HotkeyEvent` values enter `application::listener::AppEvent`, so the
  application event loop knows which native input adapter produced an event.
- macOS text injection must construct its paste writer and hotkey filter together, but the
  application is currently responsible for preserving that platform-specific relationship.

The result is a shared application workflow that calls common methods only after it has already
made platform decisions. Adding or changing a platform capability therefore requires edits in
application and input integration code as well as the platform implementation.

## Scope

### In scope

- A common platform lifecycle and capability interface used by the listener application.
- One compile-time-selected native platform type for macOS and Windows.
- An opaque platform event envelope and platform-neutral semantic actions.
- Platform ownership of tray/icon policy, hotkey policy/filtering, and text writer construction.
- Removal of target-specific branches from `application`, `runtime_config`, and shared input
  drivers.
- Preservation of the current unsupported-target test/development fallback without claiming a
  third supported desktop platform.
- Tests that enforce the shared contract and retain platform-specific behavior.

### Out of scope

- A new window, settings UI, overlay, icon design, animation, or system notification.
- Runtime operating-system detection, a platform setting, or feature-flag selection.
- Replacing `winit`, `tray-icon`, `rdev`, AppKit/Accessibility/CoreGraphics, or Win32 `SendInput`.
- Changing hotkey names, validation codes, defaults, pass-through behavior, or configuration
  schema.
- Changing macOS Accessibility/fallback routing, clipboard replacement policy, required
  permissions, or Windows Unicode injection semantics.
- Generalizing audio, transcription, post-processing, or local inference behind this interface.
- Advertising Linux or another OS as supported.

## Design

### 1. Put compile-time selection in one module

`src/platform.rs` will be the only production selection point:

```rust
#[cfg(target_os = "macos")]
type SelectedBackend = macos::Backend;

#[cfg(target_os = "windows")]
type SelectedBackend = windows::Backend;

pub(crate) type NativePlatform = PlatformRuntime<SelectedBackend>;
```

An explicit fallback backend will retain lightweight non-native behavior for unsupported
development/test targets. It will not be used by either release target and will not be described
as product support.

`macos::Backend` and `windows::Backend` implement a private backend contract. The compiler checks
the selected backend and excludes the other target's native bindings and linker symbols. There is
no runtime platform enum and no OS-name match in application code.

Target-specific Cargo dependencies remain under their existing target dependency tables.

### 2. Expose one application-facing runtime interface

The application-facing contract will be deliberately small:

```rust
pub(crate) trait PlatformInterface: Sized {
    fn start(
        hotkeys: &HotkeyConfig,
        notify: impl Fn(PlatformEvent) + Send + Sync + 'static,
    ) -> Result<Self, PlatformStartError>;

    fn handle_event(&mut self, event: PlatformEvent) -> Option<PlatformAction>;
    fn set_recording(&mut self, recording: bool);
    fn text_typer(&self) -> Arc<dyn TextTyper>;
}
```

The concrete `NativePlatform` owns the main-thread tray controller and a cloneable, thread-safe
text writer handle. Startup also installs the process-lifetime hotkey and tray callbacks. The
listener creates this object once, stores it in `ListenerApplication`, and invokes only these
methods.

`config_dir()` remains a common platform facade function because configuration discovery happens
before the listener runtime is created. Hotkey resolution similarly enters through a common
platform facade so `runtime_config` does not choose an OS policy.

The exact visibility and error representation may be tightened during implementation, but the
capability boundary and ownership model above are fixed by this plan.

### 3. Keep native events opaque and return semantic actions

Callbacks enqueue an opaque `PlatformEvent` through winit. Its native payload remains private to
the platform runtime. `ListenerApplication` passes it back to `NativePlatform::handle_event` and
receives at most one semantic action:

```rust
pub(crate) enum PlatformAction {
    HoldPressed,
    HoldReleased,
    ToggleRecording,
    ExitRequested,
}
```

The mapping is:

| Native input | Platform action |
|---|---|
| configured Hold press | `HoldPressed` |
| configured Hold release | `HoldReleased` |
| configured Toggle press | `ToggleRecording` |
| Toggle release/key repeat | none |
| accepted tray left-button release | `ToggleRecording` |
| tray Exit menu item | `ExitRequested` |

`AppEvent` will contain `Platform(PlatformEvent)` rather than separate hotkey and tray variants.
The application remains responsible for mapping semantic actions against `RecordingState` into
source-free `SessionEvent` values. Recording lifecycle decisions therefore stay outside the
platform layer.

This opaque event round-trip is intentional: raw tray input must be debounced and filtered by the
main-thread-owned tray controller, while native callbacks must remain non-blocking and wake winit
instead of mutating that controller from callback threads.

### 4. Split shared driver mechanics from platform policy

`input::hotkey` will retain only cross-platform mechanics:

- canonical key parsing and shared aliases;
- `HotkeyConfig` data;
- repeat-safe Hold/Toggle mapping;
- the process-lifetime `rdev` listener thread; and
- policy-driven validation and event filtering.

The private platform backend supplies:

- unsupported-key decisions and messages;
- Windows AltGr pair validation and warnings;
- macOS physical modifier normalization;
- the macOS synthetic-paste suppression filter; and
- the Windows/fallback pass-through filter.

`input::tray` will retain embedded PNG decoding, menu construction, own-ID filtering, click
debounce, tooltips, and status text. Platform tray policy supplies:

- AppKit Accessory activation or the Windows no-op preparation;
- system double-click interval lookup;
- macOS template-image versus explicit-color updates; and
- Windows ordinary icon updates.

Shared drivers depend on private policy contracts or policy values. They will contain no
`target_os` branches and will not expose their raw native types above `platform`.

### 5. Preserve text-writer coupling and thread ownership

`TextTyper: Send + Sync` remains the common delivery contract used by the background finalization
worker.

The macOS backend constructs `MacTyper`, `NativePasteWriter`, and the paired hotkey filter inside
the platform boundary. This preserves their shared suppression flag without returning a closure to
the application. Accessibility-first insertion, secure-control rejection, paste fallback,
modifier normalization, serialization, focus delay, and the fixed suppression grace remain
unchanged.

The Windows backend constructs the `SendInput` writer and an identity hotkey filter. UTF-16 input
construction, full-send verification, and focus delay remain unchanged.

`NativePlatform` itself stays on the winit/main thread because it owns the tray. Only the cloned
`Arc<dyn TextTyper>` crosses to a finalization worker. No AppKit/Win32 tray object becomes `Send`,
and text delivery does not move back onto the event-loop thread.

### 6. Route hotkey validation through the selected backend

The canonical string vocabulary and stable validation codes remain shared. Platform availability
and conflict rules move behind the selected backend:

- macOS retains its current unsupported `rdev` key set;
- Windows retains `RIGHTMETA`, `FUNCTION`, and keypad-Enter limitations;
- Windows retains `hotkey.altgr_conflict` and the `LEFTCTRL` warning; and
- both platforms retain pass-through warnings for non-function keys.

`runtime_config::resolve_listener` invokes the common platform validation entry point and receives
the same `HotkeyConfig`/`ValidationIssue` results it uses today. Persisted configuration behavior
does not change.

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

The platform layer owns native classification and presentation. The application owns workflow
state. Core remains unaware of native input sources and operating systems.

## File and Module Plan

| File | Planned responsibility/change |
|---|---|
| `src/platform.rs` | Define the common facade, opaque event/action contract, and the sole target-based backend alias; retain common config-directory entry. |
| `src/platform/backend.rs` | Add private backend, hotkey-policy, and tray-policy contracts used by the common runtime. |
| `src/platform/runtime.rs` | Own shared startup/wiring, tray state, text-writer handle, native-event handling, and semantic action mapping. |
| `src/platform/macos.rs` | Implement the macOS backend and keep native text coordination private. |
| `src/platform/macos/hotkey.rs` | Retain modifier normalization and expose it only through the macOS backend policy. |
| `src/platform/macos/pasteboard.rs` | Retain the paired paste/suppression implementation without exposing its filter to application code. |
| `src/platform/windows.rs` | Implement the Windows backend around `SendInput`, Windows hotkey policy, and tray policy. |
| `src/platform/fallback.rs` | Provide the explicit unsupported-target test/development adapter currently represented by `MockTyper` and no-op policy. |
| `src/input/hotkey.rs` | Remove target branches; accept private policy/filter inputs while retaining parsing, mapping, logging, and listener mechanics. |
| `src/input/tray.rs` | Remove target branches; accept private tray policy and keep raw tray events internal to platform wiring. |
| `src/input/typer.rs` | Retain the thread-safe common text-delivery trait; move fallback construction behind the platform facade. |
| `src/application/listener.rs` | Construct only `NativePlatform`; remove typer/filter/tray selection and target `cfg` blocks. |
| `src/application/listener/event_loop.rs` | Store `NativePlatform`, handle one opaque platform event variant, call the common recording/text methods, and consume semantic actions. |
| `src/runtime_config.rs` | Resolve hotkeys through the common selected-platform validation entry point. |

File boundaries may be collapsed if an implementation unit stays small, but native code must stay
under `src/platform/`, shared input drivers must stay target-neutral, and the application-facing
contract must not expand to native event types.

No dependency or configuration-schema change is expected.

## Test Strategy

### Contract and shared tests

1. Add a fake backend that drives `PlatformRuntime` without native APIs.
2. Prove hotkey and tray callback payloads remain opaque until `handle_event` runs.
3. Prove Hold press/release, Toggle press, tray toggle, Exit, repeats, unrelated IDs, and
   double-click suppression map to the expected semantic action or no action.
4. Prove `set_recording` delegates the idle/recording visual state exactly once and does not alter
   recording state itself.
5. Prove `text_typer` returns a cloneable worker-safe handle while the platform runtime remains
   main-thread owned.
6. Retain key parsing, aliases, duplicate detection, mapper reset, and callback-thread tests using
   a fake hotkey policy instead of target `cfg` blocks.
7. Retain icon decode, dimensions/transparency, menu-ID filtering, and debounce boundary tests
   using a fake tray policy.
8. Update listener tests to exercise `PlatformAction` to `SessionEvent` mapping independently of
   native input sources.

### Platform tests

- macOS CI retains Accessibility route, paste fallback, synthetic-event suppression, modifier
  normalization, template-icon policy, and unsupported-key coverage.
- Windows CI covers the AltGr conflict/warning policy, unsupported keys, icon update policy, and a
  pure Unicode `INPUT` construction helper before the real `SendInput` boundary.
- The fallback adapter proves it cannot be mistaken for a macOS or Windows backend.

### Automated validation

Run on the available native host and require both hosted target jobs:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked -- -D warnings
cargo build --locked
git diff --check
```

Also verify that target selection has not leaked back into consumers:

```bash
rg -n 'cfg\(.*target_os' src/application src/input src/runtime_config.rs
```

The expected result is empty. macOS and Windows GitHub Actions jobs must both pass before PR
readiness.

### Manual native verification

On macOS:

1. Confirm idle template tint and explicit red recording icon in light and dark menu-bar modes.
2. Confirm Hold, Toggle, tray toggle, double-click suppression, and Exit behavior.
3. Confirm Accessibility insertion, native paste fallback, secure-field rejection, and synthetic
   Cmd+V hotkey suppression.

On Windows:

1. Confirm idle/recording icon colors, tray toggle, double-click suppression, and Exit behavior.
2. Confirm Hold/Toggle input including a layout with AltGr where available.
3. Confirm ASCII, multiline, CJK, emoji, and surrogate-pair text delivery through `SendInput`.

## Implementation Order

1. Add failing fake-backend tests for the opaque event/action contract and platform runtime
   ownership.
2. Introduce the private backend/policy contracts and compile-time `SelectedBackend` alias while
   leaving behavior delegated to the existing drivers.
3. Move hotkey target rules and filter construction behind macOS/Windows backends; make the shared
   hotkey driver target-neutral.
4. Move application activation, double-click lookup, and icon update policy behind the platform
   tray contract; keep decode/menu/debounce mechanics shared.
5. Make the runtime facade construct the selected text writer, hotkey listener, and tray together;
   retain macOS writer/filter coupling internally.
6. Replace listener hotkey/tray variants with opaque `PlatformEvent`, store `NativePlatform`, and
   route only `PlatformAction` values into session requests.
7. Route runtime hotkey validation and configuration-directory discovery through the common
   selected-platform facade.
8. Run focused and full checks on macOS, obtain Windows CI validation, perform the native smoke
   matrices, and synchronize documentation with the final implementation.
9. Run the repository's code-review gate, push implementation to this same bookmark/PR, and mark
   the PR ready only after validation completes.

## Documentation Impact

During planning, add only this plan and its `docs/README.md` index entry.

During implementation:

- update `AGENTS.md` so the project tree and platform responsibilities match the new boundary;
- update `docs/architecture/platform.md` with the compile-time selection, facade contract,
  ownership, events/actions, and native backend responsibilities;
- update `docs/architecture/input.md` so hotkey/tray are described as target-neutral drivers and
  text delivery is obtained through the platform facade;
- update `docs/architecture/core.md` where it describes separate hotkey/tray winit producers;
- update the architecture descriptions and this plan's status in `docs/README.md`;
- add a `changelog` entry because this repository records material architecture refactors; and
- record any material deviation from this design in this plan rather than rewriting its original
  rationale.

`README.md`, `config.example.json`, user configuration reference, release instructions, and asset
documentation should not change because the refactor adds no user-visible behavior, setting,
artifact, or icon. If implementation changes that assumption, the documentation impact must be
reassessed before PR readiness.

## Acceptance Criteria

- `application`, `runtime_config`, and shared `input` code contain no target-OS selection.
- `src/platform.rs` selects one macOS, Windows, or explicit fallback backend at compile time.
- Listener construction names only `NativePlatform` and uses one common startup interface.
- `AppEvent` has one opaque platform event variant and contains no public `rdev` or `tray-icon`
  payload type.
- The application receives only Hold press/release, Toggle, and Exit semantic actions from the
  platform runtime.
- Tray/icon creation, native click handling, template/color policy, and double-click timing are
  hidden behind the common interface.
- Hotkey support tables, AltGr handling, macOS modifier normalization, and synthetic paste
  suppression are hidden behind the selected backend.
- Text delivery is requested through `TextTyper`; the application does not construct or name a
  platform writer or hotkey filter.
- The platform runtime remains main-thread owned while only the thread-safe text writer handle is
  used by finalization workers.
- macOS Accessibility/paste behavior, Windows `SendInput`, hotkey validation errors/warnings,
  icons, tray actions, configuration paths, and recording lifecycle behavior remain unchanged.
- Fake-backend contract tests, retained platform tests, formatting, Clippy, build, diff checks, and
  both hosted OS CI jobs pass.
- Manual macOS and Windows smoke-test results are recorded before the PR is marked ready.
- Architecture docs, plan index/status, project structure guidance, and changelog match the final
  implementation.
