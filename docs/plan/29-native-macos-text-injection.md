# 29 - Native macOS Text Injection

## Status

**Implementation revised again; local automated validation complete.** The native Accessibility route,
AppKit and CoreGraphics fallback, callback-based hotkey suppression, clipboard-replacement policy,
focused tests, and current-truth documentation are implemented on PR #99. Formatting, the complete
Rust suite, checks, Clippy, build, Windows cross-check, diff validation, the native pasteboard test,
and Python tests pass. Hosted macOS, Windows, and Python CI also pass on code-bearing revision
`40d8d8650f94`. The interactive application matrix below remains before PR readiness.

Plan 31 narrowly supersedes this plan's blanket rule that `NoFocusedElement` must always prevent
paste. Identified Chromium-family browsers now use native paste without assigning
`AXSelectedText`; `NoFocusedElement` is accepted only for those bundles because Chromium can keep
keyboard focus in a DOM editor while hiding it from the macOS AX tree. All other applications
retain this plan's original error policy.

The generated `objc2-application-services` crate does not expose Apple's macro-defined AX
attribute/subrole constants as Rust statics. The implementation therefore passes their documented
string values through `CFString`; all functions and owned native types still use generated
bindings, with no handwritten extern declarations.

The user explicitly superseded clipboard-preservation goals 3 and 4 and the corresponding design,
file-impact, test, and acceptance sections retained below as approved-plan history. Restoring a
clipboard is not part of the desired paste operation. The fallback now clears the general
pasteboard, writes the transcription as a native string, posts Cmd+V, and leaves that transcription
on the clipboard. It never reads or materializes the previous items, uses no `changeCount`
ownership token, and performs no restore.

The approved design placed acknowledged suppression state and an RAII guard directly in
`src/input/hotkey.rs`. The first implementation moved that protocol into platform-owned
`hotkey.rs` and `injection.rs` helpers. After implementation review, the user approved a smaller
boundary: `start_hotkey_listener` now accepts an `EventType -> Option<EventType>` callback, and
`NativePasteWriter` owns the atomic flag shared with its callback. `None` drops an event from
ViberWhisper and resets mapper bookkeeping. The fixed paste transaction raises the flag while it
constructs and posts Cmd+V and waits through a 100 ms synthetic-event filter grace; an RAII scope
clears it on all exits. CoreGraphics posting now lives in `pasteboard.rs`, and the forwarding delivery trait,
standalone `keyboard.rs`, coordinator, sequence matching, generation, condition variable, and
listener acknowledgement were removed. This intentionally trades the original acknowledgement
error signal for a direct filter boundary: whether ViberWhisper observes its own key events does
not determine whether the focused application received the CoreGraphics paste.

## Context

At planning time, `MacTyper::type_text` waited 100 ms for focus to settle, interpolated the
transcription into AppleScript, launched `osascript`, replaced the general clipboard, and asked
`System Events` to press Cmd+V. That path had four material problems:

- the previous clipboard contents are never restored;
- AppleScript escaping is not a safe arbitrary-text transport;
- each transcription starts a subprocess and reports failure only after it exits; and
- synthetic Command/V events are visible to the process-wide `rdev` listener, so single-key
  bindings such as `V`, `LEFTMETA`, or `RIGHTMETA` can trigger ViberWhisper itself.

PR #67 added named single-key hotkeys but deliberately did not add input grabbing or injection
suppression. PR #98 then moved `TextTyper::type_text` onto a background finalization worker. This
feature therefore needs both a native macOS injection implementation and a narrow cross-thread
contract that lets the existing hotkey listener ignore only ViberWhisper's paste sequence without
blocking the winit event loop.

Apple documents `kAXSelectedTextAttribute` as the editable text selection attribute and exposes
settable checks through `AXUIElementIsAttributeSettable`. `NSPasteboard.changeCount` is the
ownership token for deciding whether clipboard contents can still be restored. The implementation
will use the generated `objc2` framework crates for both APIs instead of adding handwritten
Objective-C declarations.

Primary API references:

