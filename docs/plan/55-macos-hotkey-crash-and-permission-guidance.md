# 55 - macOS hotkey crash and Accessibility permission guidance

**Stage:** Implementation and validation
**PR:** [#145](https://github.com/b1indsight/viberwhisper/pull/145)

## 1. Initial plan

### Summary and motivation

Fix the macOS keyboard-listener crash and make missing text-delivery permission visible
to desktop users. The user requested these changes after testing v0.4.0 in Ghostty.

Two separate failures were observed on macOS 26.6.2:

- A SIGTRAP crash passes through `_dispatch_assert_queue_fail`,
  `islGetInputSourceListWithAdditions`, `TSMGetInputSourceProperty`,
  `rdev::macos::keyboard::Keyboard::string_from_code`, and the global event callback.
  The listener runs on a background thread. The installed rdev 0.5.3 implementation
  queries input-source properties while deriving a character name from each key event;
  ViberWhisper only consumes `event.event_type`, never the character name.
- Three real recordings produced recognized text and new history entries, followed by
  `Failed to type text error=Accessibility permission is required`. The release app
  normally directs stderr to `/dev/null`, leaving the user without an explanation.

Evidence captured during the 2026-10-12 session:

- Crash report: `~/Library/Logs/DiagnosticReports/viberwhisper-2026-10-12-160201.ips`.
  An earlier `viberwhisper-2026-10-12-154502.ips` has the same failing stack.
- Runtime log: `.cache/2026-10-12/macos-input-launchservices.log`, lines 10–24.
- Existing `cargo test --lib platform::macos -- --nocapture`: 13 tests passed.
  These checks did not reproduce actual desktop key events or permission denial.

Private machine-specific logs and crash reports remain outside version control.

### Goals, scope, and success criteria

1. Desktop hotkey observation on macOS performs no input-source/layout lookup.
2. Hold, toggle, modifier keys, repeat filtering, and synthetic-paste suppression retain
   their behavior. Configured named keys retain their existing platform support.
3. The setup verification helper uses the same safe macOS event implementation.
4. Missing Accessibility permission produces a visible, actionable message, once per
   application launch. This applies both at desktop startup and if permission first
   becomes unavailable during text delivery.
5. Recognition and history remain usable while permission is missing; authorized delivery
   resumes without treating failed output as successful. Denial does not modify the clipboard.

Scope includes macOS listener plumbing, permission guidance, focused regression tests,
architecture documentation, README guidance, and changelog. Windows continues using its
current listener and text-delivery backend. No configuration schema change is required.

### Proposed behavior

On macOS, the app observes physical key-down, key-up, and modifier-state events directly
through a CoreGraphics event tap. Changing between English and Chinese input methods or
pressing ordinary keys does not invoke the crashing character-decoding path.

When desktop startup detects missing Accessibility trust, show a native message explaining:

> ViberWhisper needs Accessibility permission to enter recognized text into other apps.
> Open System Settings → Privacy & Security → Accessibility and enable ViberWhisper.
> If this started after an update, remove the old entry and add the current app again.
> Recognized text remains available in the tray’s recent history for copying.

Offer **Open Settings** and **Later**. Opening settings requires an explicit button click;
the app never changes permission itself. If opening settings fails, retain the instructions
and log the failure. Dismissing the message permits normal recording and history access.

If permission was initially available and a later delivery attempt is denied, show the same
message once. Subsequent denials in that launch only log the typed error. The user can grant
permission and retry; existing AX trust checks continue to run for each delivery attempt.
The message is limited to the recording desktop flow, not config or offline conversion commands.

### Technical design and affected modules

- `src/platform/macos/hotkey.rs` (split a listener submodule if needed): own the macOS
  observe-only CoreGraphics event tap and run-loop lifetime. Decode virtual keycodes into
  the existing `rdev::EventType`/`Key` domain values without constructing rdev `Keyboard`
  or calling TIS/TSM APIs. Maintain a single macOS keycode mapping and existing physical
  modifier normalization. Ignore unrelated mouse events and unsupported keycodes.
  Re-enable a tap disabled by timeout or user input; report creation failures explicitly.
  Keep callback storage alive for the run loop, return events unchanged, and contain
  callback panics at the FFI boundary.
- `src/platform.rs` and platform backend plumbing: provide the narrow platform-selected
  blocking physical-key listener used by desktop and setup. Non-macOS builds adapt
  the existing rdev listener. Avoid macOS-specific branches inside the shared mapper.
- `src/input/hotkey.rs`: retain the `EventMapper`, detached listener lifetime, configured
  binding diagnostics, and filter ordering; consume event types through the selected listener.
- `src/ui/setup/hotkey.rs`: reuse that listener for verification rather than retaining
  a second call to `rdev::listen` on macOS.
- `src/platform/macos/accessibility.rs`, `src/platform/macos.rs`, and the platform facade:
  reuse `AXIsProcessTrusted` and identify permission failures through typed errors, not
  error-message matching. Own the native alert and settings-opening behavior in the
  macOS backend; expose only the needed capability to the desktop UI.
- `src/ui/listener.rs` and `src/ui/listener/event_loop.rs`: check desktop permission on
  the main thread, route worker delivery-denial notifications through the existing
  application event proxy, and keep one per-launch prompt flag. All native alerts run on
  the main thread, outside text-delivery locks. Cover complete and partial delivery paths;
  canceled finalization must not enqueue a new permission prompt.
- `Cargo.toml`: enable only the additional existing objc2 CoreGraphics/CoreFoundation/AppKit
  bindings needed for the tap, run loop, and native alert. rdev remains the shared key type
  and Windows event implementation; no vendored dependency or floating Git dependency.
- `README.md`, `docs/architecture/platform.md`, `docs/architecture/input.md`, and
  `changelog`: describe the macOS listener and permission recovery behavior.

### Alternatives and rationale

- Moving the blocking rdev listener to the main thread would conflict with the existing
  winit/AppKit main loop and keep unnecessary input-source queries on every keystroke.
- Dispatching character decoding to the main queue would preserve work the app does not
  consume and introduce synchronization between the global hook and UI loop.
- Vendoring rdev solely to remove name decoding would add third-party source maintenance.
  A small backend-owned physical-key tap keeps the fix within the module that owns macOS.
- Logging permission denial alone reproduces the observed silent desktop failure.
  A native alert provides instructions without adding a notification authorization flow.

### Costs, risks, and mitigations

The event tap requires careful ownership and callback handling. Keep its lifetime explicit,
use the existing objc2 bindings, and isolate pure keycode/event translation for tests.
Mapping errors could affect less common named keys; validate supported-key coverage and
left/right modifiers against the existing catalog and macOS virtual-key definitions.

A modal alert can move keyboard focus. Show it only once per launch and never automatically
retry or paste the failed text after dismissal; the user can recover text from history.
Accessibility trust and global event-observation permissions can differ, so listener failures
must remain distinct from text-delivery permission errors.

### Implementation order

1. Introduce the macOS physical-key listener and route both desktop and setup through it.
2. Add focused mapping, modifier, filter, and listener-error regression coverage.
3. Add main-thread Accessibility guidance with typed denial routing and prompt deduplication.
4. Verify denial behavior and then granted-permission delivery in Ghostty and TextEdit.
5. Update documentation and changelog, run required checks, and complete code review on
   the same bookmark and PR before marking it ready.

### Validation and ablation strategy

- **Tests:** Exercise ordinary key press/release, auto-repeat, modifiers, supported keycodes,
  tap disable/re-enable handling, paste suppression, and shared setup routing. Verify that
  a denial requests one prompt, repeated denials do not, and unrelated delivery errors
  do not request this prompt. Verify startup denial and later revocation without showing
  real system dialogs in unit tests.
- **Interactive regression:** Use the same installed-version behavior as baseline. For
  the implementation, test real F8/F9 and ordinary typing while switching English/Chinese
  input methods, including after system/input-source changes. Inspect crash reports and
  verify recording continues. Test permission denial and subsequent recovery, then actual
  Chinese text insertion in isolated Ghostty and TextEdit windows without executing it.
  Obtain user cooperation where permissions or physical key events cannot be automated.
- **Ablation:** Compare the old character-decoding listener with the physical-key listener
  using the same key/input-source sequence; confirm the new path never invokes TIS/TSM.
  Compare denied delivery with and without guidance to prove the visible message while
  preserving the same history and clipboard behavior. Do not claim intermittent-crash
  absence from a short interactive run alone; verify structural removal of the failing API path.
- **Timing:** Run comparisons after implementation and before automated code review.
- **Checks:** `cargo fmt --check`, focused tests, `cargo test`, `cargo clippy --all-targets
  -- -D warnings`, native `cargo check`, available Windows target check, and diff validation.

### Open questions and deferred work

No blocking product questions. Exact native tap/run-loop binding details and the supported
settings deep link are implementation-time checks. Realtime configuration UI, automatic
permission changes, and a new transcript display are outside this PR.

## 2. PR change timeline

### [001] — 2026-10-12 — Narrow the crash fix to skipping unused character decoding

- **Context / requirement update:** The user asked whether the app can simply skip
  input-source/layout lookup instead of replacing the macOS listener. Inspection of
  rdev 0.5.3 confirms that `common::convert` first constructs `EventType` from the
  virtual keycode, then independently computes `Event.name` through
  `Keyboard::create_string_for_key`. Both desktop and setup only use `event_type`.
  The public `listen` API and Cargo features provide no switch to omit name decoding.
- **Options considered:** The baseline backend-owned listener avoids dependency
  maintenance but introduces a new event tap, run-loop ownership, and key mapping.
  A pinned local dependency patch preserves the existing tap and mapping while
  removing just the unused character-name calculation. Editing Cargo's global
  registry cache would not produce a reproducible build and is excluded.
- **Decision / status:** Proposed: replace the baseline listener rewrite with a
  narrowly patched, locally vendored rdev 0.5.3 dependency. In its macOS event
  conversion, set `Event.name` to `None` without querying input-source properties.
  Keep the physical-key event mapping, repeat behavior, modifier normalization,
  event tap lifetime, and Windows implementation. This proposal is pending approval.
- **Consequences:** The semantic dependency diff is small and application listener
  plumbing stays intact, including setup verification. Vendoring adds the upstream
  source snapshot to the repository; the source-file count must not be presented as
  the size of the actual fix. Preserve upstream licenses, document the exact version
  and patch, and use Cargo `[patch.crates-io]` plus the lockfile for reproducibility.
  A future rdev update requires rechecking or retiring the local patch.
- **Related records:** This supersedes the baseline native-listener design,
  listener-specific module refactoring, and its rejection of vendoring. The permission
  alert design, main-thread handling, deduplication, and recovery behavior remain proposed
  as originally described. No new CoreGraphics tap/run-loop bindings are required.
- **Validation adjustment:** Confirm macOS conversion returns unchanged physical key
  events with `name: None` and cannot call character decoding. Cover key-down/up and
  modifiers, retain existing mapper/suppression tests, and repeat the real input-method
  switching and Ghostty/TextEdit tests. Dependency-local tests must be run explicitly
  because a dependency patch may not be included in the root test suite. Compare the
  original and patched dependency under the same recording/key sequence before review.
  Windows remains unpatched; retain the Windows target check.
- **Implemented changes:** Plan amendment only; no product, dependency, or test code changed.
- **Outcome / pending:** Update the same draft PR and await approval of this smaller approach.

### [002] — 2026-10-12 — Implement the dependency patch and permission guidance

- **Context / requirement update:** Implement the approved smaller crash fix and the
  permission message without replacing the native listener or its mapping.
- **Implemented changes:** Pin rdev to 0.5.3 and vendor its published source with the
  upstream MIT license. macOS conversion returns `name: None`; physical event conversion,
  Windows/Linux code, desktop listener plumbing, and setup verification remain intact.
  Retain the unused upstream character helper with a precise dead-code allowance.
  Preserve the standalone lockfile for dependency-local tests. Normalize trailing whitespace
  in two upstream documentation/workflow files for diff checks; document these snapshot
  differences and the original crate checksum in `VIBERWHISPER-PATCH.md`.
- **Permission behavior:** Add macOS-owned `AXIsProcessTrusted` checks and a main-thread
  NSAlert with Chinese instructions, Open Settings, and Later. Only an explicit button
  click opens settings. Desktop startup and typed denial from complete or partial delivery
  use one per-launch guidance flag. Recheck trust when processing queued guidance; skip
  guidance in capture/CLI flows and after cancellation. History and clipboard denial
  behavior stay intact. Documentation and changelog describe recovery after app updates.
- **Tests:** Add background physical-key conversion coverage for A/F8/F9 and modifier
  changes, typed permission classification through anyhow context, per-launch prompt
  deduplication, and suppression for unrelated error messages and canceled delivery.
  macOS CI explicitly runs the dependency-local regression with its lockfile.
- **Ablation evidence:** Run the same isolated conversion regression on an unchanged
  upstream snapshot in `.cache/2026-10-12/rdev-baseline` and the patched dependency.
  With CoreGraphics access allowed for both, upstream fails because the ordinary key
  has `name: Some("a")`; the patch passes with unchanged physical events and no character
  names. The original crash reports establish the historical SIGTRAP; this comparison
  proves omission of decoding, rather than claiming an intermittent crash reproduced
  every time. Sandboxed exploratory runs stalled in system event-service access and
  were stopped; those runs are not counted as crash-fix evidence.
- **Native validation:** An independently signed test bundle was built in the local
  cache without replacing the installed app. The user confirmed the permission popup,
  then confirmed successful Ghostty text input after switching input methods and no
  repeated popup. A sampled test process had a live background rdev run loop and no
  character-decoding stack. Short interactive runs cannot establish long-term crash absence;
  the failing listener API call has also been structurally removed.
- **Checks completed:** `cargo test --offline --quiet` with local loopback access:
  244 passed, 2 private/live-service tests ignored. Native build, `cargo fmt --check`,
  `cargo clippy --offline --all-targets -- -D warnings`, and dependency-local regression pass.
  Upstream rdev retains its existing FFI/static-mut warnings; application lint checks pass.
- **Limitations / pending:** Local Windows GNU checking is blocked by ort-sys's unsupported
  binary target; MSVC cross-checking is blocked by missing native Windows C headers/toolchain.
  The existing native Windows CI will verify that target after publication. TextEdit
  granted-permission insertion and the settings deep-link destination were not separately
  confirmed in this run; Ghostty input and the visible native dialog were user-confirmed.
  Independent code review and hosted CI are pending.
