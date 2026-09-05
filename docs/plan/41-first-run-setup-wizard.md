# First-run setup wizard

## Status

Approved and implemented on draft PR #117. Automated validation is recorded in the PR; packaged
macOS and Windows dialog smoke tests remain manual release checks.

## Context

ViberWhisper currently treats a missing `config.json` as an in-memory `ConfigDocument::default()`.
That is convenient for development, but a packaged desktop launch gives a new user no way to enter
an API key, choose a microphone, or verify that recording and transcription work. The CLI can edit
ordinary fields, while secret fields intentionally remain read-only and the recorder always opens
the operating system's default input device.

The listener already owns the process's `winit` event loop. Starting and closing a second full GUI
event loop before it is unsafe on macOS and would add disproportionate rendering and lifecycle
complexity for a one-time setup flow.

## Goals

1. Detect a missing, unreadable, or operationally invalid listener configuration before starting
   the tray listener.
2. Provide the same interactive setup flow from the console binary and the no-console desktop
   entry point on macOS and Windows.
3. Guide the user through the STT endpoint, model, and optional API key; optional LLM cleanup;
   recording hotkey preference; and microphone selection.
4. Let the user make a real recording and send it through the configured transcription path before
   saving.
5. Save a complete schema-v2 document atomically only after confirmation, while preserving the
   existing environment-secret and redaction rules.
6. Allow the wizard to be skipped for the current launch, using built-in defaults without silently
   overwriting an existing file.
7. Provide an explicit `viberwhisper setup` command so the flow can be rerun later.

## Non-goals

- Replacing the tray application with a persistent settings window.
- Supporting providers that are not OpenAI-compatible.
- Installing or validating the optional local Python/Gemma profile.
- Storing environment-provided secrets or integrating with Keychain/Windows Credential Manager.
- Migrating legacy flat configuration files or weakening strict schema-v2 parsing.
- Adding general audio routing, output-device selection, or live level visualization.
- Changing normal recording, chunking, retry, post-processing, or text-injection semantics.

## User experience

### Entry conditions

Normal listener startup performs a narrow bootstrap check before constructing the listener:

- a missing file opens the wizard with built-in defaults;
- a valid document that resolves into a listener configuration starts normally;
- an unreadable/schema-invalid document opens a recovery prompt, explains that the existing file
  will remain untouched until setup is saved, and starts the wizard from defaults if accepted;
- a readable document with listener validation issues opens the wizard prefilled from that
  document and shows the issues without exposing secret values.

CLI workflows such as `config get`, `convert`, `local`, and `prompt-lab` keep their present startup
behavior. `viberwhisper setup` always opens the wizard and uses the current document when it can be
loaded.

### Modal flow

Use the `tinyfiledialogs` crate for native modal message, confirmation, text-input, and password
dialogs. These dialogs work before the tray's `winit` loop and also remain visible from the Windows
GUI-subsystem executable. UI access is hidden behind a small `SetupUi` trait so flow tests do not
open real windows.

The wizard advances through these steps:

1. **Welcome/recovery** — explain why setup opened and offer Continue, Skip, or Cancel.
2. **Speech-to-text** — edit the OpenAI-compatible transcription URL and model. Enter an optional
   API key through a password field. A blank key preserves a disk key already present; if only an
   environment key is effective, the UI reports that fact but never reads it into the document.
3. **Post-processing** — enable or disable cleanup. When enabled, collect the chat-completions URL,
   model, and optional password-field API key. Disabling cleanup keeps its saved provider values so
   re-enabling it later does not discard settings.
4. **Hotkeys** — show both current bindings first (Hold F8 and Toggle F9 for a new configuration) and
   default to keeping them. When the user chooses to edit, capture the next physical key press for
   Hold and then for Toggle, show both candidate bindings together, and apply them only after final
   confirmation. Run the existing platform hotkey validation before confirmation, including
   duplicate and unsupported-key checks. An existing disabled mode remains supported when the user
   keeps the current configuration; the simplified sequential editor itself collects two keys.
5. **Microphone** — enumerate input devices, present System Default plus numbered device names, and
   store either `None` or the selected name. If device enumeration fails, show the error and allow
   retry or System Default.
6. **Verification** — show the candidate Hold and Toggle bindings before entering verification, then
   explain in that confirmation that choosing Yes closes the window, waits for a candidate hotkey,
   asks the user to say “测试”, and shows a result after the matching stop action. Use the bindings'
   normal runtime semantics instead of dialog clicks: Hold press/release starts/stops, and successive
   Toggle presses start/stop. Run the stoppable global hook in a short-lived helper process, drain
   complete audio chunks while waiting for the stop hotkey, and transcribe every returned chunk with
   the candidate STT configuration. Merge raw text with the canonical text merger and, when cleanup
   is enabled, also run the candidate post-processor. Show the raw and final text and offer Retry,
   Save, or Back to settings. A failed recording/request can be retried or explicitly saved without
   verification.
