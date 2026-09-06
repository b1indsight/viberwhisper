# Anyhow application error boundaries

## Status

Approved and implemented on draft PR #119. Native formatting, tests, and Clippy pass; hosted macOS
and Windows checks remain the final validation gate.

## Context

Application and infrastructure code currently exposes `Box<dyn std::error::Error>` through 73
signatures and test doubles across 17 Rust files. Those aliases make propagation verbose, differ
on whether `Send + Sync` is present, and encourage constructing errors from bare strings without
adding operation-level context.

`anyhow::Error` is still a type-erased wrapper around dynamic errors; this migration does not
eliminate dynamic dispatch. It does provide one narrow, thread-safe application error type,
automatic `?` conversions for compatible errors, and contextual error chains. Typed domain errors
remain preferable wherever callers need to branch on a variant.

## Goals

1. Use `anyhow::Result` for fallible application, local-runtime, history, input, and platform glue
   that currently returns a boxed dynamic error.
2. Replace ad hoc string-to-box conversions with `anyhow!`, `bail!`, or `Context` at useful
   operation boundaries while preserving the underlying source error.
3. Give internal traits and their test doubles one consistent `Send + Sync + 'static` error
   contract through `anyhow::Error`.
4. Preserve existing CLI, desktop-dialog, tracing, and control-flow behavior.

## Non-goals

- Replacing structured errors such as `ConfigError`, `ChunkError`, `TranscribeError`,
  `PostProcessError`, `SessionError`, `CaptureError`, `DatasetError`, or `RegressionError`.
- Replacing `std::error::Error::source` return types; those `dyn Error` references are required by
  the standard trait contract.
- Converting localized setup-wizard validation results that intentionally use `String` for direct
  UI presentation.
- Changing retry policy, recovery behavior, logging policy, or user-visible success output.
- Treating `anyhow` as a performance optimization; the purpose is a simpler and more consistent
  application error boundary.

## Error boundary design

| Boundary | Result type after migration | Rationale |
| --- | --- | --- |
| CLI/process orchestration | `anyhow::Result<T>` | Callers propagate heterogeneous operational failures and do not match variants. |
| Local installer/service and history persistence | `anyhow::Result<T>` | These compose filesystem, process, HTTP, and serialization errors. |
| Text typing, tray, and platform runtime traits | `anyhow::Result<T>` | A concrete wrapper keeps trait contracts uniform and supports thread-safe propagation. |
| Domain state machines and validation | Existing typed `Result<T, E>` | Callers inspect variants to select recovery or user feedback. |
| Standard error source chains | `Option<&(dyn Error + 'static)>` | Required by `std::error::Error`; no application-defined replacement is appropriate. |

The crate will declare `anyhow = "1"` as a direct dependency even though it is currently present
transitively. Application code may rely only on direct dependencies.

## File impact

| Area | Files | Change |
| --- | --- | --- |
| Dependency and entry points | `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/application.rs` | Add the direct dependency and migrate the process/application return boundary. |
| Application workflows | `src/application/listener.rs`, `src/application/listener/event_loop.rs`, `src/application/prompt_lab.rs`, `src/application/setup.rs`, `src/application/setup/hotkey.rs` | Use `anyhow::Result` for heterogeneous orchestration failures and attach context where propagation currently loses the operation. |
| Local data and services | `src/history.rs`, `src/local/installer.rs`, `src/local/service.rs` | Replace boxed aliases and string conversions while retaining source chains. |
| Input and platform glue | `src/input/typer.rs`, `src/input/tray.rs`, `src/platform.rs`, `src/platform/backend.rs`, `src/platform/runtime.rs`, `src/platform/fallback.rs`, `src/platform/macos.rs`, `src/platform/windows.rs` | Align trait implementations, test doubles, and fatal-startup reporting on the concrete application error. |
| Project records | `docs/README.md`, `changelog`, this plan | Index the plan and record completion after implementation. |

## Implementation order

1. Add the direct dependency and migrate the smallest shared contracts first: `TextTyper`, tray
   policy, platform backend/runtime, and their mocks.
2. Migrate history and local-runtime APIs, replacing `format!(...).into()` and bare string errors
   with `anyhow` constructors and operation-specific context.
3. Migrate setup, prompt-lab adapters, listener orchestration, `application::run`, and `main` from
   the leaves toward the process boundary.
4. Update fatal desktop reporting to consume the resulting `anyhow::Error` without discarding its
   cause chain; keep native-dialog and stderr fallback behavior unchanged.
5. Re-run the boxed-error inventory, update project records, and execute the full validation set.

## Test strategy

This refactor does not add application behavior, so new tests will be limited to a credible error
chain or boundary regression discovered during implementation. Existing unit tests and mock trait
implementations provide compile-time coverage of the affected contracts.

Validation will include:

- a source inventory proving no application-owned `Box<dyn Error>` result signatures remain;
- a source inventory proving remaining `dyn Error` references are standard `source()` contracts;
- `cargo fmt --check`;
- `cargo test --locked`;
- `cargo clippy --locked --all-targets --all-features -- -D warnings` on the native target;
- hosted macOS and Windows CI, including the `windows-app` feature, before merge.

## Acceptance criteria

- Application and infrastructure fallible APIs use `anyhow::Result` instead of boxed dynamic-error
  result types.
- Structured domain errors and their variant-based handling remain intact.
- Added context preserves, rather than stringifies away, underlying filesystem, process, HTTP, and
  serialization errors.
- Setup, CLI, tray, text injection, history, and local-service success and recovery behavior are
  unchanged.
- Native checks pass and required cross-platform PR checks are green.
