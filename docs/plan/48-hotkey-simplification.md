# 48 - Hotkey State and Key Catalog Simplification

## Status

Draft — awaiting approval of the four changes below before implementation.

This plan continues on PR #129 and bookmark `refactor/hotkey-warning-inline`.
The previously reviewed passthrough-warning inlining remains in that PR. This
planning update adds documentation only; implementation continues on the same PR.

## Goal

Simplify the hotkey driver while preserving the named-key behavior established by
[plan 13](13-full-hotkey-support.md) and the current
[input architecture](../architecture/input.md).

The current driver wraps callback-owned state in a mutex, extracts a two-assignment
reset into a one-use method, stores each binding's key and display name in separate
optional fields, and converts captured keys to names by formatting their Debug
representation and parsing it again. Remove those four sources of indirection.

## Design

### Callback-owned event state

`rdev 0.5.3` accepts an `FnMut(Event) + 'static` callback on both macOS and Windows.
Move a mutable `EventMapper` directly into the listener closure and mutate it there.
Remove the hotkey driver's `Mutex` import, locking, and poison-recovery branch.
Keep the detached listener thread, platform filter, event notification order, and
process-lifetime shutdown boundary intact.

Inline the two key-down resets into the `None` branch of `map_filtered` and remove
`EventMapper::reset`. A filtered event must still clear both Hold and Toggle state
so a later physical press can be recognized after synthetic paste suppression.

### One optional value per configured binding

Reuse `NamedKey`, which already pairs an `rdev::Key` with an `&'static str`
canonical name. Store `hold: Option<NamedKey>` and `toggle: Option<NamedKey>` in
`HotkeyConfig` instead of four independent key/name options.

Keep the physical key private to the hotkey module. Expose the binding and its
canonical name within the crate as needed by existing listener diagnostics.
Build the runtime config directly from the two validated bindings. Read physical
keys when constructing `EventMapper`; pass a complete binding to warning logging.
This removes label String allocations and checks for mismatched key/name presence.

### One explicit key catalog

Replace the parser's match table and the Debug-based reverse conversion with one
private static catalog in `src/input/hotkey.rs`. Each row contains an `rdev::Key`,
its canonical name, and its accepted aliases. Use plain data and direct iteration;
the small fixed vocabulary does not need a macro, registry, cache, or new dependency.

Forward lookup trims the input and compares canonical names and aliases with
`eq_ignore_ascii_case`. Reverse lookup compares the physical key and returns the
stored canonical name. Change the crate-private `canonical_key_name` return type
to `Option<&'static str>` and update any affected callers and assertions.

Use the current parser as the compatibility inventory: retain every accepted
canonical spelling and alias, including all keypad aliases. Keep `parse_key`'s
public signature and the separation between vocabulary lookup and platform policy.

## Compatibility requirements

- F8/F9 defaults and empty or whitespace-only disabled bindings remain valid.
- Case-insensitive aliases resolve to the same key and canonical runtime label.
- Persisted configuration retains the user's spelling; runtime labels use the
  canonical spelling. Config JSON and CLI input/output contracts stay compatible.
- Unknown names and unlisted captured keys remain rejected. Existing platform
  restrictions, duplicate-binding errors, and Windows LEFTCTRL/RIGHTALT conflicts
  retain their current field attribution and behavior.
- Hold press/release, Toggle press, repeat suppression, and filtered-event reset
  preserve their observable event sequences.
- Passthrough and platform-specific warnings retain their text and conditions.

## Files

| File | Planned change |
| --- | --- |
| `src/input/hotkey.rs` | Callback state, reset inlining, paired bindings, catalog, and existing tests |
| `src/application/listener.rs` | Read canonical names from paired bindings; adapt affected assertions |
| `src/application/setup.rs` and directly affected caller tests | Adapt to borrowed canonical names only where required |
| `docs/architecture/input.md` | Document the catalog, paired bindings, and callback ownership |
| `docs/README.md`, this plan, and `changelog` | Record plan state and completed implementation |

## Implementation order

1. Before changing production code, run the existing hotkey, setup, and platform
   tests. Extend the existing alias coverage for any accepted aliases missing from
   its independent expected cases, and cover both modes after a filtered event.
2. Introduce the catalog and switch forward/reverse lookup while retaining the
   existing independent expected canonical-name cases.
3. Store paired bindings, update diagnostics, and adapt affected call sites.
4. Remove the listener mutex and inline the mapper reset.
5. Update architecture/changelog, run validation, and pass the independent code
   review gate before pushing implementation on the same bookmark and PR.

## Validation and acceptance

Retain the existing canonical-name round-trip tests, alias and whitespace cases,
unknown-key rejection, config validation, repeat suppression, right-Alt distinction,
and setup tests. Expected names must remain independent of the production catalog
so a missing entry cannot silently remove its own test. Add a focused catalog
integrity check for duplicate keys or ambiguous canonical/alias spellings.
Do not add tests for trivial accessors, reset assignments, or log-helper structure.

Run `cargo fmt --check`, `cargo build --locked`, `cargo test --locked`, and
`cargo clippy --locked -- -D warnings` locally on macOS. Use the existing Windows
CI build/test commands with `--features windows-app` and Clippy with
`--all-targets --features windows-app -- -D warnings`. Record native keyboard
verification separately from compilation and unit tests; do not claim a platform
smoke test without actually exercising it.

Completion means all four simplifications are implemented, compatibility coverage
passes, both platform CI jobs pass, and the completed diff has passed the repository
review gate. Keep the same PR open for final implementation review.
