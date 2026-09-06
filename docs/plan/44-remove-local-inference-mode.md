# Remove bundled Local inference mode

## Status

Approved and implemented on draft PR #120. Native formatting, 181 Rust tests, strict Clippy, and
release-contract fixtures pass. The local Windows cross-check reaches third-party C compilation
but requires Windows SDK headers unavailable on the macOS host; hosted Windows CI remains the final
platform validation gate.

## Context

ViberWhisper currently has two inference profiles. The API profile sends transcription and
optional post-processing requests to configurable OpenAI-compatible endpoints. The Local profile
adds a bundled Python/FastAPI runtime, downloads Gemma weights, manages a virtual environment and
background process, and rewrites both endpoint configurations to that loopback service.

The bundled Local path substantially increases the repository, configuration, CI, documentation,
and runtime surface while release artifacts already exclude it. This change removes that bundled
path and leaves one API-backed application flow.

Users can still point the configurable API URLs at a separately managed service on localhost.
ViberWhisper will no longer install, start, monitor, or stop that service or its models.

## Goals

1. Remove the `viberwhisper local install/start/stop/status` command surface.
2. Remove the Rust Local installer, path validation, service manager, runtime override, and process
   guard code.
3. Remove the bundled Python Gemma server, its dependency manifests, tests, fixtures, and CI job.
4. Replace the two-profile configuration with one explicit API configuration and remove every
   Local configuration key.
5. Synchronize user, architecture, contributor, CI, and release documentation with the API-only
   product.
6. Preserve ordinary listener, setup, offline conversion, and prompt-lab behavior through the
   existing configurable OpenAI-compatible HTTP clients.

## Non-goals

- Restricting API URLs to public or remote hosts. A user-managed localhost endpoint remains valid.
- Deleting downloaded models, virtual environments, PID files, logs, or other data from an
  existing `~/.viberwhisper` directory. Removing repository support must not delete user data.
- Adding a replacement local provider, embedded model runtime, or external service manager.
- Rewriting historical implementation plans merely to erase past references. Historical plans
  remain records, with the Local plan marked as superseded by this removal.
- Changing transcription, retry, chunking, post-processing, prompt-lab, or text-injection policy.

## Configuration contract

Removing `inference.active` and `inference.local` changes the canonical JSON shape, so the strict
schema version advances from 2 to 3. Keeping version 2 while changing required/allowed fields would
make the same version describe two incompatible formats.

The v3 inference section retains the existing API nesting to minimize user and implementation
churn:

```json
{
  "schema_version": 3,
  "inference": {
    "api": {
      "transcription": {
        "api_url": "https://api.groq.com/openai/v1/audio/transcriptions",
        "model": "whisper-large-v3-turbo"
      },
      "post_process": {
        "api_url": "https://api.openai.com/v1/chat/completions",
        "model": "gpt-4o-mini"
      }
    }
  }
}
```

The repository's existing strict-config policy remains intact: it will not silently reinterpret or
rewrite an old file. A v2 configuration produces the existing actionable unsupported-schema error;
the README migration note tells API users to change `schema_version` to `3` and remove
`inference.active` plus `inference.local`. A former Local configuration must additionally provide
usable API endpoints/models before starting the application. Running `setup` without an existing
configuration writes a valid v3 API-only document.

The canonical config catalog removes these keys:

- `inference.active`
- `inference.local.data_dir`
- `inference.local.server_port`
- `inference.local.quantization`

## Runtime and CLI design

`runtime_config` will resolve the API consumer configuration directly. `ProfileSelection`,
`InferenceProfile`, the Local branch, and `BackendConfig.local_service` disappear. Listener,
offline conversion, setup validation, and prompt-lab call the direct resolver without carrying
config-directory or home-directory arguments used only by Local path resolution.

The application entry point removes Local command dispatch, installation helpers, server-file
discovery, startup guards, and automatic service startup. Clap will report `local` as an unknown
subcommand. The normal no-subcommand desktop flow remains unchanged apart from no longer starting
a bundled backend.

## Repository removal inventory

