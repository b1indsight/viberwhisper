# 41 - Non-elevated Windows Installation

## Status

**Implemented on 2026-09-02. Hosted Windows validation pending.**

## Context

[Issue #113](https://github.com/b1indsight/viberwhisper/issues/113) reports that Windows
installation should not require administrator privileges. Before this change, the MSI declared
`Scope="perMachine"`, which requires elevation even though ViberWhisper is a user-session desktop
utility with no service, driver, or shared machine configuration.

Version 0.1.0 was already published as per-machine. Windows Installer cannot perform a major
upgrade across per-machine and per-user contexts, so the existing machine upgrade path must remain
separate from the new default.

## Change

- Set `wix/main.wxs` to WiX 7's `perUserOrMachine` scope. A normal install then defaults to the
  current user without elevation; an explicit per-machine invocation remains available for the
  published upgrade path.
- Keep the standard Program Files and Start Menu directories, `UpgradeCode`, component GUIDs, and
  frozen per-machine fixture. Windows Installer redirects those standard resources to the selected
  context.
- Make the Start Menu shortcut advertised so its executable component remains valid in both
  installation contexts and Windows Installer can repair a missing target.
- Extend the existing Windows packaging lifecycle check to verify both cases sequentially: a fresh
  candidate install with no scope override creates per-user files and shortcut and uninstalls
  cleanly, then the frozen per-machine fixture upgrades explicitly in its original context.
- Accept WiX's current dual-purpose-package limitation: a per-user installation can publish its
  Add/Remove Programs entry under HKLM even though its MSI context, files, and shortcut are
  per-user. This does not require elevation, but other accounts on a shared computer may see an
  unusable ViberWhisper entry in Installed Apps.
- Update the release runbook and changelog. No Rust, portable archive, macOS package, or application
  data behavior changes.

## Validation

- `xmllint --noout wix/main.wxs wix/upgrade-fixture.wxs`
- `bash scripts/validate-release-contract.sh`
- `git diff --check`
- Hosted Windows package dry run covering the explicit per-machine upgrade and default per-user
  install/uninstall paths.
- Manual install from a standard Windows account, confirming that a fresh default install requests
  no administrator credentials and creates current-user files and shortcut.

## Acceptance Criteria

- [ ] A fresh default MSI install is per-user and does not require administrator credentials.
- [ ] User-scoped payload, shortcut, MSI context, and uninstall cleanup are verified.
- [ ] The frozen per-machine fixture still upgrades and uninstalls in an explicit machine context.
- [ ] Portable and non-Windows packaging remain unchanged.
