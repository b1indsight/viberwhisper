# 32 - Transcription History

## Status

**Approved and implemented on draft PR #102; interactive native validation remains.** The user
requested simplification after the first implementation became disproportionate to the feature.
The implemented store supersedes the document schema described below: `history.jsonl` contains
one chronological JSON record per line, with exact text and `created_at_unix_ms` metadata. Normal
saves validate only the trailing typed record and append one line. Invalid trailing JSON or
metadata is repaired by truncating that line only. Startup reads only the newest five records in
reverse; later successful saves send only the new text to the tray's bounded in-memory cache.
Crossing the 5 MiB cap is the only normal path that rewrites the newest complete suffix through a
temporary file.

Other material simplifications: there is no schema wrapper, companion lock, clipboard-writer
object, or menu-rebuild state machine. The single running listener owns the file; five fixed menu
slots update in place and raw menu IDs return to the main thread. Labels use standard Unicode
characters instead of a new grapheme dependency. Windows calls `CF_UNICODETEXT` directly with the
required ownership guards. The original approved design below remains a decision record where this
status section does not explicitly supersede it.

## Context

ViberWhisper currently finalizes a non-empty transcription, optionally post-processes it, and asks
the selected platform typer to deliver it to the focused application. The final text has no durable
application-owned recovery path:

- direct macOS Accessibility insertion leaves the clipboard untouched;
- Windows `SendInput` does not retain a copy;
- macOS native paste leaves one transcript on the clipboard, but the next clipboard change replaces
  it; and
- the tray's right-click menu contains only title, status, and exit items.

The requested feature adds a bounded local text history. The five newest entries must be directly
available from the tray menu, long entries must not widen the menu without limit, selecting an
entry must copy its complete text to the system clipboard, and the serialized history file must
never exceed 5 MiB.

## Goals

1. Persist every non-empty text selected for final delivery, including post-processed full results
   and the non-empty partial text already delivered after a partial failure or convergence timeout.
2. Keep persistence independent of automatic insertion success so history remains a recovery path
   when native delivery fails or cannot confirm destination consumption.
3. Show at most the five newest history entries directly in the tray's right-click menu, newest
   first, and refresh those entries without restarting the application.
4. Keep each menu label single-line and visibly bounded while preserving the exact full text behind
   the menu action.
5. Copy the selected entry byte-for-byte as Unicode text to the macOS or Windows clipboard.
6. Store history atomically in the platform application-data directory and keep the complete JSON
   file at or below 5 MiB by evicting the oldest entries.
7. Treat history I/O and clipboard failures as recoverable operational errors that do not prevent
   transcription delivery or terminate the listener.

## Non-goals

- Adding a history window, editor, search, pagination, delete/clear action, or CLI command.
- Making the menu expose more than the five newest entries; older retained entries remain available
  only in the serialized file for this feature.
- Persisting audio, raw chunks, STT requests/responses, API credentials, errors, target application
  metadata, or pre/post-processing variants of the same delivery.
- Encrypting the history file, synchronizing it to another device, or adding a configurable
  retention/display limit.
- Changing recording controls, transcription/chunking policy, post-processing policy, automatic
  text injection, or the existing macOS native-paste clipboard behavior.
- Treating a tray copy as text insertion or emitting a synthetic paste shortcut after copying.

## Design

### 1. Persist one canonical text-history document

Add a focused `history` module that owns a `HistoryStore` and a strict versioned JSON document:

```json
{
  "schema_version": 1,
  "entries": [
    "newest complete transcription",
    "older complete transcription"
  ]
}
```

Entries are stored newest first, so order does not depend on wall-clock timestamps and no unused
metadata is collected. The stored text is the exact non-empty string selected for delivery:

- a successful session records the post-processed result, or the original STT result when
  post-processing returns empty or fails;
- partial failure and convergence timeout record the same non-empty `partial_text` currently sent
  to the typer; and
- empty results, sessions with no chunks, routing failures, and finalizations cancelled before the
  publication checkpoint create no entry.

One finalization appends at most one entry. Persistence runs immediately before the existing typer
attempt after the cancellation checkpoint. A persistence error is logged and delivery continues;
a typer error does not remove a successfully persisted entry. This deliberately makes history a
recovery surface rather than evidence that the focused control consumed the text. If shutdown races
with work that already passed the final publication checkpoint, the already-started persistence and
delivery may finish under the event loop's existing non-blocking cancellation contract.

