# Core session interface simplification

## Scope and approval

Implement the eight items agreed during the core review, excluding configuration.
The user approved the individual decisions and explicitly requested their implementation
on 2026-09-07. This document records that approved scope for the implementation PR.

Keep the current lifecycle states, session routing, bounded worker queue, transcription strategy,
convergence deadline, partial results, and dynamic transcriber injection.

## Design

1. `RecordingSessionMachine` owns ID allocation. Pass a `SessionId` value into `transition` and
   advance the machine's counter only when an `Idle` to `Starting` transition is accepted.
   Rejected events do not consume IDs, and failed starts do not allow ID reuse.
2. Replace the single-variant `SessionStartError` enum with a struct exposing `requested` and
   `active`. Preserve its display text and error implementation, and simplify the listener caller.
3. Rename `ActiveSessionInner.chunks` to `chunk_entries`, including local variables and helper
   parameters that refer to tracking records rather than WAV data.
4. In `on_chunk_ready`, try sending first and then append one entry with the resulting `Flushed`
   or `Failed` state. Failed submissions still participate in the final failure report.
5. Treat `on_chunk_ready` as a notification returning `()`. Reject invalid session routing with
   internal logging, retain submission failures in the session, and report final outcomes through
   `finish_session`. Remove the listener's duplicate error logging and unused return-value handling.
6. Pass language configuration by value as `Option<String>` through result collection and shared
   text merging. Clone when retaining configuration for later work; update the offline conversion,
   setup verification, and prompt-lab merge callers consistently.
7. Inline `drain_worker_events` into its sole caller after the entry is registered.
8. Inline `begin_upload` and `record_worker_result` into `apply_worker_event`. Keep the event handler
   shared between recording and finalization. Preserve `Flushed`-only upload transitions, terminal
   result protection, and existing diagnostic logging.

Function extraction follows `code-principles`: keep meaningful responsibility boundaries and
shared state-update rules; remove wrappers that add navigation without reducing complexity.
The length of `finish_session` alone is not a reason to split it.

## Files and implementation order

1. Adapt existing lifecycle, submission, routing, and text-merge tests to the approved contracts.
   Add only missing behavior coverage that protects ID allocation or terminal-state handling.
2. Update `src/core/recording_session.rs` and `src/core/orchestrator.rs`.
3. Update `src/application/listener/event_loop.rs`, `src/text.rs`, `src/application.rs`,
   `src/application/setup.rs`, and `src/prompt_lab/regression.rs` for changed interfaces.
4. Update `docs/architecture/core.md`, `changelog`, and this plan's implementation status.
5. Run local validation, the independent code-review gate, and the same PR's platform CI.

## Validation

Reuse existing coverage for lifecycle completion, failed starts, stale events, queued transcription,
partial failures, queue saturation, timeout, worker panic, and session-routing rejection.
Submission tests should assert retained state/final results rather than removed return values.
Verify language-aware merging still preserves Chinese concatenation and non-Chinese spacing.

- `cargo test --locked core::`
- `cargo fmt --check`
- `cargo test --locked`
- `cargo clippy --locked -- -D warnings`
- GitHub CI for macOS and Windows, including the Windows GUI feature checks.

## Progress

All eight review items are implemented. The updated tests first exposed the old error, transition,
and language signatures, then passed after implementation.

- Targeted core tests: 52 passed.
- Full macOS suite: 187 passed, including language merging and prompt-lab regressions.
- Formatting, Clippy with warnings denied, and diff whitespace checks passed.
- Independent review and macOS/Windows CI results are tracked on PR #126.
