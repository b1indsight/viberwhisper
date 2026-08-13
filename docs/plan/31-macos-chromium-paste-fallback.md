# 31 - macOS Chromium Paste Fallback

## Status

**Implemented and validated.** The user approved the plan and its
hidden-web-tree secure-field boundary after reviewing draft PR #101. The implementation identifies
the approved Chromium-family bundle IDs, validates exposed AX focus only for trust and secure
controls, accepts browser-only `NoFocusedElement`, and always posts the existing recoverable native
paste instead of assigning browser `AXSelectedText`.

Focused regression tests and the complete 121-test Rust suite pass, as do formatting, native
`cargo check`, Clippy with warnings denied, native build, the Windows GNU target check, and diff
validation. `Cargo.lock` did not change. The existing pasteboard implementation required no code
change. The independent review gate returned no findings, hosted macOS, Windows, and Python CI all
passed, and the user confirmed the interactive result before PR readiness.

## Context

PR #99 replaced AppleScript delivery with focused Accessibility (`AX`) selected-text assignment
and a native AppKit/CoreGraphics paste fallback. PR #100 kept that behavior behind the
compile-time-selected platform interface.

The direct route works in TextEdit and Chromium browser chrome such as the address bar. Chromium
web content exposes two different failure modes:

- with renderer accessibility inactive, the system-wide `AXFocusedUIElement` lookup returns no
  focused element even though a DOM editor owns keyboard focus; and
- with renderer accessibility forced on, `AXSelectedText` assignment can return synchronous
  success after queuing an asynchronous renderer action without the page changing.

The second case currently produces a false `text inserted through Accessibility` log and returns
success without putting the transcription on the clipboard. The user cannot recover it from the
delivery path.

Chromium accepts `AXEnhancedUserInterface` requests, but current macOS Chromium debounces the
request for two seconds and then enables complete, process-wide screen-reader accessibility. That
builds and maintains web accessibility trees, consumes additional CPU and memory, affects every
visible WebContents, and relies on an undocumented attribute. This fix will not activate it.

Typeless provides the product-level reference for recovery: automatic insertion is best-effort,
while a transcript remains copyable and can be recovered from local history. ViberWhisper does not
have a transcript-card/history UI, so this change uses the existing intentional clipboard policy
as the recovery boundary: a browser paste attempt leaves the exact transcription available for a
manual Cmd+V.

Primary references:

- [Chromium macOS `AXEnhancedUserInterface` handling](https://chromium.googlesource.com/chromium/src/+/master/chrome/browser/chrome_browser_application_mac.mm)
- [Chromium accessibility modes](https://chromium.googlesource.com/chromium/src/+/HEAD/content/browser/accessibility/README.md)
- [Chromium cached accessibility-tree cost](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/accessibility/browser/how_a11y_works_2.md)
- [Typeless transcript recovery](https://www.typeless.com/help/troubleshooting/missing-transcript)

## Goals

1. Insert into Chromium web editors without activating renderer accessibility.
2. Never treat Chromium `AXSelectedText` acceptance as proof that a web editor changed.
3. Preserve direct AX insertion for ordinary native controls, where success is synchronous and the
   clipboard should remain untouched.
4. Leave the transcription on the clipboard whenever native paste is selected, so failed or
   unobservable delivery is manually recoverable.
5. Preserve Accessibility trust checks, secure-control rejection when AX exposes the control,
   synthetic-hotkey filtering, the common `TextTyper` contract, and Windows behavior.
6. Describe a paste command as posted/attempted rather than claiming the destination consumed it.

## Non-goals

- Setting `AXEnhancedUserInterface`, launching Chromium with renderer-accessibility flags, or
  keeping a browser accessibility tree alive.
- Reading the browser DOM, using a browser extension, Chrome DevTools Protocol, AppleScript, or an
  input method.
- Adding transcript history, a floating transcript card, notifications, or a new configuration
  option.
- Proving that the target consumed a CoreGraphics Cmd+V event; CoreGraphics provides no such
  acknowledgement.
- Treating every Electron application as a browser. This fix targets identified Chromium-family
  browsers and retains the existing AX capability fallback for other applications.
- Changing Windows `SendInput`, hotkey semantics, tray behavior, recording, transcription, or
  post-processing.

## Design

### 1. Classify the frontmost application without reading web accessibility

Add a private `src/platform/macos/application.rs` helper backed by
`NSWorkspace.frontmostApplication` and `NSRunningApplication.bundleIdentifier`. It classifies only
known Chromium-family browser bundle identifiers:

- Google Chrome stable/beta/dev/canary;
- Chromium;
- Microsoft Edge stable/beta/dev/canary;
- Brave stable/beta/nightly;
- Arc;
- Vivaldi; and
- Opera.

Exact identifiers and intentional variant prefixes live in one constant table/predicate. A
lookalike identifier must not match. Missing application metadata returns `Other`, retaining the
existing conservative behavior.

This requires only the generated `objc2-app-kit` `NSWorkspace` and `NSRunningApplication`
features. It adds no handwritten Objective-C declarations, subprocesses, permissions, or runtime
dependencies.

### 2. Use a browser-specific validation and paste route

Keep route selection inside `src/platform/macos.rs` behind private testable traits. For non-empty
text:

1. classify the frontmost application;
2. if it is not a known Chromium browser, retain the existing AX-first policy unchanged;
3. if it is a known Chromium browser, require Accessibility trust and attempt to inspect the
   focused AX element only for destination safety;
4. reject `AXSecureTextField` when a focused element is exposed;
5. accept the expected `NoFocusedElement` result for that browser, because a DOM editor can still
   own keyboard focus while Chromium hides its web AX tree; and
6. write the transcription to the pasteboard and post the existing filtered Cmd+V sequence.

The browser path does not call `AXUIElementSetAttributeValue` at all. This is true even when
renderer accessibility or VoiceOver has made the DOM control visible, avoiding the observed
asynchronous false-success path. Browser chrome such as the address bar also uses native paste;
that path is already verified to accept Cmd+V.

Permission denial, an exposed secure control, invalid AX values, messaging failures, and unexpected
types remain hard errors. Only `NoFocusedElement` is reclassified, and only while a known Chromium
browser is frontmost.

### 3. Make the secure-field boundary explicit

When Chromium exposes a focused AX element, the existing `AXSecureTextField` rejection remains in
force. When Chromium hides the entire focused web subtree, macOS does not provide enough
information to distinguish a normal DOM editor from a password editor without activating browser
accessibility.

For the hidden-tree case, this plan chooses browser compatibility: the app performs the same
clipboard replacement and Cmd+V that the user could perform manually. It does not read page
content. The transcription remains on the clipboard whether or not the page accepts it.

This narrowly supersedes plan 29's rule that `NoFocusedElement` always prevents paste. It does not
weaken the rule for native applications, unidentified applications, permission failures, or secure
controls that AX actually exposes.

### 4. Report attempt semantics honestly

The paste transaction can prove that AppKit accepted the transcription and that CoreGraphics
events were constructed and posted. It cannot prove the focused application consumed them.

The existing successful browser/fallback log will therefore say that native paste was posted and
the transcription remains on the clipboard. Direct native AX insertion may continue to report
`inserted through Accessibility`. Errors after the pasteboard write continue to return an error,
while the transcription remains available for manual paste.

No public result type changes: `TextTyper::type_text` still returns `Ok(())` when the selected
native delivery mechanism completes its observable work.

## Error and Route Matrix

| Frontmost destination / condition | Result |
|---|---|
| Empty text | Success without delay or native calls |
| Accessibility permission missing | Hard error; clipboard untouched |
| Native focused, non-secure, AX selected text succeeds | Direct AX success; clipboard untouched |
| Native selected text unsupported/non-settable | Existing native paste fallback |
| Native or unidentified application has no focused AX element | Hard error; clipboard untouched |
| Known Chromium browser, focused AX element exposed | Reject if secure; otherwise native paste |
| Known Chromium browser, no focused AX element | Native paste; text remains manually recoverable |
| Chromium `AXSelectedText` would synchronously accept an async renderer action | Not called |
| Pasteboard write or event construction fails | Error; if written, transcription remains on clipboard |
| CoreGraphics events are posted | Success with attempt-oriented log; no target-consumption claim |

## File Impact

| File | Planned change |
|---|---|
| `Cargo.toml` | Enable the generated AppKit features needed for frontmost bundle identification. |
| `src/platform/macos.rs` | Add application-aware route policy, honest outcome logging, private seams, and regression tests. |
| `src/platform/macos/application.rs` | Identify known Chromium-family browsers from the frontmost bundle identifier. |
| `src/platform/macos/accessibility.rs` | Separate browser-paste validation from direct selected-text assignment while sharing trust, focus, and secure checks. |
| `src/platform/macos/pasteboard.rs` | Retain the current clipboard replacement, CoreGraphics Cmd+V, and hotkey suppression implementation; no code change was required. |
| `README.md`, `AGENTS.md` | Describe the browser paste route, recovery clipboard, and focused security boundary. |
| `docs/architecture/platform.md` | Document application classification, native/browser routing, errors, and Windows non-change. |
| `docs/architecture/input.md` | Keep the paste-filter description aligned with the expanded browser route. |
| `docs/README.md` | Index this plan and update its status after implementation. |
| `docs/plan/29-native-macos-text-injection.md` | Record that plan 31 supersedes the blanket no-focus rule for identified Chromium browsers. |
| `docs/plan/31-macos-chromium-paste-fallback.md` | Record implementation status and any approved deviations. |
| `changelog` | Record Chromium web-field delivery and manual clipboard recovery. |

No configuration, examples, schemas, Windows modules, packaging, or release documentation changes
are expected. `Cargo.lock` should remain unchanged because only features of an existing dependency
are enabled; implementation will verify that assumption.

## Test Strategy

Implementation is test-first at the policy boundaries.

### Application classification

Table-driven unit tests prove that supported stable/preview Chromium identifiers match and similar
or unrelated identifiers do not. Missing bundle metadata remains `Other`.

### Route regression tests

Private fakes prove the smallest scenarios that reproduce the browser regression:

- a known Chromium browser uses paste without calling direct selected-text assignment;
- `NoFocusedElement` is accepted only for a known Chromium browser and posts paste exactly once;
- an exposed secure Chromium control never writes the clipboard or posts paste;
- permission and unexpected AX failures remain hard errors;
- a non-browser `NoFocusedElement` remains a hard error;
- native AX success remains clipboard-free;
- existing unsupported native controls still paste exactly once with byte-for-byte text; and
- empty text remains a complete no-op.

Regression tests will comment both triggering mechanisms: Chromium can hide the focused DOM node,
and exposed `AXSelectedText` success can represent only asynchronous action dispatch.

### Native and cross-platform validation

Before PR readiness, run:

```text
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build
cargo check --target x86_64-pc-windows-gnu
git diff --check
```

Interactive macOS checks:

1. Chrome launched normally, focused HTML `input`, `textarea`, and contenteditable: text appears;
   the clipboard contains the transcription.
2. Chrome with renderer accessibility already active: the same fields use paste and do not emit a
   false AX-success log.
3. Chrome address bar: text appears through paste and remains on the clipboard.
4. TextEdit caret and selection: direct AX insertion/replacement succeeds without changing the
   clipboard.
5. A native secure field and a Chromium secure field exposed through AX: delivery is rejected and
   the clipboard is untouched.
6. With Chrome's web AX tree hidden, acknowledge the documented limitation that password-vs-normal
   DOM focus cannot be distinguished; verify only on a disposable non-secret test field.
7. Configure `V`, `LEFTMETA`, and `RIGHTMETA` as hotkeys in turn and confirm the synthetic paste
   sequence does not trigger recording.
8. Revoke Accessibility permission and confirm delivery fails rather than silently posting paste.

Hosted macOS and Windows CI must pass on the code-bearing revision. The Windows target check proves
the compile-time platform facade and `SendInput` implementation remain unaffected.

## Implementation Order

1. Add failing classifier and route-policy regression tests.
2. Add frontmost Chromium classification using generated AppKit bindings.
3. Split AX browser validation from direct selected-text assignment and implement the narrow
   browser no-focus exception.
4. Update logs to describe posted paste rather than confirmed target delivery.
5. Run focused tests, then the complete local validation matrix.
6. Synchronize README, AGENTS, architecture docs, plan 29, this plan's status/index, and changelog
   with the implemented behavior.
7. Run the workflow's code-review gate, push the same `fix/macos-chromium-paste` bookmark, inspect
   hosted CI, and perform the interactive macOS matrix before marking the PR ready.

## Acceptance Criteria

- Chromium web editors receive transcription without `AXEnhancedUserInterface` or launch flags.
- The browser route never calls `AXSelectedText` assignment and never logs AX insertion success.
- Identified Chromium browsers can paste when macOS reports no focused AX element.
- Native TextEdit insertion/replacement remains direct and clipboard-free.
- Missing Accessibility permission and exposed secure controls still fail without clipboard
  mutation.
- Every native paste attempt leaves the exact transcription on the clipboard for manual recovery.
- Logs distinguish direct insertion from a posted, unacknowledged paste command.
- Existing synthetic-hotkey suppression, the common platform interface, and Windows delivery stay
  unchanged.
- Automated tests, local validation, hosted CI, documentation synchronization, and the manual
  macOS matrix complete before the draft PR becomes ready.