The store uses the same platform application-data directory as `config.json`, with a separate
`history.json` file:

- macOS: `~/Library/Application Support/com.b1indsight.viberwhisper/history.json`;
- Windows: `%APPDATA%\ViberWhisper\history.json`; and
- unsupported development targets: the existing fallback application directory.

The document contains plaintext transcription text because the requested local history must be
human-readable and directly recoverable. Documentation will call out that privacy boundary.

### 2. Enforce the 5 MiB limit on encoded file bytes

`5 MiB` means `5 * 1024 * 1024` bytes for the complete UTF-8 JSON encoding, including schema and
JSON escaping overhead, not the sum of Rust string lengths. On append, the store will:

1. prepend the new complete entry;
2. serialize the complete document in memory;
3. remove the oldest entry and re-serialize while the encoded document exceeds the limit; and
4. atomically replace `history.json` only after the final bytes are within the limit.

If one entry cannot fit even in an otherwise empty document, the append fails with a specific
oversize error. The text is not silently truncated and the previous on-disk history remains
unchanged. This preserves exact copy semantics while honoring the hard file limit.

Writes create the parent directory, write a temporary file beside the destination, flush and
`sync_all`, and persist by rename, mirroring `ConfigStore`'s established crash-safe pattern. The
history store remains separate because malformed user configuration is a startup error, whereas
history is auxiliary and must degrade gracefully.

A missing file loads as empty history. Invalid JSON, an unsupported schema version, or a file over
5 MiB is logged and ignored at listener startup rather than preventing recording. The next
successful append starts from an empty valid document and atomically replaces the unusable file.
The store never silently interprets a malformed document as a partially valid list.

### 3. Publish history changes back to the main-thread tray owner

The finalization worker already reports completion through `EventLoopProxy<AppEvent>`. Extend that
completion payload with an optional snapshot of the five newest strings returned by a successful
history append. The main-thread `ListenerApplication` applies that snapshot to the platform runtime
before completing the matching recording session.

At startup, load the history before entering the winit loop and initialize the tray with the newest
five entries. Native tray objects stay on the event-loop thread; the worker moves only owned
strings through the existing application-event boundary. No global mutex, polling timer, filesystem
watcher, or second worker pool is introduced.

If startup loading or an append fails, the tray keeps its last valid in-memory snapshot and the
listener continues. An event sent after shutdown remains harmless under the existing closed-loop
behavior.

### 4. Render five bounded, direct menu entries

`TrayManager` retains the menu plus five stable `MenuItem` slots and their IDs between the status
and exit sections. They appear directly in the first-level right-click menu under a disabled
`最近识别` label; there is no submenu. Refreshing updates and inserts only the occupied slots and
removes the unused slots through `muda`'s common menu API. With no history, the first slot is the
single disabled `暂无识别历史` row and the other four are absent. A slot is enabled only while its ID
has a corresponding full string.

For each visible entry, a pure label formatter will:

1. collapse all whitespace runs, including newlines and tabs, to one ASCII space;
2. escape menu mnemonic markers so transcript ampersands render literally;
3. retain at most 40 Unicode grapheme clusters, without splitting an emoji or combining sequence;
4. append `…` only when content was omitted; and
5. use a fixed fallback such as `（空白）` if normalization produces an empty label.

The 40-cluster value is a fixed UI policy, not a configuration field. Only the label is normalized
and shortened. `TrayManager` keeps the full unmodified text associated with each stable item ID, so
selecting an abbreviated multiline/CJK/emoji item yields the exact stored string.

The new history event maps to `TrayAction::CopyHistory(String)` and then to the opaque platform
action boundary. Existing own-ID filtering, left-click recording toggle, double-click debounce,
status changes, and Exit behavior remain unchanged.

### 5. Copy through the selected platform backend

Add a small platform-private clipboard-writer contract owned by `PlatformRuntime`. The application
handles `CopyHistory(full_text)` on the event-loop thread by asking the runtime to replace the
system clipboard with that text:

- macOS reuses the existing AppKit string replacement primitive without posting Cmd+V or entering
  the synthetic-hotkey suppression scope;
- Windows uses the native Unicode clipboard contract and retains ownership/error handling inside a
  Windows-only adapter; and
- the unsupported fallback logs the copy through a mock adapter for development/tests.

