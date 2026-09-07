# Caller-declared configuration fields

## Status

The user approved field-driven configuration after the initial module consolidation in PR #125.
Typed field selection and direct consumer construction are published on PR #125. The user
approved binding secret sources to `ConfigDocument` and requested publication; this follow-up
is implemented and validated locally. Independent review and hosted CI results
are tracked on the same PR for each published change.
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
    build: impl FnOnce(R::Values) -> T,
) -> T;
```

`FieldRequest` is implemented by typed selectors and tuples of selectors. For example, a
recorder requests `(fields::AudioInputDevice, fields::AudioMicGain)` and supplies a constructor
accepting `(Option<String>, f32)`. Its result may be a configuration value or a construction
result. No string-key conversion, JSON round trip, workflow registry, or upstream type is
needed to construct the result.

The existing field catalog generates dotted names, writability metadata, typed selectors,
and CLI reads from one set of value readers. Secret selections combine environment overrides
with disk values; authentication remains redacted, and CLI reads expose only source status.
Only requested fields and their associated secret sources are consulted.

`ConfigDocument::new(source)` creates default settings bound to an owned, injectable secret
source. Default construction and deserialization use `EnvironmentSecretSource`. Clones
retain that source. Serialization and document
equality cover only persisted fields, and debug output omits the source. Field readers keep
the existing typed catalog and resolve API keys through the document, so consumers no longer
pass a source through their constructors. No additional routing table is needed.

## Ownership

| Location | Responsibility |
| --- | --- |
| `src/core/config/fields.rs` | Canonical catalog, typed requests, selected-value construction, CLI field access, and effective secrets. |
| `src/core/config.rs` | Public configuration entry point, persistence errors, and secret types. |
| `src/core/config/store.rs` | Configuration persistence and canonical platform-specific directory discovery. |
| `src/audio.rs` | Requests microphone settings and supplies fixed audio chunk limits. |
| `src/transcriber/api.rs` | Requests STT fields and constructs settings with a parsed URL. |
| `src/postprocess.rs` | Requests the enabled flag first, then LLM fields only when enabled. |
| `src/application/listener.rs` | Composes `RecordingConfig` for capture and `ListenerConfig` for normal delivery. |
| `src/application.rs` | Composes `ConvertConfig` for offline STT, cleanup, and merge language. |
| `src/application/setup.rs` | Uses the application's listener settings for setup verification. |
| `src/application/prompt_lab.rs` | Uses recording settings for capture and only STT settings for evaluation. |
| `src/input/hotkey.rs`, `src/platform.rs` | Construct named-key bindings with the selected target policy. |
| `src/history.rs`, `src/platform*` | History uses configuration's directory helper; desktop backends no longer own directory discovery. |

`BackendConfig`, workflow resolvers in `core::config`, and the redundant `check` wrapper are
removed. The CLI `config check` command builds the application-owned `ListenerConfig` directly.
Consumers construct settings directly and propagate the first error with `?`. The standalone
`validate` methods, `ValidationIssue`, `ValidationReport`, and validation-composition helper are
removed. URL parsing remains necessary for URL-typed fields; enabled cleanup still requires URL
and model values. Hotkey construction resolves key names and rejects unsupported or conflicting
bindings, including the Windows AltGr pair. There is no separate validation pass.

## Behavioral boundaries

Schema v3, the configuration file location, atomic persistence, field permissions, optional
values, environment-over-disk precedence, and redaction retain their existing behavior.
Directory identifiers remain `com.b1indsight.viberwhisper` on macOS, `ViberWhisper` on Windows,
and `viberwhisper` on fallback targets. History continues to use the same directory.

Loading and editing configuration remain independent of component construction. STT evaluation
and raw capture do not construct unused cleanup settings. Offline conversion still requires its
configured cleanup, and normal delivery/setup still construct usable hotkeys and cleanup settings.
Disabled cleanup does not resolve LLM fields or credentials. Empty-model and HTTP protocol
prechecks are removed; requests report those errors. `config check` verifies local construction
without starting services and does not guarantee that API requests will succeed.

## Validation

- Added field-projection tests first and observed them fail because selectors and `select`
  were not implemented, then made them pass with typed requests.
- Updated behavior tests first: direct API configuration initially failed under the old
  validation pass, then passed after switching to direct construction.
- Added source-binding tests before implementation, then covered source retention across clones,
  unchanged JSON and equality, rejection of runtime source fields in JSON, and setup prompts
  using the document's injected source for both API keys.
- Covered caller-owned output types, selective secret reads, environment precedence and
  redaction, STT independence, raw-capture independence, disabled cleanup, offline conversion,
  and direct consumer construction. Existing persistence, field-access, hotkey parsing, duplicate-key,
  and platform-policy coverage is retained.
- The clone test focuses on retaining the bound source; existing persistence and redaction
  tests cover serialization and debug output. Runtime-source rejection is part of the shared
  schema test, and tests do not require a fixed construction-error order.
- `cargo test --locked`: 187 tests passed on macOS.
- `cargo fmt --check`, `cargo build --locked`, and
  `cargo clippy --locked --all-targets -- -D warnings`: passed on macOS.
- Source inspection found no project-module imports in `core::config` or its submodules.
- Independent review runs before publication; hosted Windows/macOS CI results are tracked on
  PR #125 for each published commit.