| Area | Files | Change |
| --- | --- | --- |
| Rust Local runtime | `src/local.rs`, `src/local/installer.rs`, `src/local/service.rs`, `src/lib.rs` | Delete the module and registration. |
| Application wiring | `src/application.rs`, `src/application/listener.rs`, `src/application/prompt_lab.rs`, `src/application/setup.rs` | Remove command dispatch, service startup/guarding, Local override, and Local-only path arguments. |
| CLI and config | `src/core/cli.rs`, `src/core/config.rs`, `src/core/config/document.rs`, `src/core/config/fields.rs`, `src/runtime_config.rs`, `config.example.json` | Remove Local commands/profile/fields, simplify API assembly, and establish strict schema v3. |
| Python runtime | `server/`, `pyproject.toml`, `uv.lock` | Delete the bundled server, model runtime, dependencies, tests, client, and audio fixture. |
| Automation | `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `scripts/validate-release-contract.sh`, `scripts/test-release-notes-contract.sh` | Remove the Python-only job and obsolete package assertion; update release-note validation for configurable endpoints and localhost support. |
| Current documentation | `README.md`, `AGENTS.md`, `docs/README.md`, `docs/architecture/core.md`, `docs/architecture/transcriber.md`, `docs/architecture/postprocess.md`, `docs/releasing.md`, `.github/release-notes.md` | Describe the API-only product, v3 migration, and simplified runtime/release contract. |
| Historical records | `docs/plan/12-local-gemma-service.md`, `docs/README.md`, `changelog`, this plan | Mark the old feature superseded and record its removal without rewriting unrelated historical plans or changelog entries. |

Implementation will run a repository-wide inventory after deletion. Incidental uses of “local”
that mean local WAV files, local metrics, localhost test servers, machine-local installation, or
local history are not part of the Local inference feature and remain.

## Implementation order

1. Add or update config and CLI tests to specify the API-only v3 document, removed catalog keys,
   rejected v2 documents, and rejected `local` subcommand.
2. Simplify the config document/catalog and runtime assembly, then update listener, conversion,
   setup, and prompt-lab callers until the Rust crate has no Local runtime dependency.
3. Delete the Rust Local module and bundled Python runtime, then remove their Python toolchain and
   CI configuration.
4. Update current user, architecture, contributor, release, and migration documentation; preserve
   relevant historical context and add a changelog entry.
5. Run focused tests during each step, then the complete formatting, test, lint, and cross-platform
   validation set.

## Test strategy

- Config tests prove the example and defaults round-trip as schema v3, old schema versions fail,
  removed dotted keys are unknown, secrets remain redacted, and unknown JSON fields remain strict.
- CLI tests prove supported commands still parse and `viberwhisper local ...` is rejected.
- Runtime-config tests prove listener, conversion, setup, and prompt-lab assemble the configured
  API URLs, models, authentication, and post-processing settings with no profile selection.
- Existing application, transcription, post-processing, listener, conversion, and prompt-lab tests
  guard behavior outside the removed backend.
- Repository inventories prove no product/runtime references to `LocalCommand`, `InferenceProfile`,
  `LocalServiceManager`, `inference.local`, Gemma, Hugging Face downloads, or `server/` remain.
- Validation runs `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets --all-features
  -- -D warnings`, `cargo check --target x86_64-pc-windows-msvc --all-targets --all-features`, and
  the release-contract checks used by CI. Hosted CI remains the final macOS/Windows authority.

## Acceptance criteria

- The binary exposes no `local` subcommand and starts no managed inference process.
- The repository contains no bundled model installer, Python inference server, or Python-only CI
  environment.
- Canonical configuration is strict schema v3 with only the API inference section; generated and
  example configurations match it.
- A user-managed OpenAI-compatible service, including one on localhost, remains configurable via
  the normal API URLs.
- Existing non-Local Rust behavior passes the full test and lint suite on supported targets.
- Documentation neither advertises Local mode nor implies that release packages omit a still
  supported source-only runtime.
- No implementation step removes data from a user's existing Local runtime directory.
