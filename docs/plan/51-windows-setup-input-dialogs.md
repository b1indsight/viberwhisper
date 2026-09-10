# 51 - Windows Setup Input Dialogs

## Status

The initial input-dialog change passed CI and release packaging in
[PR #136](https://github.com/b1indsight/viberwhisper/pull/136). A follow-up on 2026-09-10 addresses
missing first-run confirmation in the GUI entry. The follow-up now passes native Windows CI
and both-platform release packaging; the reporting machine still needs the new artifact.

## Follow-up validation results

- Local 191-test macOS suite, formatting, and macOS/Windows cross-target Clippy passed. Each
  follow-up code-bearing push passed the independent review gate with no findings.
- [Hosted CI](https://github.com/b1indsight/viberwhisper/actions/runs/34451194897) passed all
  179 Windows and 191 macOS tests on `930871ab3d49f6812ad818cdb4cb2865365c384f`.
- [Release dry run](https://github.com/b1indsight/viberwhisper/actions/runs/34451193551) passed
  on that same commit. The static-CRT dialog test completed in 0.87 seconds. The six actual-EXE
  launch cases passed: explicit CLI setup, first-run CLI, and first-run GUI via direct and shell
  execution with `SSH_CLIENT` both absent and present (`DISPLAY` absent). Every first-run case
  displayed its initial confirmation; all cases displayed text/password inputs and cancelled
  successfully without saving configuration.
- Windows PE/dependency checks, MSI validation/extraction, per-user installation, per-machine
  upgrade, shortcut/uninstall checks and portable ZIP inspection passed. macOS packaging also
  passed; GitHub Release publication was skipped (`publish=false`).
- [Updated Windows MSI/ZIP artifact](https://github.com/b1indsight/viberwhisper/actions/runs/34451193551/artifacts/10141761129)
  is retained for seven days. Use this artifact for the remaining machine-level verification;
  the earlier artifact predates the confirmation/message fix.
- The first follow-up test run timed out when its driver tried to dismiss an informational
  message with a synthetic IDOK command. Capturing the no-console child's output showed that
  both Yes/No confirmations and the informational message were visible. The test now dismisses
  the informational message through its normal Close action; the runtime adapter did not need
  another change for this automation issue.

## Initial validation results

- Local formatting, build, all 188 macOS tests, macOS Clippy, Windows GNU test compilation and
  Clippy, and the release contract check passed. Both independent code reviews had no findings.
- [Hosted CI](https://github.com/b1indsight/viberwhisper/actions/runs/34441640013) passed on code
  commit `71394fad2065f49b9c287ddd3813eeee737acb04`; the Windows suite includes 176 tests and
  actually creates and drives native Unicode/password inputs in a bounded child process.
- [Release dry run](https://github.com/b1indsight/viberwhisper/actions/runs/34441637979) passed
  on the same code commit. The static-CRT native input test passed, and both `viberwhisper.exe
  setup` and first-run `viberwhisper-app.exe` displayed text and masked-password inputs, then
  exited successfully on cancellation without writing configuration.
- The Windows executable subsystem/dependency checks, MSI extraction, per-user installation,
  machine upgrade, shortcut and uninstall checks, and portable ZIP inspection passed. The
  [Windows MSI/ZIP artifact](https://github.com/b1indsight/viberwhisper/actions/runs/34441637979/artifacts/10138263157)
  is retained for seven days. The macOS packaging job also passed. `publish=false` skipped
  GitHub Release publication.
- The user subsequently confirmed that with configuration absent, `viberwhisper.exe` works
  while `viberwhisper-app.exe` still has the missing-window issue. Whether the tested binaries
  came from this artifact and the exact blocking stage are not yet confirmed. The initial
  automation does not establish resolution on that machine.

## Problem and evidence

The released Windows application is reported to show no setup window while its process remains
alive with nonzero CPU usage. This plan assumes the report concerns `viberwhisper.exe setup`
or the first-run configuration wizard. The exact launch
path and Windows version remain unconfirmed. An MSI installation window is a separate path;
`wix/main.wxs` currently does not include a WixUI dialog set.

The published v0.2.0 tag is commit `5e3433f4e5f4ac795edab5f84969718736afb7e6`.
Its CLI dispatches `setup` to `src/ui/setup.rs`; the desktop launcher reaches the same wizard
when configuration is missing or invalid. The first wizard input asks for the STT API address
with a Chinese prompt.

The pinned `tinyfiledialogs` 3.9.1 implementation has a concrete hang path:

- Windows text input writes a temporary VBScript file and runs `cscript.exe`. Password input
  writes an HTA file and runs `mshta.exe`; neither input is an in-process Win32 edit control.
- After starting the hidden command process, `hiddenConsoleW` runs
  `while (EnumWindows(EnumThreadWndProc, (LPARAM)NULL)) {}`. The callback stops enumeration only
  after finding a window titled `tinyfiledialogsTopWindow`. When no such window exists,
  successful enumeration returns nonzero and the loop immediately scans again. It has no
  deadline, sleep, or child-process exit check; even an already-failed script can leave the
  application spinning indefinitely. The child-process wait and failure handling come later.
- `tinyfd_inputBoxW` opens the script with `_wfopen(..., L"w")`, writes its Unicode prompt with
  `fputws`, and ignores the write result. The ordinary text stream introduces a locale-dependent
  conversion before the script runs. Neither the application nor this dependency initializes
  the C locale. Chinese prompts therefore have an encoding failure path in addition to the
  dependency on scripting components.
- Failures that return through the backend's error paths produce a null pointer. The Rust wrapper
  maps that to `None`, and the wizard treats it as a normal user cancellation. This remains an
  error-handling defect, but it does not describe the reported process that is still alive.

The unbounded loop matches the observed live process and CPU usage; it corrects the initial
silent-exit diagnosis. Nonzero CPU alone does not establish the running thread's location or
the reason its script window failed to appear. These findings are source analysis, not a
reproduction on the reporting user's Windows machine. Verification must cover the actual
released entry points before declaring the reported issue fixed. Device enumeration also runs
before the first text input, so runtime evidence must identify whether setup reached the dialog.

Microsoft documents [EnumWindows return values](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumwindows),
the relevant [text-stream conversion](https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/fputs-fputws),
[default C locale](https://learn.microsoft.com/en-us/cpp/c-runtime-library/code-pages), and
[VBScript retirement schedule](https://techcommunity.microsoft.com/blog/windows-itpro-blog/vbscript-deprecation-timelines-and-next-steps/4148301).

## Change

### Native Windows input

Add `src/ui/setup/windows.rs`, compiled only on Windows, and route `NativeSetupUi::input` and
`NativeSetupUi::password` through it. Keep the existing wizard navigation and configuration
ownership in `src/ui/setup.rs`. The initial change retained message and confirmation dialogs;
the follow-up below removes their remaining environment-dependent console fallback. Keep the
current non-Windows adapters.

Use `DialogBoxIndirectParamW` with a small in-memory dialog template containing a prompt,
single-line edit control, OK, and Cancel. Its own modal message loop works before the tray's
event loop and from both CLI and GUI entry points. Use UTF-16 for text throughout and a normal
Windows dialog font, default-button behavior, keyboard navigation, and initial input focus.
The password form uses `ES_PASSWORD` and always starts empty. Permit empty accepted values so
the wizard can preserve its existing optional-key behavior; required fields remain validated
by the wizard.

Use a target-specific `windows-sys` dependency with only the Win32 features needed by this
adapter, selecting a compatible version already in the lockfile. Keep the template, callback,
and their state in this module. Observe native alignment and lifetime requirements, keep the
callback free of Rust unwinding, and give all handles and allocated data a bounded lifetime.
No input values pass through a script, command line, or temporary file. Remove the input path's
dependence on `hiddenConsoleW` and its global window-title polling; normal waits for user input
belong to the native modal message loop.

The API is documented by Microsoft:
[modal dialog creation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-dialogboxindirectparamw)
and [edit control styles](https://learn.microsoft.com/en-us/windows/win32/controls/edit-control-styles).

### Distinguish cancellation from failure

Change the text/password methods of the existing `SetupUi` boundary to return
`anyhow::Result<Option<String>>`: `Ok(Some(value))` means accepted, `Ok(None)` means Cancel,
Escape, or close, and `Err` means the native dialog or input retrieval failed. Propagate errors
through the wizard's input helpers and its existing `anyhow::Result` return path. Adapt the
non-Windows implementation and test doubles to the same signature.

Preserve the Win32 error immediately on failure and add context naming the failed operation,
without including entered values or secrets. The CLI reports the error with a failing exit
code; the GUI launcher uses its existing native startup-error dialog. A failed or cancelled
wizard never saves its candidate configuration before explicit confirmation. A failure showing
the final saved notification reports an error but does not undo the completed save.

## Follow-up: first-run GUI confirmation

The remaining `tinyfiledialogs::message_box_yes_no` / `message_box_ok` adapter calls
`tinyfd_messageBox`. In tinyfiledialogs 3.9.1, the Windows implementation selects the GUI only
when `!getenv("SSH_CLIENT") || getenvDISPLAY()` (along with its force-console check). With
`SSH_CLIENT` present and `DISPLAY` absent it can reach a `do ... _getch() ... while` console
loop. A GUI-subsystem process has no usable console for that prompt. This is a concrete
unbounded fallback that the initial clean-environment, input-focused smoke test did not cover;
it is not yet proof of the original machine's environment or stack.

- Route all Windows setup confirmations and informational prompts, including hotkey capture,
  through direct Unicode `MessageBoxW` calls in the existing Windows adapter.
- Use a null owner with `MB_TASKMODAL | MB_SETFOREGROUND`. Setup has no main window yet;
  borrowing `GetForegroundWindow()` would attach the prompt to an unrelated application.
- Propagate native confirmation/message failures. Preserve No, cancelled hotkey capture and
  native failure as separate outcomes, and release the capture helper if its prompt fails.
- Extend bounded native tests to confirmations and messages under `CREATE_NO_WINDOW` with
  `SSH_CLIENT` set and `DISPLAY` removed. Assert Unicode content and no borrowed owner.
- Check the first-run confirmation explicitly before advancing to inputs. Run both EXEs with
  no configuration, explicit CLI setup, and GUI direct/ShellExecute launches in clean and
  console-fallback environments. Cancel before secrets, networking or persistence.
- Keep the same bookmark and PR, and rerun CI and `publish=false` release packaging.

Microsoft documents the null-owner and task-modal behavior in
[`MessageBoxW`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-messageboxw).

## Files and implementation order

| Order | Files | Work |
| --- | --- | --- |
| 1 | `src/ui/setup.rs` tests | Add a regression proving an input failure propagates and leaves existing configuration intact; reuse cancellation coverage. |
| 2 | `src/ui/setup/windows.rs`, `Cargo.toml`, `Cargo.lock` | Add the focused Windows adapter and native Unicode/password behavior checks. |
| 3 | `src/ui/setup.rs` | Connect the Windows adapter and propagate fallible input results through existing wizard helpers. |
| 4 | `docs/architecture/ui.md`, `changelog`, this plan | Document the dialog boundary and record actual validation results. |

The release workflow also runs the native input test with the static CRT and
`scripts/test-windows-setup.ps1` against both actual release executables. That smoke check goes
through the STT address and model windows to the masked password window, then cancels and
requires process termination without saving configuration. It refuses to run over an existing
configuration and bounds native message delivery and window appearance waits.

Do not change MSI UI, CLI dispatch, hotkeys, verification recording, or configuration schema as
part of this fix. If launch details instead identify an installer issue, revise this plan and
the same PR before implementation.

## Validation and acceptance

1. Keep wizard regression tests deterministic using the existing `SetupUi` test double. Verify
   a failed text/password input returns an error and does not write configuration. Existing
   successful-save and cancellation tests should continue to pass without duplicating them.
2. Exercise real Windows text and password dialogs in a bounded native test process. Verify
   Chinese prompt/default text, a Unicode input round trip, password masking and empty defaults,
   and distinct accepted-empty/cancel results. Require the input window to appear within the
   test deadline and the process to finish after automated confirmation or cancellation, so a
   regression cannot hang CI indefinitely. Automate interaction through native controls; do not
   require microphone access, network access, or unattended human input. Do not impose a short
   production timeout on a user who is legitimately entering configuration.
3. Run the contributor-required macOS format/build/test/Clippy checks and Windows
   build/test/Clippy checks with `--features windows-app`. Native checks must run on Windows;
   a macOS-only test result cannot establish that a Windows window appears.
4. Validate the static-CRT release binaries on Windows. Run `viberwhisper.exe setup`, then
   launch `viberwhisper-app.exe` with no configuration and enter the wizard. Confirm the STT
   address input and masked API-key input appear, support Chinese text, and cancel cleanly.
   Repeat on a machine without VBScript enabled, and record Windows version and launch path.
5. Confirm real dialog creation failures use the existing visible GUI error reporting or CLI
   error output and leave no process spinning while searching for a missing window. Retain the
   published application data throughout manual checks by using an isolated Windows test account.

Implementation and validation continue on the same bookmark and PR.