7. **Save** — resolve the candidate one final time and atomically write `config.json`. Report the
   canonical path, then start the normal listener for implicit startup or exit successfully for the
   explicit `setup` command.

Cancel exits setup without changing disk state. Skip resolves and uses `ConfigDocument::default()`
for this listener process only; it does not create or replace `config.json`, so setup is offered
again on the next launch. The skip confirmation warns that hosted defaults commonly require an
environment API key and that transcription has not been verified.

Text entry dialogs use Cancel as Back where possible. Passwords are never used as dialog defaults,
included in error text, logged, or exposed through `Debug`.

## Configuration and ownership changes

### Startup load

Make `ConfigStore::load` return `Result<Option<ConfigDocument>, ConfigError>` as the single read and
parse path. `None` identifies a missing file, `Some` carries a loaded document, and invalid files
remain errors. Ordinary callers explicitly select defaults while the application setup layer
combines absence or errors with `runtime_config::resolve_listener`; `core::config` remains
responsible only for persistence and schema parsing.

The setup coordinator lives in `src/application/setup.rs`. It owns navigation and maps UI answers
into a candidate `ConfigDocument`, but delegates field mutation and secret handling to narrow
configuration APIs. This keeps native dialog types, cpal devices, and HTTP clients out of the core
configuration module.

### Secret mutation

Keep `config set` unable to write secrets. Add setup-only setters on `ConfigDocument` for the two
optional disk secrets, with values immediately wrapped in the existing redaction-aware boundary.
Environment values remain higher priority during candidate resolution and are never copied into the
document. Existing `config get/list` output continues to report only secret source status.

### Input device

Add optional `audio.input_device` to the strict schema, field catalog, example configuration, and
`AudioConfig`. `None` means the operating-system default. A configured name is matched exactly
against `cpal` input devices whenever recording starts; absence returns a specific operational
error instead of silently recording from another microphone. A failure to read one device's display
name is logged and skipped without hiding other readable devices; failure to enumerate the device
collection remains an operational error.

cpal exposes a display name rather than a cross-reboot stable device identifier. If the host
reports duplicate names, the first exact match is used and the wizard warns about the ambiguity.
This limitation is preferable to persisting a transient enumeration index.

Expose device enumeration and selection through narrow audio-module functions/traits. The normal
recorder and verification recorder share the same resolver so the test cannot validate one device
and later record from another policy path.

## Runtime structure

```text
run / run_desktop / setup command
              |
              v
        startup configuration load
          | ready          | missing / invalid
          v                v
  resolve + listener   SetupCoordinator
                         |       |       |
                      SetupUi  devices  verifier
                         \       |       /
                          candidate document
                                  |
                         final resolve + save
                                  |
                      listener or setup-command exit
```

`SetupVerifier` is a boundary owned by the setup application module. Production verification uses
`AudioRecorder`, `ApiTranscriber`, the optional `PostProcessor`, and the shared text merger. Tests
use deterministic fakes. A short-lived helper owns the otherwise process-lifetime global keyboard
hook and reports one Start/Stop pair to the synchronous verifier. While it waits, the verifier polls
and drains each complete recorder chunk before stopping on the second hotkey event.

## File impact

| File | Planned responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Add the cross-platform modal-dialog dependency. |
| `src/core/cli.rs` | Add the explicit `setup` command. |
| `src/core/config/store.rs` | Return optional documents from the single non-mutating load and parse path. |
| `src/core/config/document.rs`, `fields.rs` | Persist optional input-device name and expose narrow setup-only secret mutation without making CLI secret fields writable. |
| `src/audio.rs`, `src/audio/recorder.rs` | Enumerate/resolve named input devices and make both normal and test recording honor `AudioConfig`. |
| `src/runtime_config.rs` | Carry selected-device configuration through listener resolution; retain current provider and secret precedence. |
| `src/application.rs` | Route implicit listener startup and the explicit command through setup bootstrap. |
| `src/application/setup.rs` | Own wizard state, dialog abstraction, candidate editing, verification, and save/skip/cancel outcomes. |
| `src/application/setup/hotkey.rs` | Own short-lived capture/verification helper IPC and the one-session hotkey state. |
| `src/application/listener.rs` | Accept the already-resolved configuration returned by bootstrap without changing listener behavior. |
| `src/input/hotkey.rs`, `src/platform.rs` | Reuse canonical event mapping and target-specific key normalization in the verification helper. |
| `config.example.json`, `README.md` | Document first-run behavior, rerunning setup, device-name semantics, plaintext disk secrets, and skip behavior. |
| `docs/architecture/core.md`, `docs/architecture/audio.md` | Record bootstrap/config ownership and named-device resolution. |
| `changelog` | Add the user-visible setup wizard entry. |

