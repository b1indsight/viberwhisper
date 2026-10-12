# Persist missing Realtime configuration defaults — Feature record

**Stage:** Review
**PR:** https://github.com/b1indsight/viberwhisper/pull/147

## 1. Initial plan

### Summary and motivation

Existing schema-v3 configuration files remain unchanged after an application upgrade.
Serde supplies missing `dictation` defaults in memory, so the listener can start,
but users cannot discover newly added settings by opening their configuration file.
The reported macOS configuration has a `dictation` section without `api_key_env`
or `memory`. The same persistence behavior affects Windows.

### Goals, scope, and success criteria

After a successful desktop configuration validation, persist missing Realtime
fields into the existing file. Preserve every explicit setting, including `false`,
empty strings, nulls, custom prompts, endpoints, models, and memory entries.
Keep schema version 3 and preserve existing rejection of unsupported schemas and
invalid configuration. Ordinary config inspection commands remain read-only.

### Proposed behavior

A file without `dictation` receives the default section, with `enabled: false`.
A partial section such as `{"enabled":true,"url":"ws://example:8080/v1/realtime"}`
receives its missing model, authentication-variable, prompt, and memory settings;
the user's endpoint and enabled state remain intact. A partial `memory` object
receives missing fields. An already complete file is not rewritten.

Missing `prompt` and `prompt_components` materialize as null, allowing the app to
continue using its bundled defaults. Do not copy bundled prompt prose into the
user file, because doing so would freeze future default improvements.
Explicit custom components remain unchanged. Invalid supplied components are
still invalid; this operation does not repair malformed user data.

### Technical design and affected modules

- `src/core/config/store.rs`: expose an explicit desktop backfill operation,
  restricted to missing keys under `dictation`; obtain defaults from the typed
  configuration serializer and reuse atomic persistence. Preserve other JSON
  sections rather than normalizing the complete document.
- `src/ui/setup.rs`: invoke backfill only after the loaded document successfully
  resolves, before starting desktop recording. New setup already saves a complete
  document. Propagate write failures through the normal startup error path.
- `src/core/config.rs`: regression coverage for whole-section and partial-field
  backfill, preservation, idempotency, and invalid-file handling.
- `README.md` and configuration architecture documentation: describe automatic
  missing-field persistence and preservation of explicit settings.

### Alternatives and rationale

Leaving defaults in memory reproduces the reported discoverability problem.
Rewriting files inside every `load()` would make config inspection unexpectedly
mutate files. Replacing the entire section would erase user configuration.
An explicit startup backfill keeps reads predictable and limits writes to the
requested Realtime upgrade behavior.

### Costs, risks, and mitigations

Startup needs one conditional atomic write for incomplete existing configuration.
Check for missing keys first to avoid repeated writes. Validate before writing;
surface persistence failures and retain the original file on atomic-write failure.
Recheck the source before persistence and avoid overwriting a configuration edited
since it was loaded. JSON formatting may change when the file is updated.

### Implementation order

1. Implement the focused store operation and meaningful preservation tests.
2. Integrate with validated desktop startup and update user documentation.
3. Run targeted tests, formatting, and required cross-platform CI and review.

### Validation and ablation strategy

Tests cover missing `dictation`, missing top-level and memory fields, preservation
of custom values and unrelated sections, complete-file idempotency, read-only
loads, invalid input, and a changed source file. Removing backfill must reproduce
the original missing-field behavior. No optional runtime layer or performance
mechanism is proposed, so no broader ablation experiment is needed.

### Open questions and deferred work

No blocking design questions. Automatic replacement of explicit settings and
migration from older schema versions are outside this change.

## 2. PR change timeline

### 001 — 2026-10-12: Focused startup backfill implemented

The store now exposes `materialize_dictation_defaults` after the caller validates
the loaded document. It adds only absent keys under `dictation`, including nested
memory fields, and shares the existing atomic writer. Null prompt defaults avoid
freezing bundled prose. Explicit values and all unrelated sections remain intact.
Normal loads remain read-only. Validated settings are compared with the source
before backfill, and source bytes are rechecked before atomic persistence to
detect edits during preparation. This is an optimistic check, not a file lock
against edits occurring in the final check-to-replace interval.

The desktop setup path invokes the operation after successful listener resolution.
Three regression tests cover missing sections, nested defaults, explicit values,
unchanged complete files, read-only loads, changed sources, and invalid input.
Targeted startup tests passed (4 tests). Broader validation and review are pending.

### 002 — 2026-10-12: Local validation and backfill comparison

Full tests passed: 247 passed, 2 existing optional tests ignored. Formatting,
Clippy for all targets with warnings denied, and diff whitespace checks passed.
For the planned comparison, temporarily bypassing the backfill operation made
`startup_materializes_missing_dictation_but_load_stays_read_only` fail at the
expected materialization assertion (exit 101). Restoring the operation produced
the passing full suite. This demonstrates the missing-field regression against
the previous read-only behavior. Logs remain in `.cache/2026-10-12/` and are not
committed. Hosted macOS/Windows validation and independent review remain pending.

### 003 — 2026-10-12: Independent review reports a concurrent-write limitation

The independent review passed the critical-only gate and reported one medium
finding at `src/core/config/store.rs:94`: a successful `config set` between the
last source comparison and atomic replacement can still be overwritten by
startup backfill. The optimistic check does not coordinate all writers. The
review recommends a shared lock and a deterministic interleaved-write test.
This limitation remains present; no findings-driven code changes have been
made. Such rework requires a new explicit request under the review skill.
The implementation was pushed at reviewed commit
`5049984deec4c4e344720aca9fe3a9947bc04aed`. Local build also passed; hosted CI is
pending.

### 004 — 2026-10-12: Hosted validation passed

macOS and Windows CI both passed for the pushed implementation and review record:
https://github.com/b1indsight/viberwhisper/actions/runs/38251202081.
The PR is ready for review with the concurrent-write finding from entry 003 still
open. No release has been produced and the installed application is unchanged.

### 005 — 2026-10-12: Remove concurrency handling as requested

The user explicitly requested removal of the low-probability concurrency handling.
This supersedes the initial source-recheck requirement and entry 001's optimistic
checks. Removed both source comparisons, `SourceChanged`, and the concurrent-edit
test. The flow now reads JSON, fills defaults, and atomically saves only on change.
Concurrent writers are not coordinated; no lock is added. Existing-value,
idempotency, read-only-load, and invalid-JSON tests remain.
Validation after removal: 247 tests passed, 2 existing optional tests ignored;
formatting, all-target Clippy with warnings denied, and diff checks passed.
