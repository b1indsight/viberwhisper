# Caller-declared configuration fields

## Status

The user approved field-driven configuration after the initial module consolidation in PR #125.
The revised implementation and local validation are complete. The user requested one combined
review and publication on the same bookmark and PR. Review and hosted CI results are tracked
on PR #125.
Base: `master` at `bef71a4e` (PR #123).

## Problem and outcome

Moving runtime assembly into `core::config` made configuration depend on audio, hotkeys,
transcription, cleanup, orchestration, and the desktop platform. Consumers also depended on
configuration types, producing bidirectional module dependencies. Workflow-wide resolution
made STT-only prompt-lab operations validate unused post-processing settings.

Callers now declare their required fields, and configuration resolves those fields before
invoking a caller-provided constructor. The return type belongs to the caller. Configuration
imports no application or business modules, and the separate `runtime_config` module remains
removed.

## Interface

```rust
pub fn select<R: FieldRequest, T>(
    &self,
    fields: R,
    secrets: &dyn SecretSource,
    build: impl FnOnce(R::Values) -> T,
) -> T;
```

`FieldRequest` is implemented by typed selectors and tuples of selectors. For example, a
recorder requests `(fields::AudioInputDevice, fields::AudioMicGain)` and supplies a constructor
accepting `(Option<String>, f32)`. Its result may be a configuration value or a validation
result. No string-key conversion, JSON round trip, workflow registry, or upstream type is
needed to construct the result.

The existing field catalog generates dotted names, writability metadata, typed selectors,
and CLI reads from one set of value readers. Secret selections combine environment overrides
with disk values; authentication remains redacted, and CLI reads expose only source status.
Only requested fields and their associated secret sources are consulted.

## Ownership

| Location | Responsibility |
| --- | --- |
| `src/core/config/fields.rs` | Canonical catalog, typed requests, selected-value construction, CLI field access, and effective secrets. |
| `src/core/config.rs` | Public configuration entry point, errors, secret types, and generic validation issue collection. |
| `src/core/config/store.rs` | Configuration persistence and canonical platform-specific directory discovery. |
| `src/audio.rs` | Requests microphone settings and supplies fixed audio chunk limits. |
| `src/transcriber/api.rs` | Requests STT fields and applies transcriber validation. |
| `src/postprocess.rs` | Requests the enabled flag first, then LLM fields only when enabled. |
| `src/application/listener.rs` | Composes `RecordingConfig` for capture and `ListenerConfig` for normal delivery. |
| `src/application.rs` | Composes `ConvertConfig` for offline STT, cleanup, and merge language. |
| `src/application/setup.rs` | Uses the application's listener settings for setup verification. |
| `src/application/prompt_lab.rs` | Uses recording settings for capture and only STT settings for evaluation. |
| `src/history.rs`, `src/platform*` | History uses configuration's directory helper; desktop backends no longer own directory discovery. |

`BackendConfig`, workflow resolvers in `core::config`, and the redundant `check` wrapper are
removed. The CLI `config check` command builds the application-owned `ListenerConfig` directly.
Business modules retain their validation rules; errors from required components are aggregated
and normalized through `ValidationReport`.

## Behavioral boundaries

Schema v3, the configuration file location, atomic persistence, field permissions, optional
values, environment-over-disk precedence, and redaction retain their existing behavior.
Directory identifiers remain `com.b1indsight.viberwhisper` on macOS, `ViberWhisper` on Windows,
and `viberwhisper` on fallback targets. History continues to use the same directory.

Loading and editing configuration remain independent of business validation. STT evaluation
and raw capture intentionally stop validating unused cleanup settings. Offline conversion still
requires its configured cleanup, and normal delivery/setup still validate hotkeys and cleanup.
Disabled cleanup does not resolve LLM fields or credentials.

## Validation

- Added field-projection tests first and observed them fail because selectors and `select`
  were not implemented, then made them pass with typed requests.
- Covered caller-owned output types, selective secret reads, environment precedence and
  redaction, STT independence, raw-capture independence, disabled cleanup, offline conversion,
  and combined listener validation. Existing persistence and field-access coverage is retained.
- `cargo test --locked`: 186 tests passed on macOS.
- `cargo fmt --check`, `cargo build --locked`, and
  `cargo clippy --locked --all-targets -- -D warnings`: passed on macOS.
- Source inspection found no project-module imports in `core::config` or its submodules.
- Independent review runs before publication; hosted Windows/macOS CI results are tracked on
  PR #125 for the published commit. Earlier PR checks apply to the initial implementation only.
