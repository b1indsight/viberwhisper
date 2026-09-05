# Structured runtime logging cleanup

## Status

Proposed on a draft PR; implementation requires explicit approval.

## Context

The application already initializes `tracing` with an environment filter, and most background
components use structured events. A few listener and capture paths still write operational
messages directly with `println!`, so those messages bypass log filtering and lack structured
fields. At the same time, CLI commands intentionally use stdout for command results such as config
values, JSON reports, paths, status fields, and converted transcription text. Treating those values
as logs would break shell pipelines and the CLI output contract.

## Goals

1. Route remaining runtime diagnostics through `tracing` at an appropriate level.
2. Attach useful fields to capture and file-write events instead of embedding them in multiline
   strings.
3. Preserve stdout for intentional CLI results and keep errors propagated to the caller.
4. Make the diagnostic-versus-command-output boundary explicit enough to prevent future drift.

## Non-goals

- Replacing the existing `tracing` subscriber or changing its default filter.
- Changing command output formats, introducing a quiet/verbose flag, or adding file logging.
- Removing the emergency native/stderr startup-error reporting used when normal logging may be
  unavailable.
- Reworking unrelated error handling or command behavior.

## Output boundary

| Output kind | Destination | Examples |
| --- | --- | --- |
| Runtime lifecycle and diagnostics | `tracing` | listener startup, enabled hotkeys, capture mode, capture completion |
| Recoverable or propagated operational failures | `tracing` plus existing `Result` propagation | output-file write failure |
| Command results intended for people or scripts | stdout | config values, local status, report JSON, converted text and saved path |
| Last-resort desktop startup presentation | platform-native UI or stderr | failure before or around desktop logger availability |

The migration therefore does not mechanically replace every `println!`. It replaces only calls
whose content is an application event. Remaining direct stdout calls are reviewed and retained as
part of the CLI contract.

## File impact

| File | Change |
| --- | --- |
| `src/application/listener.rs` | Replace banner, mode, hotkey, and exit instructions with concise `info!` events and fields. |
| `src/application/listener/event_loop.rs` | Record completed prompt-lab captures as one structured `info!` event. |
| `src/application.rs` | Log output-file write failures before returning them; retain command results on stdout. |
| `docs/README.md` | Index this plan and its current status. |
| `changelog` | Record the completed runtime logging cleanup during implementation. |

## Implementation order

1. Classify every current `println!`/`eprintln!` call as runtime diagnostics, CLI result output, or
   emergency startup presentation.
2. Convert listener startup and capture-completion diagnostics to structured tracing events.
3. Convert the duplicated convert-file failure print to an error event while preserving the
   returned error and successful stdout output.
4. Re-run the call-site inventory, format, lint, and test checks, then update this plan's status and
   the changelog.

## Test strategy

No new unit test should assert formatted log text: the change does not add branching behavior, and
such a test would couple the suite to presentation details. Validation will instead use:

- a source inventory confirming that every remaining direct print is an intentional CLI result or
  emergency fallback;
- `cargo fmt --check`;
- `cargo test`;
- `cargo clippy --all-targets --all-features -- -D warnings` on the native target.

## Acceptance criteria

- Listener lifecycle and prompt-lab capture diagnostics respect the configured tracing filter.
- Structured capture logs expose sample ID, audio path, and sidecar path as fields.
- A convert output-file failure is logged once and still returned to the process boundary.
- CLI data/result output remains on stdout with its existing content and ordering.
- The fallback desktop startup error remains visible even if normal tracing cannot be initialized.
- Formatting, tests, and native Clippy pass.