- [Apple: `kAXSelectedTextAttribute`](https://developer.apple.com/documentation/applicationservices/kaxselectedtextattribute)
- [Apple: `AXUIElementSetAttributeValue`](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue)
- [Apple: `NSPasteboard.changeCount`](https://developer.apple.com/documentation/appkit/nspasteboard/changecount)
- [Apple: `NSPasteboardItem`](https://developer.apple.com/documentation/appkit/nspasteboarditem)
- [`objc2-application-services` 0.3.2 bindings](https://docs.rs/objc2-application-services/0.3.2/objc2_application_services/)

## Goals

1. Insert directly through the focused Accessibility element when selected text is writable.
2. Fall back to a native pasteboard transaction plus a CoreGraphics Cmd+V sequence for controls
   that do not support direct selected-text insertion.
3. Preserve every pasteboard item and every eagerly readable representation when the application
   still owns the clipboard after paste delivery.
4. Never restore stale clipboard data over a concurrent user or application change.
5. Prevent the fallback's synthetic `V`, `LEFTMETA`, and `RIGHTMETA` events from producing
   ViberWhisper hotkey events or leaving the hotkey mapper in a stale key-down state.
6. Reject secure text controls and genuine Accessibility failures without silently replacing an
   entire field or pasting into an unknown target.
7. Preserve the shared `TextTyper` interface, Windows `SendInput` behavior, the event-driven UI
   loop, and current recording-session semantics.

## Non-goals

- Changing the Windows text injector.
- Replacing `rdev`, introducing an input grab, or suppressing events in the focused application.
- Using `kAXValueAttribute`, which could replace the entire control rather than its selection.
- Implementing an InputMethodKit input source.
- Using `CGEventKeyboardSetUnicodeString` as the primary insertion path.
- Adding configuration fields, a selectable injection mode, or user-tunable timing knobs.
- Claiming that every third-party editor implements the macOS Accessibility text contract
  correctly; unsupported controls use the bounded paste fallback.

## Design

### 1. Keep one `MacTyper` coordinator with focused native helpers

`MacTyper` will become a constructed value rather than a zero-sized type. It will own:

- a cloneable handle to the hotkey suppression state shared with the `rdev` callback; and
- a mutex that serializes access to the process-global general pasteboard if `TextTyper` is ever
  called concurrently.

The platform implementation will be split into three private helpers under `src/platform/macos/`:

```text
macos.rs                  route selection, focus delay, error policy, logging
macos/accessibility.rs    focused AX element lookup and selected-text insertion
macos/pasteboard.rs       deep snapshot, temporary text ownership, conditional restore
macos/keyboard.rs         CoreGraphics Cmd+V construction and posting
```

Native Objective-C/Core Foundation values are created within the call and are not stored in
`MacTyper`. The background finalization thread will enter an Objective-C autorelease pool around the
native operation. `MacTyper` remains `Send + Sync`, and no AppKit object is sent through winit.

`type_text("")` remains a no-op. Non-empty input retains the existing fixed 100 ms focus-settling
delay before either AX access or hotkey suppression begins.

### 2. Prefer Accessibility selected-text insertion

The AX adapter will:

1. require the current process to be a trusted Accessibility client;
2. create the system-wide AX element;
3. resolve `kAXFocusedUIElementAttribute` and require a concrete focused target;
4. read `kAXSubroleAttribute` when available and reject
   `kAXSecureTextFieldSubrole` as a protected destination;
5. ask whether `kAXSelectedTextAttribute` is settable; and
6. set that attribute to a `CFString` containing the transcription.

Setting selected text gives both required direct behaviors: an empty selection inserts at the
caret, and a non-empty selection is replaced. The implementation will not read or set
`kAXValueAttribute`.

The adapter returns a small private outcome rather than flattening all AX results into one error:

```text
Inserted
Unsupported(reason)
SecureControl
Failed(ax_error)
```

Only a present, non-secure focused element whose selected-text capability is absent, explicitly
non-settable, or returns the documented unsupported/no-value/not-implemented result selects the
paste fallback. Failure to identify a focused target, missing Accessibility trust, invalid AX
objects, messaging failure, illegal arguments, and type mismatches are hard Accessibility errors.
This keeps unsupported editors usable without turning genuine AX failures into an unobservable
paste attempt.

Successful direct insertion returns immediately. It does not access the clipboard, construct
keyboard events, or enter hotkey suppression.

### 3. Deep-snapshot all pasteboard items before taking ownership

The fallback must not retain and later rewrite the `NSPasteboardItem` values returned by
`pasteboardItems`: AppKit associates those objects with the current owner and documents them as
stale after ownership changes. Instead, the pasteboard adapter will eagerly copy a neutral snapshot:

```text
PasteboardSnapshot
  items[]
    representations[]
      type identifier string
      owned data bytes
```

For every current item, it enumerates every advertised type and calls `dataForType`. Item order,
type order, and the exact representation bytes are preserved. If any advertised representation
cannot be read, snapshotting fails before the pasteboard is changed; a partial snapshot is never
used. An empty pasteboard is represented explicitly.

The adapter records `changeCount` before and after snapshotting. If ownership changes during the
snapshot, fallback aborts without writing, rather than racing a newly copied value. No automatic
retry is needed because a user-driven clipboard change is not an internal transient failure.

After a stable snapshot:

1. clear the general pasteboard;
2. write the transcription as `NSPasteboardTypeString`;
3. verify the native write succeeded; and
4. record the resulting `changeCount` as ViberWhisper's ownership token.

Plain text, multiline content, quotes, backslashes, CJK, and emoji travel as native strings; no
escaping or shell representation is involved.

### 4. Post Cmd+V under acknowledged hotkey suppression

The fallback will create one CoreGraphics event source and post this fixed virtual-key sequence to
the HID event tap:

1. left Command down (virtual key 55);
2. V down (virtual key 9, Command flag set);
3. V up (virtual key 9, Command flag set); and
4. left Command up (virtual key 55).

The sequence is one internal constant used by both the native poster and suppression expectation so
the two cannot drift. Failure to create the source or any event aborts delivery and enters the same
clipboard-restoration path as an acknowledgement failure. `CGEventPost` itself returns no status;
failure to observe the sequence within the bounded acknowledgement window is the delivery failure
signal.

`src/input/hotkey.rs` will add a cloneable suppression handle backed by a mutex, condition variable,
monotonic epoch, and the expected injected key sequence. Starting a paste returns an RAII guard.
While that guard is active, the existing rdev callback will:

- normalize macOS modifier direction from physical key state before inspecting the rdev event;
- withhold all hotkey notifications during the short injection window;
- advance only when it sees the expected Command/V sequence in order; and
- signal the guard after the final Command-up has passed through the listener.

Unrelated physical input during the window is ignored by ViberWhisper but does not advance the
expected sequence. It still reaches the focused application because this is mapper suppression,
not an operating-system input grab.

`EventMapper` gains a narrow `reset` operation. The suppression epoch advances both when the guard
starts and when it is released; the callback resets hold/toggle key-down bookkeeping on each
observed transition. A release missed during suppression therefore cannot leave the next physical
press suppressed, and an injected press cannot become remembered mapper state.

After posting, the fallback waits up to a fixed internal 500 ms for rdev to acknowledge the exact
sequence. The value is a safety bound, not configuration. Timeout is a paste-injection error. The
guard's `Drop` releases suppression and advances the epoch on success, timeout, event-construction
failure, or panic unwinding. Tests use direct state transitions and controlled threads rather than
sleeping for this production timeout.

Once the listener acknowledgement arrives, a fixed 100 ms background-worker grace period allows
the target application to consume the paste before clipboard restoration. This grace is outside
the suppression guard: only synthetic-event production and observation suppress ViberWhisper
hotkeys. The winit UI thread remains unblocked.

### 5. Restore only while ViberWhisper still owns the pasteboard

After the paste-consumption grace period, the adapter compares the current `changeCount` with the
token recorded after writing the transcription:

- if they match, it creates fresh `NSPasteboardItem` objects from the snapshot, clears the board,
  and writes the complete ordered item array;
- if they differ, another owner won the race, so restoration is skipped and the new clipboard is
  left untouched.

The same conditional restoration runs after any failure that occurs after ViberWhisper took
ownership. Event injection remains the primary result:

- write, event, or acknowledgement failure returns a typed paste error after best-effort restore;
- a successful paste followed by restoration failure is logged as a distinct warning and still
  returns success, because the requested text was already delivered;
- a concurrent ownership change is an informational skip, not a restoration error; and
- restoration failure never triggers a second paste or overwrites a newer owner.

The private error variants and log messages will distinguish Accessibility failure, secure-control
rejection, pasteboard snapshot/write failure, CoreGraphics event-construction failure, suppression
acknowledgement timeout, and clipboard restoration failure. The public `TextTyper` signature
remains unchanged.

### 6. Assemble one shared suppression state

`start_hotkey_listener` will create the suppression state used by its callback and return a handle
to the application assembly. On macOS, `run_with_config` moves that handle into
`MacTyper::new(...)`; other platforms retain their current typer behavior and do not enter
suppression because they do not synthesize this fallback sequence.

The handle has real runtime state and acknowledgement semantics, so it does not reintroduce the
ownership-free hotkey manager removed by plan 28. The recording state machine remains unaware of
injection events. During finalization it is already in `Stopping`, and any unrelated user input
that reaches the application boundary remains subject to the existing state transitions.

## Error and Fallback Matrix

| Condition | Result |
|---|---|
| Empty text | Success without delay or native calls |
| Accessibility permission missing | Hard AX error; no clipboard write |
| No focused AX element | Hard AX error; no paste into an unknown target |
| Secure AX subrole | Hard secure-control error; no fallback |
| Selected text unsupported, absent, or non-settable | Native paste fallback |
| AX messaging/invalid-element/type failure | Hard AX error |
| AX selected-text set succeeds | Success; clipboard and keyboard untouched |
| Pasteboard snapshot is incomplete or changes while read | Paste error; clipboard untouched |
| Temporary text write fails | Paste error; conditionally restore snapshot |
| CoreGraphics event construction or suppression acknowledgement fails | Paste error; release guard and conditionally restore |
| Clipboard owner unchanged after successful paste | Restore all items/representations |
| Clipboard owner changed after write | Skip restoration and preserve new contents |
| Paste succeeds but restoration fails | Log restoration warning; return delivery success |

## File Impact

| File | Planned change |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Add narrowly featured macOS `objc2-application-services`, `objc2-core-foundation`, and `objc2-foundation` bindings; enable AppKit pasteboard item features. |
| `src/application/listener.rs` | Share the hotkey suppression handle with constructed `MacTyper`. |
| `src/input/hotkey.rs` | Add suppression state/guard, sequence acknowledgement, epoch reconciliation, and mapper reset tests. |
| `src/platform/macos.rs` | Replace `osascript` with the two-tier coordinator and typed route/error logging. |
| `src/platform/macos/accessibility.rs` | Add focused-element, secure-subrole, settable, and selected-text AX adapter. |
| `src/platform/macos/pasteboard.rs` | Add deep snapshot, temporary text ownership, change-count validation, and fresh-item restoration. |
| `src/platform/macos/keyboard.rs` | Add CoreGraphics Cmd+V event construction/posting. |
| `AGENTS.md`, `README.md` | Replace current osascript/clipboard descriptions with the implemented native behavior and permission boundary. |
| `docs/architecture/platform.md` | Document direct AX insertion, paste transaction ownership, errors, and Windows non-change. |
| `docs/architecture/input.md` | Document shared injection suppression and mapper-state reconciliation. |
| `docs/README.md` | Keep the plan index/status and platform-document description synchronized. |
| `changelog` | Record native macOS insertion and clipboard preservation after implementation. |
| `docs/plan/29-native-macos-text-injection.md` | Record implementation status and material deviations without rewriting the approved design. |

No configuration schema or example changes are planned because the strategy is fixed internal
macOS behavior. Core, audio, transcriber, post-processing, local-service, Windows platform, and
release contracts are unchanged.

## Test Strategy

Implementation is test-first at each boundary. Tests should protect behavior and concurrency
contracts rather than reproduce framework getters and setters.

### Route and error policy

Use private fake AX and paste adapters to prove:

- direct AX success never snapshots the clipboard or emits key events;
- unsupported/non-settable selected text chooses paste exactly once;
- secure targets and genuine AX failures never fall back;
- empty text is a complete no-op; and
- text is forwarded exactly for multiline, quotes/backslashes, CJK, and emoji cases.

### Hotkey suppression

Drive the reducer/mapper directly to prove:

- the Command-down, V-down, V-up, Command-up sequence produces no Hold/Toggle event when any of
  `V`, `LEFTMETA`, or `RIGHTMETA` is configured;
- unrelated events cannot falsely complete the expected injected sequence;
- entry and exit epochs reset both hold and toggle down-state;
- normal mapping resumes after completion; and
- dropping the guard on every simulated error path releases suppression.

Threaded acknowledgement coverage will use channels/barriers and a short injected test deadline,
not wall-clock sleeps.

### Pasteboard transaction

Use a deterministic fake pasteboard containing multiple items and multiple binary representations
to prove:

- every item/type/byte payload is restored in order while ownership is unchanged;
- a change during snapshot aborts before clear/write;
- a change after ViberWhisper's write skips restoration;
- snapshot, text-write, event, acknowledgement, and restore failures retain their documented
  primary result; and
- restoration failure after successful delivery is reported separately from delivery success.

The native AppKit adapter will have a focused macOS-only test with an isolated uniquely named
pasteboard where practical; tests will not mutate the user's general pasteboard or post real global
keys.

### Native and cross-platform validation

Run:

```bash
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked -- -D warnings
cargo build --locked
cargo check --locked --target x86_64-pc-windows-msvc
git diff --check
```

Hosted macOS and Windows CI must pass. Windows validation confirms that target-specific bindings and
the constructed macOS typer do not alter `SendInput` compilation or behavior.

## Manual macOS Verification

With Accessibility permission granted, verify all of the following before PR readiness:

1. A native AppKit `NSTextField` or equivalent controlled test field inserts at an empty caret and
   replaces an active selection through AX, without changing the clipboard.
2. A browser/Electron editor that lacks settable selected text uses fallback and receives the text.
3. Terminal.app (and iTerm2 when available) receives fallback text at the active prompt.
4. Plain text, multiline content, quotes, backslashes, CJK, and emoji arrive unchanged.
5. A clipboard containing multiple items/representations is restored byte-for-byte after fallback.
6. Changing the clipboard concurrently during the transaction leaves the newer contents intact.
7. A secure/password field fails without text delivery or clipboard replacement.
8. Separate runs with Hold/Toggle bound to `V`, `LEFTMETA`, and `RIGHTMETA` show no recording
   transition from injected Cmd+V, and the next physical press/release still works normally.
9. AX failure and fallback/restore outcomes are distinguishable in logs, and no `osascript` process
   is launched.

## Implementation Order

1. Add failing hotkey suppression and mapper reconciliation tests, then implement the shared
   state, RAII guard, and acknowledgement path.
2. Add failing coordinator tests for AX success, fallback eligibility, secure rejection, and hard
   errors; introduce the private route/result model.
3. Add failing pasteboard transaction tests for complete restoration, concurrent ownership change,
   and restoration failure; implement the neutral snapshot/restore policy.
4. Add the native AX, AppKit pasteboard, and CoreGraphics adapters with narrowly enabled bindings.
5. Construct `MacTyper` with the listener's suppression handle and remove all AppleScript/process
   invocation code.
6. Run focused and full automated validation, then update current-truth documentation and record
   any implementation deviations in this plan.
7. Perform the manual macOS matrix, run the repository's independent code-review gate, push the
   implementation on this same bookmark/PR, and mark the PR ready only after the gate and required
   validation complete.

## Acceptance Criteria

- Supported editable AX controls insert at an empty caret and replace an active selection through
  `kAXSelectedTextAttribute`.
- Unsupported/non-settable selected text selects native paste fallback; genuine AX and secure-field
  failures do not.
- No implementation path reads or writes `kAXValueAttribute`, builds AppleScript, or launches
  `osascript`.
- AX success neither reads/writes the clipboard nor emits keyboard events.
- Fallback round-trips every eagerly readable item and representation while ownership is unchanged.
- A concurrent clipboard owner is never overwritten by restoration.
- Plain, multiline, quoted, backslashed, CJK, and emoji text is preserved.
- Every success/failure/unwind path releases hotkey suppression and reconciles mapper state.
- Fallback cannot emit ViberWhisper events for `V`, `LEFTMETA`, or `RIGHTMETA` bindings.
- Paste delivery, AX failure, secure rejection, concurrent-owner skip, and restoration failure are
  distinguishable in logs.
- `TextTyper` and Windows `SendInput` contracts remain unchanged.
- Focused tests, the full suite, formatting, Clippy, build, diff checks, Windows cross-check, and
  hosted CI pass.
- The manual AppKit, browser/Electron, terminal, clipboard, secure-field, and hotkey matrix is
  completed before PR readiness.
- `AGENTS.md`, README, platform/input architecture docs, plan status/index, and changelog match the
  implemented behavior.
