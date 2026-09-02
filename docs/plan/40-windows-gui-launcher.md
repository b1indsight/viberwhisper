# 40 - Silent Windows Desktop Launcher

## Status

**Implemented on 2026-09-02.** Windows packages now carry separate CUI and GUI executable entry
points, the Start Menu targets the GUI launcher, fatal GUI startup errors use a native dialog, and
hosted Windows checks validate the PE subsystems plus the complete installer and portable layout.

## Context

[Issue #112](https://github.com/b1indsight/viberwhisper/issues/112) reports that starting
ViberWhisper opens a command-line window. The current Rust package exposes one
`viberwhisper.exe`, and Rust links that binary as a Windows console-subsystem executable by
default. The MSI Start Menu shortcut and the portable ZIP both expose that same executable, so
Explorer creates a console before Rust enters `main`; removing `println!` calls would not change
that process-level behavior.

The executable is not desktop-only. It also owns the supported `config`, `local`, `convert`, and
`prompt-lab` CLI workflows, whose output and exit status are part of their contract. Marking the
existing binary as a Windows GUI-subsystem executable would suppress the unwanted desktop console
but would also make ordinary CLI invocation unreliable, hide diagnostics, and invalidate the
release workflow's `viberwhisper.exe --help` smoke test.

## Goals

1. Starting the installed application from its Start Menu shortcut does not create a console
   window.
2. The portable Windows archive offers an explicit console-free desktop launcher.
3. Existing `viberwhisper.exe` CLI commands retain their name, stdout/stderr, synchronous process
   behavior, and exit codes.
4. A fatal desktop startup error remains visible even though no console exists.
5. Windows release CI proves the subsystem, package contents, shortcut target, upgrade, and
   uninstall contracts.

## Non-goals

- Changing macOS application startup or bundle structure.
- Adding automatic startup, a desktop shortcut, a background Windows service, or a single-instance
  process policy.
- Replacing the current tracing destination with persistent file logging.
- Renaming or removing the existing Windows CLI executable.

## Design

### 1. Keep the console binary and add a dedicated desktop binary

Keep `src/main.rs` and `viberwhisper.exe` as the console-subsystem entry point. It continues to
call the existing CLI dispatcher, including the no-subcommand listener mode used by developers
who intentionally start the application from a terminal.

Add a second Cargo binary target named `viberwhisper-app`, gated behind an internal
`windows-app` feature so ordinary builds and macOS bundling continue to select only the existing
binary. Its thin entry point carries:

```rust
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
```

The attribute changes the Windows PE subsystem at link time, which prevents Explorer from
allocating a console. The binary calls a desktop-specific library entry point and does not parse
CLI arguments. The Windows release job explicitly enables the feature and builds both binary
targets.

The release names are intentionally asymmetric:

| File | Subsystem | Purpose |
|---|---|---|
| `viberwhisper.exe` | Windows CUI | Existing CLI and terminal-started listener |
| `viberwhisper-app.exe` | Windows GUI | Start Menu and double-click desktop launch |

Keeping the established CLI filename avoids breaking commands, scripts, documentation, and user
muscle memory. The user-facing Start Menu entry remains named `ViberWhisper`; users do not need to
know the launcher filename after an MSI installation.

### 2. Give the desktop launcher a direct application entry point

Split the process initialization in `application.rs` into a small shared tracing initializer plus
two explicit public entry points:

- `run()` initializes tracing, parses `Cli`, and dispatches all existing console workflows; and
- `run_desktop()` installs valid output sinks, initializes tracing, and enters the configured
  listener directly.

The listener, runtime configuration, local-backend startup, tray, recording, transcription, and
history code remain shared. The new binary must not duplicate application assembly or synthesize
CLI arguments to reach the listener.

Before tracing or shared listener code can write output, the Windows desktop process redirects its
stdout and stderr handles to `NUL`. This keeps existing diagnostics harmless in the GUI-subsystem
process without allocating a console or changing CLI output. The desktop process boundary catches
a fatal startup error and reports it through a small native Windows error-dialog helper before
returning a failing exit code. This preserves actionable configuration, microphone, and tray
initialization failures after stderr is no longer visible.
The helper belongs to the Windows platform module and uses the existing direct Win32 FFI style;
it does not add a new dependency or introduce a general dialog abstraction.

### 3. Package both executables and point only desktop launch surfaces at the GUI binary

Extend `wix/main.wxs` to accept separate CLI and desktop executable paths. Preserve the existing
`MainExecutable` component GUID and `viberwhisper.exe` key path so the established installer
component identity does not drift. Add a new component and stable GUID for
`viberwhisper-app.exe`, and move a non-advertised Start Menu shortcut to that file. The direct
shortcut target keeps installed-package validation and diagnostics deterministic.

Do not modify the frozen `wix/upgrade-fixture.wxs` baseline. The hosted major-upgrade test must
prove that upgrading the old one-executable fixture installs both current executables, replaces
the old shortcut target, removes the fixture-only marker, and later uninstalls both executables.

The portable ZIP contains both executables and `LICENSE`. Its documentation identifies
`viberwhisper-app.exe` as the double-click entry and `viberwhisper.exe` as the CLI entry.

### 4. Make the subsystem and shortcut contracts executable release checks

The Windows release workflow already discovers Visual Studio's `dumpbin`. Extend that step to
inspect both PE headers and require:

- `viberwhisper.exe` reports the console/CUI subsystem; and
- `viberwhisper-app.exe` reports the Windows GUI subsystem.

Continue executing `viberwhisper.exe --help` to protect CLI output and exit behavior. Inspect both
executables for unintended dynamic MSVC runtime dependencies.

Update MSI extraction, install/upgrade/uninstall, digest, and portable archive assertions for the
two-executable layout. Resolve the installed Start Menu shortcut and assert that it targets
`viberwhisper-app.exe`; checking only that the shortcut exists would allow this regression to
return.

## File and Module Changes

| Path | Change |
|---|---|
| `Cargo.toml` | Declare the feature-gated `viberwhisper-app` binary target. |
| `src/bin/viberwhisper-app.rs` | Add the Windows GUI-subsystem desktop process entry point. |
| `src/lib.rs` | Export the desktop application entry point alongside the existing CLI entry. |
| `src/application.rs` | Share tracing initialization and add direct desktop dispatch with fatal-error reporting. |
| `src/platform.rs`, `src/platform/windows.rs` or a focused Windows child module | Present fatal desktop startup errors with a native dialog. |
| `wix/main.wxs` | Install both executables and target the Start Menu shortcut at the GUI launcher. |
| `.github/workflows/release.yml` | Build, inspect, package, upgrade-test, and archive both Windows executables. |
| `README.md` | Document installed and portable desktop/CLI launch choices. |
| `docs/releasing.md` | Add console-free Windows startup to the manual release smoke checks. |
| `docs/architecture/core.md` | Record the two Windows process entry points and shared application boundary. |
| `AGENTS.md` | Add the Windows desktop binary to the repository structure map. |

## Implementation Order

1. Add failing Windows release assertions for the expected two binaries and their PE subsystems.
2. Add the feature-gated GUI binary target and the shared direct desktop application entry point.
3. Add fatal Windows startup error presentation without changing ordinary CLI error handling.
4. Update WiX to install both binaries and retarget the Start Menu shortcut while preserving the
   existing CLI component identity.
5. Extend upgrade, extraction, uninstall, dependency, digest, and portable ZIP validation.
6. Update architecture, user, agent, and release documentation.
7. Run local Rust checks and the hosted Windows packaging workflow; perform the manual Start Menu
   and portable launcher smoke checks before release.

## Test Strategy

### Local deterministic checks

- `cargo fmt --check`
- `cargo test --locked`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo build --locked`
- `cargo check --locked --target x86_64-pc-windows-msvc`

The default local build must remain unambiguous and must not require the internal `windows-app`
feature. Existing CLI parser and application tests continue to cover command dispatch; the new
entry point adds no duplicate business logic that warrants parallel unit tests.

### Hosted Windows packaging checks

- Build the static-CRT CLI and feature-gated desktop binaries for `x86_64-pc-windows-msvc`.
- Run `viberwhisper.exe --help` and require a successful exit.
- Inspect PE headers with `dumpbin /headers` and assert CUI for the CLI and GUI for the desktop
  launcher.
- Inspect both executables' runtime dependencies.
- Validate MSI extraction contains exactly both executables and `LICENSE`.
- Install the frozen fixture, upgrade it to the candidate MSI, and verify both candidate digests,
  removal of fixture-only files, the GUI shortcut target, and complete uninstall cleanup.
- Inspect the portable ZIP for exactly `viberwhisper.exe`, `viberwhisper-app.exe`, and `LICENSE` in
  its documented directory.

### Manual acceptance

On a clean Windows x86_64 machine:

1. Install the MSI and launch `ViberWhisper` from the Start Menu. The tray icon appears and no
   console window appears or flashes.
2. Introduce an invalid configuration and launch again. A useful native error dialog appears
   instead of a silent exit.
3. Extract the portable ZIP and double-click `viberwhisper-app.exe`. It has the same console-free
   behavior.
4. Run `viberwhisper.exe --help` and one representative `config` command from PowerShell. Output,
   waiting behavior, and exit status remain normal.

## Risks and Controls

### Accidentally converting the CLI binary

Applying `windows_subsystem = "windows"` to `src/main.rs` would hide CLI output and change shell
process semantics. The attribute is isolated in the new binary, and PE-header assertions lock both
sides of the contract.

### Silent GUI startup failures

With no console, returning an error from `main` is not an adequate user-facing diagnostic. The
desktop entry catches fatal startup errors and displays them through the Windows platform helper.

### Installer upgrade drift

Changing the key path behind the existing component GUID could violate Windows Installer component
rules. The CLI component remains unchanged; the GUI executable receives a new component identity,
and the frozen-fixture major-upgrade test covers migration and cleanup.

### Cross-platform build ambiguity

A second unconditional binary could change ordinary `cargo run`, macOS bundling, or all-target
checks. Requiring the internal feature keeps default builds on the established entry point, while
the Windows release job names and enables the second target explicitly.

## Acceptance Criteria

- [x] MSI Start Menu and portable GUI launchers use the Windows GUI subsystem.
- [x] `viberwhisper.exe` retains working CLI output and exit behavior.
- [x] Fatal desktop startup failures remain visible through a native Windows error dialog.
- [x] The MSI installs, upgrades, and uninstalls both executables, and its shortcut targets the GUI
      launcher.
- [x] Windows CI asserts both PE subsystems rather than inferring behavior from program output.
- [x] The portable ZIP and all current documentation describe the two-entry layout consistently.
- [x] macOS packaging and ordinary default Cargo commands remain unchanged.
