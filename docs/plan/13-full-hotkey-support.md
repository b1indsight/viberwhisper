# 13 - Named Single-Key Hotkey Support

## Status

Implemented. Local macOS tests, formatting, strict Clippy, Windows-target check/Clippy, and GitHub
macOS/Windows/Python CI pass. Hardware-level right-Alt smoke tests remain for the maintainer because
this implementation environment cannot generate interactive physical keyboard input; the required
macOS startup-held/overlap and Windows AltGr cases are listed below. The design and implementation
were reviewed against the current `HotkeyConfig`, `EventMapper`, listener integration,
configuration persistence, and `rdev 0.5.3` platform backends. It replaces the earlier, removed
design for modifier combinations and raw platform key codes.

## Goal

Expand `input.hold_hotkey` and `input.toggle_hotkey` from `F1`–`F12` to the stable named keyboard
keys exposed by `rdev 0.5.3`, while retaining the existing single-key recording semantics. The
primary requested binding is a standalone right Alt key:

```bash
viberwhisper config set input.hold_hotkey RIGHTALT
```

On macOS, `RIGHTALT` means the physical right Option key. On Windows, it means the physical right
Alt key, which the operating system may also use as AltGr for character input.

## Scope

This feature adds named **single keyboard keys** only. It does not expose modifier combinations,
mouse-button bindings, media keys, configurable raw platform key codes, or interception of the configured key.
The existing configuration fields, empty-string disabling, Hold/Toggle behavior, duplicate
validation, defaults (`F8` and `F9`), and configuration schema version remain unchanged.

The parser will support these canonical names:

| Group | Canonical names |
|---|---|
| Function | `F1`–`F12` |
| Letters | `A`–`Z` |
| Number row | `0`–`9` |
| Editing and whitespace | `BACKSPACE`, `DELETE`, `INSERT`, `ENTER`, `SPACE`, `TAB`, `ESCAPE` |
| Navigation | `UP`, `DOWN`, `LEFT`, `RIGHT`, `HOME`, `END`, `PAGEUP`, `PAGEDOWN` |
| Modifiers | `LEFTALT`, `RIGHTALT`, `LEFTCTRL`, `RIGHTCTRL`, `LEFTSHIFT`, `RIGHTSHIFT`, `LEFTMETA`, `RIGHTMETA` |
| Locks and system | `CAPSLOCK`, `NUMLOCK`, `SCROLLLOCK`, `PRINTSCREEN`, `PAUSE`, `FUNCTION` |
| Punctuation | `BACKQUOTE`, `MINUS`, `EQUAL`, `LEFTBRACKET`, `RIGHTBRACKET`, `SEMICOLON`, `QUOTE`, `BACKSLASH`, `INTLBACKSLASH`, `COMMA`, `DOT`, `SLASH` |
| Numeric keypad | `NUMPAD0`–`NUMPAD9`, `NUMPADENTER`, `NUMPADMINUS`, `NUMPADPLUS`, `NUMPADMULTIPLY`, `NUMPADDIVIDE`, `NUMPADDELETE` |

Names are ASCII case-insensitive and continue to ignore outer whitespace. A small alias set makes
platform terminology discoverable without creating multiple behaviors:

| Canonical name | Accepted aliases |
|---|---|
| `RIGHTALT` | `ALTGR`, `RIGHTOPTION` |
| `LEFTALT` | `ALT`, `LEFTOPTION`, `OPTION` |
| `ENTER` | `RETURN` |
| `ESCAPE` | `ESC` |
| `LEFTMETA` | `COMMAND`, `WIN`, `SUPER` |
| `UP`, `DOWN`, `LEFT`, `RIGHT` | `UPARROW`, `DOWNARROW`, `LEFTARROW`, `RIGHTARROW` |
| `NUMPAD*` | corresponding `KP*` spelling |

`LEFTALT` maps to `rdev::Key::Alt`; `RIGHTALT` maps to `rdev::Key::AltGr`. Names identify
`rdev::Key` values rather than produced characters. The macOS backend maps hardware key codes, but
the Windows backend maps virtual-key values, so letter and punctuation bindings may follow the
active Windows layout instead of fixed physical QWERTY positions. Number-row and numeric-keypad
keys remain distinct where the backend exposes that distinction.

## Platform Boundaries

The supported vocabulary follows named `rdev::Key` variants rather than undocumented numeric key
codes. A key is usable only when the operating system, keyboard, and `rdev` backend report that
named variant. Inspection of the current `rdev 0.5.3` backend tables gives these explicit
exceptions:

| Platform | Names rejected because `rdev` does not emit a distinct named key |
|---|---|
| macOS | `CAPSLOCK`, `RIGHTCTRL`, `DELETE`, `INSERT`, `HOME`, `END`, `PAGEUP`, `PAGEDOWN`, `NUMLOCK`, `SCROLLLOCK`, `PRINTSCREEN`, `PAUSE`, `INTLBACKSLASH`, and all `NUMPAD*` names |
| Windows | `RIGHTMETA`, `FUNCTION`, and `NUMPADENTER` |

