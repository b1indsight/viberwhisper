# Consolidate runtime configuration into core::config

## Status

Plan approved by the user; implementation and local validation are complete on PR #125.
Base: `master` at `bef71a4e` (PR #123).

## Problem and outcome

Configuration handling currently spans `core::config` and the top-level `runtime_config`
module. Callers import document and storage types from one module and workflow configuration
types and resolvers from the other. The user wants these interfaces consolidated directly in
`src/core/config.rs`.

After this change, `core::config` will expose document access, persistence, and workflow
configuration assembly through one module. `src/runtime_config.rs` will be removed.

## Technical approach

- Move `ListenerConfig`, `BackendConfig`, and `ConvertConfig` into `src/core/config.rs`.
- Move the existing `resolve_listener`, `resolve_convert`, and `check` functions there,
  preserving their signatures. Keep `resolve_api_backend`, `effective_secret`,
  `collect_issues`, and `report` private in the same file.
- Update application, setup, listener, and prompt-lab imports to `crate::core::config`.
  Resolver calls become `config::resolve_listener`, `config::resolve_convert`, and
  `config::check`; configuration types are imported from that module.
- Remove the crate-root module declaration and the old source file. The implementation
  lives directly in `config.rs`, without a compatibility facade or new resolver submodule.
- Move the four existing runtime configuration tests into the existing configuration test
  module and reuse its identical `MapSecrets` fixture.
- Update the current architecture description and the repository module outline.

`core::config` will consequently import the module-owned audio, hotkey, orchestrator,
transcriber, and post-process configuration types. Their validation rules remain owned by
those modules; the consolidated module assembles their results.

## Behavioral boundaries

This is a module consolidation. Preserve schema v3, configuration paths, field permissions,
atomic persistence, environment-secret precedence and redaction, validation error ordering
and deduplication, and the existing listener and conversion configuration shapes.

File loading and field edits continue to work independently of runtime validation.
`config set` continues to support incremental configuration. The previously discussed
prompt-lab/post-processing validation coupling is outside this refactor's scope.

## Files

| File | Change |
| --- | --- |
| `src/core/config.rs` | Own the workflow types, resolvers, private helpers, and relocated tests. |
| `src/runtime_config.rs` | Remove after moving its implementation and tests. |
| `src/lib.rs` | Remove `mod runtime_config`. |
| `src/application.rs` | Use the consolidated configuration entry point. |
| `src/application/setup.rs` | Update listener configuration resolution and imports. |
| `src/application/listener.rs` | Import `ListenerConfig` from `core::config`. |
| `src/application/prompt_lab.rs` | Update configuration resolution and imports. |
| `docs/architecture/core.md`, `docs/architecture/input.md`, `AGENTS.md` | Describe the resulting ownership and module layout. |
| `changelog` | Record the configuration module consolidation. |

Historical plans retain their original descriptions of earlier implementations.

## Implementation order and validation

1. Run the existing configuration and runtime configuration tests as a baseline:
   `cargo test --locked core::config::tests` and `cargo test --locked runtime_config::tests`.
2. Move the definitions and tests, reuse the test fixture, and update all caller imports.
3. Remove the old module and update the current architecture documentation and module outline.
4. Run `cargo fmt --check`, `cargo build --locked`, `cargo test --locked`, and
   `cargo clippy --locked -- -D warnings` on macOS. Confirm all four relocated tests execute.
5. Use the existing Windows CI build and test jobs with `--locked --features windows-app`,
   plus `cargo clippy --locked --all-targets --features windows-app -- -D warnings`.
6. Check that source code and current architecture documentation contain no stale
   `runtime_config` module references, review the final diff, and push implementation to
   this same bookmark and PR through the repository's code review gate.

The existing tests cover parsing, persistence, field edits, listener resolution, API backend
construction, secret handling, and invalid runtime configuration. Reuse this behavior
coverage; add no tests that merely assert the new file location or repeat moved logic.

## Implementation results

- The baseline configuration suite passed all 17 tests across the two original modules.
- The consolidated configuration suite passed the same 17 tests, including all four relocated
  runtime configuration tests with one shared `MapSecrets` fixture.
- `cargo fmt --check`, `cargo build --locked`, `cargo test --locked` (183 tests), and
  `cargo clippy --locked -- -D warnings` passed on macOS.
- A source comparison confirmed that the moved runtime definitions and four test bodies were
  preserved verbatim, and current source and architecture docs have no old module references.
- Hosted macOS and Windows validation is tracked by the checks on
  [PR #125](https://github.com/b1indsight/viberwhisper/pull/125).