The exact split inside `setup.rs` may be adjusted during implementation if the production adapters
and pure coordinator become materially clearer as sibling modules. Native platform branches are not
expected because the selected dialog library supplies the platform adapters.

## Test-first implementation order

1. Add failing `ConfigStore::load` tests for missing, valid, and invalid documents; make ordinary
   callers explicitly select defaults and setup distinguish a missing file.
2. Add schema/catalog round-trip tests for `audio.input_device` and tests proving setup can replace
   disk secrets while public field mutation still rejects them.
3. Add fake-device tests for System Default, exact-name selection, missing names, duplicate names,
   individual unreadable names, and enumeration errors; update `AudioRecorder` to use the shared
   resolver.
4. Add table-driven setup-coordinator tests for first run, prefilled repair, environment-secret
   precedence, sequential hotkey capture and final confirmation, Back/Cancel, Skip, verification
   retry, explicit save without verification, and final save. Implement the UI-independent
   coordinator.
5. Add verifier tests around recorder outcomes, multi-chunk merge, STT failures, optional cleanup,
   and redacted errors using fake recorder/transcriber/post-processor boundaries.
6. Add the native dialog adapter and wire both listener entry points plus `viberwhisper setup` to
   bootstrap. Keep other CLI commands outside automatic setup.
7. Update user and architecture documentation, then run the complete quality gates and manual
   platform matrix.

Regression tests should protect branching, persistence, secret handling, and device selection;
they should not assert dialog wording or duplicate existing transcriber/recorder internals.

## Validation

Automated checks:

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- the existing macOS and Windows CI build/test/clippy jobs

Manual macOS and Windows checks:

1. Start with no application-data directory, complete setup, verify the chosen microphone is used,
   confirm transcription, and confirm the saved document starts the tray listener next time without
   reopening setup.
2. Repeat through the packaged/no-console entry point and confirm every dialog appears in front.
3. Enable cleanup and verify both raw STT and cleaned output are shown.
4. Supply each API key through the environment and confirm setup neither displays nor persists it.
5. Keep the displayed default hotkeys, then capture custom Hold and Toggle keys sequentially. Reject
   the final summary once and confirm the second pair, ensuring only the confirmed pair is saved.
6. Verify that both Hold press/release and two Toggle presses control test recording without a stop
   dialog, including a test longer than one chunk boundary.
7. Disconnect a named microphone and confirm recording reports the selected-device error rather
   than falling back silently.
8. Exercise Skip, Cancel, Back, failed STT retry, and explicit save-without-verification paths and
   confirm unexpected paths do not modify `config.json`.
9. Start from malformed and runtime-invalid documents, confirm recovery is offered, and confirm the
   old file is unchanged until a replacement is explicitly saved.
10. Run `viberwhisper setup` over a valid configuration and confirm existing optional values and disk
   secrets are retained when their inputs are left blank.

## Risks and controls

- **Modal UX is intentionally simple.** A sequence of native dialogs is less polished than a
  settings window, but avoids a second event loop and keeps this issue bounded. A persistent
  settings UI can later reuse the coordinator boundaries.
- **Disk keys are plaintext.** The wizard says so before saving; environment variables remain
  supported and take precedence. OS credential-store integration is separate work.
- **Provider verification costs money and sends audio.** No request is made until the user chooses
  Verify, and Save without verification remains an explicit option.
- **Device names can change.** Missing configured devices fail clearly, and rerunning `setup`
  provides the recovery path.
- **Recovery could destroy an invalid file.** No automatic repair writes to disk; only the final
  user-confirmed atomic save replaces it.

## Acceptance criteria

- [ ] Missing or invalid listener configuration opens setup from both desktop entry points before
      tray/runtime construction.
- [ ] A valid existing configuration bypasses setup; other CLI workflows retain their behavior.
- [ ] The wizard covers STT, optional cleanup, Hold/Toggle/Both, and System Default/named input
      devices without exposing secrets.
- [ ] Verification records from the selected device and displays raw transcription plus optional
      cleaned output, with retry and explicit save-without-verification paths.
- [ ] Confirmed setup writes one complete schema-v2 `config.json` atomically; cancel and failed paths
      do not write, and skip uses in-memory defaults only.
- [ ] Normal recording honors the persisted device name and reports a missing selected device.
- [ ] `viberwhisper setup` can rerun the flow for a valid configuration.
- [ ] Automated checks pass on macOS and Windows, and the packaged no-console smoke test passes.