Those names stay in the shared canonical vocabulary for configurations moved between platforms,
but validation rejects them on the affected platform with an actionable error. In particular:

- `RIGHTALT` is first-class on both supported platforms (`AltGr` in `rdev`; right Option on macOS).
- `rdev 0.5.3` can misclassify macOS modifier direction when the listener starts with a modifier
  held or matching left/right modifiers overlap because it compares aggregate flags. The listener
  normalizes modifier events from Core Graphics physical key state before `EventMapper`; ordinary
  key repeat suppression is unchanged.
- macOS reports Caps Lock as an on/off status change rather than one physical press followed by one
  release. That cannot satisfy the existing Hold or Toggle repeat-suppression contract, so
  `CAPSLOCK` is rejected on macOS rather than given mode-dependent behavior.
- `FUNCTION` is macOS-only.
- `NUMPADENTER` is not distinguishable from `ENTER` by the Windows backend.
- Numeric-keypad reporting on Windows depends on Num Lock, as documented by `rdev`.
- On Windows layouts with AltGr, the operating system exposes right Alt as left Ctrl plus right Alt.
  `rdev::listen` keeps only the virtual-key identity and cannot identify the accompanying left-Ctrl
  event as part of AltGr. A `RIGHTALT` binding still matches its `Key::AltGr` event correctly, but a
  standalone `LEFTCTRL` binding can also fire when the user presses AltGr. Startup and user
  documentation must warn about this limitation; no reliable filter can be added at the current
  `rdev::Event` boundary. Validation rejects a configuration that assigns `LEFTCTRL` and
  `RIGHTALT` to the two modes in either order, preventing one AltGr press from enqueuing both
  actions.
- Specialized, media, and extended function keys that arrive as `Key::Unknown` remain out of
  scope. The feature does not hard-code platform scan codes.

If implementation or CI discovers another named variant that the backend cannot emit on a
supported platform, it must receive the same explicit validation and documentation treatment; it
must not be silently accepted as a non-working binding.

## Key-Passthrough Safety

`rdev::listen` observes global input but does not consume it, so the configured key still reaches
the focused application. This is especially important for bare letters, numbers, punctuation,
editing keys, and Alt/Option:

- Configuration accepts them because standalone keys are the requested capability.
- Startup logs one warning for each enabled binding that is likely to type, edit, navigate, or
  invoke an operating-system/application action.
- User documentation recommends spare function/modifier keys such as `RIGHTALT` and explains that
  Windows AltGr may participate in localized character entry.

No key suppression is introduced; suppression would require a platform-specific input-grab design
with materially different permissions and failure modes.

## Technical Approach

### `src/input/hotkey.rs`

1. Keep `HotkeyConfig`, `HotkeyEvent`, `HotkeySource`, the listener thread, and `EventMapper`
   single-key behavior intact.
2. Preserve the public `parse_key(&str) -> Option<rdev::Key>` contract and replace only its
   `F1`–`F12` match with an explicit, auditable name/alias lookup. Small private helpers provide the
   canonical label, platform availability, and passthrough-risk classification; do not introduce a
   generic registry or new module for a fixed key vocabulary.
3. Store canonical runtime labels in `HotkeyConfig` rather than uppercasing the user's alias. The
   listener's startup text, registration logs, and heartbeat therefore consistently show
   `RIGHTALT`, including when the user entered `altgr` or `rightoption`. Persisted configuration and
   `config get`/`config list` continue to show the exact user-entered string, matching the current
   persistence contract.
4. Validate platform-limited names in `HotkeyConfig::validate`, before the listener starts. Preserve
   the existing aggregated `ValidationIssue` behavior: use `hotkey.invalid` for unknown names,
   `hotkey.unsupported` for a known name unavailable on the current platform, and
   `hotkey.duplicate` for aliases that resolve to the same `rdev::Key`. On Windows, use
   `hotkey.altgr_conflict` to reject a `LEFTCTRL`/`RIGHTALT` pair.
5. Emit passthrough and Windows `LEFTCTRL`/AltGr warnings during `HotkeyManager` construction, after
   validation has produced a usable configuration.
6. At the macOS event boundary, replace `rdev`'s modifier direction with the current physical state
   reported by Core Graphics `CGEventSourceKeyState`. This handles both startup-held and overlapping
   modifiers while leaving ordinary key-repeat suppression unchanged.

No new module is needed. The macOS target adds a direct, narrow-feature
`objc2-core-graphics` dependency for `CGEventSourceKeyState`; the implementation remains in the
existing input module rather than adding an indirection that only forwards key lookup.

### Configuration and Runtime Assembly