Clipboard success and failure are logged without transcription contents. Failure leaves recording
and tray handling operational. Copying history does not require macOS Accessibility permission,
does not change focus, and does not automatically paste into the active application.

## Data and Error Matrix

| Condition | History result | Delivery/menu result |
|---|---|---|
| Missing `history.json` | Start with empty document | Show one disabled empty-state row |
| Valid history with more than five entries | Load all within 5 MiB | Show only newest five |
| Invalid schema/JSON or startup file over 5 MiB | Warn and ignore document | Start with empty menu; listener continues |
| Full non-empty finalized text | Append exact selected text | Attempt existing automatic delivery |
| Non-empty partial text | Append exact partial text | Attempt existing partial delivery |
| Empty/error/cancelled before publication | Do not append | Preserve existing behavior |
| Append exceeds 5 MiB | Evict oldest until it fits | Refresh newest five from persisted result |
| One new entry alone exceeds 5 MiB | Keep previous file; warn | Still attempt automatic delivery |
| History write fails | Keep previous valid snapshot; warn | Still attempt automatic delivery |
| Automatic injection fails | Keep persisted entry | Log existing injection error; history remains copyable |
| History menu item selected | No file mutation | Replace clipboard with exact full text |
| Clipboard write fails | No file mutation | Warn; listener and menu remain active |

## File Impact

| File | Planned change |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Add a narrowly scoped Unicode grapheme dependency for safe menu-label shortening. |
| `src/lib.rs` | Register the private `history` module. |
| `src/history.rs` | Add the versioned JSON model, load/append policy, exact 5 MiB enforcement, atomic persistence, recent-five projection, and deterministic tests. |
| `src/application/listener.rs` | Discover/load history, initialize the platform's recent entries, and pass the store into listener ownership. |
| `src/application/listener/event_loop.rs` | Persist each final delivery candidate off the main thread, forward successful recent-five snapshots, refresh the tray, and keep failures non-fatal. |
| `src/input.rs`, `src/input/clipboard.rs` | Define the narrow clipboard writer contract and fallback test implementation. |
| `src/input/tray.rs` | Add five stable recent-entry slots, bounded label formatting, full-text ID mapping, and copy actions without changing recording/exit handling. |
| `src/platform/backend.rs`, `src/platform/runtime.rs`, `src/platform.rs` | Construct the selected clipboard adapter, expose history refresh/copy through the opaque platform boundary, and carry the non-`Copy` history action. |
| `src/platform/macos.rs`, `src/platform/macos/pasteboard.rs` | Supply AppKit clipboard replacement for menu copy while retaining existing paste/hotkey behavior. |
| `src/platform/windows.rs`, `src/platform/windows/clipboard.rs` | Supply a native Unicode clipboard adapter without changing `SendInput`. |
| `src/platform/fallback.rs` | Supply the development/test clipboard adapter. |
| `README.md` | Document persistent history, the recent-five menu workflow, exact copy behavior, file path/limit, and plaintext privacy boundary. |
| `AGENTS.md` | Add history to the project overview, user flow, and source layout. |
| `docs/architecture/history.md` | Document history ownership, schema, capacity/eviction, error recovery, and finalization-to-tray flow. |
| `docs/architecture/input.md` | Document recent menu slots, truncation, ID filtering, and copy action behavior. |
| `docs/architecture/platform.md` | Document the selected clipboard capability and main-thread copy boundary. |
| `docs/README.md` | Index the new architecture document and track this plan's status. |
| `docs/plan/32-transcription-history.md` | Record approval, implementation status, and material deviations. |
| `changelog` | Record persistent transcription history and tray-to-clipboard recovery. |

`config.example.json`, the v2 configuration schema, CLI docs, audio/transcriber/post-process
architecture, packaging, and release documentation are not expected to change. Display count,
label length, file name, and capacity are fixed internal policies, so no new configuration field or
example value is introduced.

## Test Strategy

Implementation starts with failing tests at the behavior boundaries.

### History store

- missing history loads as an empty versioned document;
- Unicode, multiline, quotes, backslashes, and repeated identical entries round-trip exactly and
  retain newest-first order;
- an append produces an on-disk encoding at or below 5 MiB and evicts only the oldest entries;
- JSON escaping overhead participates in the capacity decision;
- a single oversize entry leaves the previous file byte-for-byte unchanged;
- malformed, unsupported-version, and pre-existing oversize documents produce explicit load errors
  suitable for non-fatal startup recovery; and