`InputSection`, `ConfigKey`, `runtime_config`, and persistence continue to carry the two bindings as
strings; no migration or schema bump is planned. `config set` currently persists string fields
without resolving the whole listener configuration, and this feature preserves that behavior.
Hotkey validity is checked by the existing `runtime_config::check` / `resolve_listener` path, so an
unsupported value is reported by `viberwhisper config check` and listener startup rather than by
`config set`. This avoids coupling field persistence to API credentials, profiles, or other runtime
validation.

## Test-First Implementation

Tests are written before the parser and validation implementation:

1. Table-driven parsing coverage for every canonical name, including all letters, digits,
   punctuation, keypad keys, and the existing `F1`–`F12` compatibility set.
2. Table-driven alias coverage, with explicit assertions that `RIGHTALT`, `ALTGR`, and
   `RIGHTOPTION` all map to `Key::AltGr` while `LEFTALT` maps to `Key::Alt`.
3. Case-insensitivity, outer-whitespace handling, empty disabling, invalid-name errors, and the
   complete cfg-gated platform rejection matrix, including macOS `CAPSLOCK`.
4. Duplicate-binding validation across canonical names and aliases, proving `RIGHTALT` conflicts
   with `altgr`.
5. Windows validation rejects `LEFTCTRL` and `RIGHTALT` as the two bindings in either order, while
   retaining the warning for a standalone `LEFTCTRL` binding.
6. Focused canonical runtime-label, passthrough-risk, and Windows `LEFTCTRL`/AltGr warning
   classification tests where these protect behavior; avoid duplicating the exhaustive parser
   table in separate tests.
7. Retain the existing event-order and key-repeat test, adding one representative right-Alt Hold
   press/release case to prove a modifier used alone follows ordinary single-key semantics.
8. Add macOS normalization coverage proving physical key state corrects both misreported modifier
   directions, including the startup-held release case, without querying ordinary keys.

Manual smoke testing after implementation:

1. macOS: bind Hold to `RIGHTALT`, press/release right Option, and verify exactly one recording
   session without triggering from left Option. Repeat while left Option remains held and verify
   releasing right Option still ends the session. Start the listener while right Option is held,
   release it, and verify that release does not start recording.
2. Windows: repeat with right Alt and verify left Alt does not trigger it; also check expected AltGr
   interaction on a keyboard layout that uses AltGr when available. With `LEFTCTRL` configured by
   itself, confirm/document whether the same AltGr press triggers it, matching the known backend
   limitation; confirm a `LEFTCTRL`/`RIGHTALT` pair is rejected by `config check`.
3. Configure a representative navigation key and a printable key, verifying Hold/Toggle behavior
   and the documented passthrough warning.
4. Confirm existing F8/F9 defaults behave unchanged.

## Documentation Impact

During implementation:

- Update `README.md` with the supported single-key groups, the `RIGHTALT` example, key-identity and
  passthrough warnings, Windows `LEFTCTRL`/AltGr behavior, the `config set` versus `config check`
  boundary, and the platform-limited names.
- Update `docs/architecture/input.md` so the parser contract, canonical-label behavior, validation,
  and unchanged event mapping match the code.
- Append a user-visible entry to `changelog`.
- Keep `config.example.json` unchanged because its canonical F8/F9 defaults remain correct.
- Update this plan's status and record any material platform deviations discovered during manual or
  CI validation.

This plan is indexed in `docs/README.md` during Planning. No current-truth documentation will claim
the feature is available until the implementation is present.

## Validation

- `cargo fmt --check`
- `cargo test input::hotkey`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- GitHub macOS and Windows CI
- Manual right-Alt smoke tests on both supported platforms when hardware/platform access is
  available; otherwise the PR handoff must call out which manual checks remain for the maintainer.

## Acceptance Criteria

1. Both hotkey fields accept every documented, platform-available named single key while existing
   F1–F12 values and empty-string disabling remain compatible.
2. `RIGHTALT` works as an independent Hold or Toggle binding, maps to right Option on macOS and
   right Alt/AltGr on Windows, and remains distinct from `LEFTALT`.
3. Canonical names and aliases normalize consistently for runtime display and duplicate validation;
   persisted configuration continues to preserve the user's input.
4. Platform-unavailable bindings fail configuration validation with an actionable error instead of
   silently never firing.
5. Hold release, Toggle press-only behavior, event ordering, and ordinary-key repeat suppression are
   unchanged for all added keys; startup-held and overlapping macOS modifier events use current
   physical key state rather than the backend's ambiguous direction.
6. Passthrough, platform/layout key-identity, and Windows `LEFTCTRL`/AltGr behavior is visible in
   validation errors, startup warnings, and user documentation; Windows rejects assigning
   `LEFTCTRL` and `RIGHTALT` to the two modes together.
7. Automated validation and available cross-platform smoke tests pass, with any unavailable manual
   verification explicitly recorded before the PR is marked ready.