- successful writes can be reloaded after the temporary file is persisted.

### Finalization and event flow

- a processed full result records exactly the text passed to `TextTyper` and publishes its newest
  five entries;
- post-processing fallback records the original STT text once;
- partial failure/timeout records the same non-empty partial text passed to the typer;
- empty, routing-failed, no-chunk, and pre-publication-cancelled finalizations record nothing;
- persistence failure does not suppress the typer attempt; and
- a matching completion refreshes history before the session returns to Idle, while stale/closed
  events cannot mutate native tray state.

### Tray and clipboard policy

- zero through six stored strings project to an empty-state row or at most five newest selectable
  slots in the expected order;
- multiline whitespace is collapsed, long ASCII/CJK/emoji text is shortened at a grapheme
  boundary, an ellipsis appears only on truncation, and ampersands render literally;
- selecting a shortened item produces `CopyHistory` with the exact full original string;
- unrelated menu IDs never copy, and history clicks do not toggle recording or map to Exit;
- a fake platform clipboard receives the exact selected Unicode text once and reports failures
  without changing session state; and
- the macOS named-pasteboard test continues to cover exact native string replacement without
  mutating the user's general clipboard.

### Validation

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

Interactive checks on packaged or native development builds:

1. Produce six short transcripts and verify the right-click menu immediately shows only the newest
   five in newest-first order while recording left click and Exit still work.
2. Restart the app and verify those five entries reload from `history.jsonl`.
3. Record multiline, CJK, emoji, and long text; verify the menu stays bounded and selecting its
   abbreviated label places the exact full text on the clipboard.
4. Verify history copy on both macOS and Windows without automatically pasting or changing focus.
5. On macOS, verify a direct AX insertion is recoverable from history without changing the
   clipboard until the history item is selected.
6. Inspect the serialized file after capacity tests and confirm its actual byte size never exceeds
   5 MiB and its oldest records were evicted first.

Hosted macOS and Windows CI must pass on the code-bearing revision.

## Implementation Order

After explicit plan approval:

1. Add failing `HistoryStore` tests for schema, round-trip fidelity, capacity eviction, oversize
   rejection, and invalid-file recovery; then implement the private history module.
2. Add failing pure tray-projection tests, then implement five stable recent-entry slots and exact
   full-text action mapping.
3. Add failing platform-runtime tests for exact clipboard copy, then add the selected macOS,
   Windows, and fallback clipboard adapters.
4. Add failing finalization/event tests, then persist delivery candidates in the worker and refresh
   the main-thread tray through the existing winit event boundary.
5. Run focused tests throughout, followed by the complete formatting/test/check/Clippy/build and
   Windows cross-target matrix.
6. Synchronize README, AGENTS, history/input/platform architecture docs, plan status/index, and
   changelog with the implemented behavior and privacy/error boundaries.
7. Run the workflow's independent code-review gate, push the same
   `feat/transcription-history` bookmark and draft PR, inspect hosted CI, complete the interactive
   macOS/Windows matrix, then mark the PR ready.

## Acceptance Criteria

- Every non-empty full or partial text selected for delivery is persisted exactly once before the
  automatic typer attempt; delivery failure does not remove it.
- `history.jsonl` is appended beside `config.json`; every line is a typed record with timestamp
  metadata, and its complete encoded size never exceeds 5 MiB after an application write.
- Startup and append validate only the last record; malformed JSON or metadata is repaired by
  deleting that record without rewriting older records.
- Capacity pressure removes oldest complete entries, never truncates entry text, and a single
  oversize entry cannot replace a valid previous history file.
- Missing/invalid/unwritable history and clipboard failures do not stop recording, transcription,
  post-processing, automatic delivery, tray control, or application shutdown.
- The first-level right-click menu exposes no more than the five newest entries in newest-first
  order and refreshes after each successful append and across restarts.
- Long or multiline labels are bounded, single-line, Unicode-safe summaries; clicking any summary
  copies the exact full stored string.
- Menu copy works on macOS and Windows without Accessibility permission, synthetic paste, or focus
  changes.
- Existing hold/toggle/tray recording, debounce, status, Exit, macOS injection, Windows
  `SendInput`, and finalization cancellation contracts remain intact.
- Automated tests, local validation, hosted CI, documentation synchronization, and manual native
  checks complete before the draft PR becomes ready.
