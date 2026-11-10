# Releasing ViberWhisper

This is the canonical maintainer runbook for API-mode macOS and Windows releases. The Release
workflow runs only when explicitly dispatched. Creating or pushing a tag does not start packaging
or publication by itself.

## Current Distribution Scope

Each release contains:

- `ViberWhisper-v<version>-macos-universal.dmg`
- `ViberWhisper-v<version>-macos-universal.tar.gz`
- `ViberWhisper-v<version>-windows-x86_64.msi`
- `ViberWhisper-v<version>-windows-x86_64.zip`
- `SHA256SUMS`

The macOS artifacts contain one universal `.app`; the Windows artifacts contain the x86_64 Rust
CLI executable, the console-free desktop launcher, and license. The MSI Start Menu shortcut and
portable double-click flow use `viberwhisper-app.exe`; terminal commands continue to use
`viberwhisper.exe`. A normal MSI invocation installs for the current user without administrator
credentials; an explicit per-machine invocation remains available for same-context upgrades of
older machine-wide installations. WiX dual-purpose packages can still publish the Installed Apps
entry under HKLM for a per-user installation; the MSI context, payload, and shortcut remain scoped
to the installing user. Transcription and optional LLM cleanup use configurable service endpoints;
release artifacts do not bundle a speech-recognition or LLM service or manage an external service
process. The application includes Silero VAD and its CPU ONNX Runtime for local silence filtering.

The `.app` receives an ad-hoc signature, but releases are not yet Developer ID signed/notarized or
Authenticode signed. Document and test the expected Gatekeeper and SmartScreen prompts; never
describe these artifacts as platform-signed.

The stable distribution guidance shown at the start of every GitHub Release is tracked in
`.github/release-notes.md`. Keep its signing, packaged-runtime, checksum, and provenance statements
in sync with this runbook. Release preflight rejects a missing, empty, or incomplete header.

The Windows packaging job records acceptance of the [WiX 7 OSMF EULA](https://docs.firegiant.com/wix/osmf/)
on its ephemeral runner before building the MSI. Before dispatching that workflow, the maintainer
must confirm the current EULA and satisfy any applicable maintenance-fee obligation.

## 1. Prepare the Version PR

Choose the version according to SemVer, then update:

1. `version` in `Cargo.toml`;
2. the root `Cargo.lock` by running a Cargo command;
3. `changelog` with maintainer- and user-visible changes.

Release versions must be stable numeric `major.minor.patch` values. The Windows Installer fields
limit major and minor to 255 and patch to 65535; prerelease/build suffixes and `0.0.0` are rejected
by preflight rather than being published with ambiguous MSI upgrade ordering.

Confirm that Cargo sees one consistent version and a locked dependency graph:

```bash
cargo metadata --locked --no-deps --format-version 1
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked -- -D warnings
```

Submit this through a normal PR against `master`. The existing CI runs a lightweight package
contract check: locked Cargo metadata, stable/MSI-compatible version rules, required release inputs,
required distribution-guidance markers, package-mode/feature/name propagation, and plist/WiX XML
syntax. Focused fixtures also confirm that each missing guidance category is rejected. Run these
locally with `bash scripts/validate-release-contract.sh` and
`bash scripts/test-release-notes-contract.sh`; the latter includes the package-mode fixtures
(`ruby scripts/test-release-package-contract.rb`, using Ruby's standard library). The fixtures
execute all four publish/diagnostic input combinations and the macOS cargo command blocks with
stub tools, and inspect the Windows cargo/delegated-script contract. They do not replace native
packaging checks. CI does not build DMG or MSI artifacts. Do not create a release tag from an
unmerged change.

## 2. Validate Packaging Before Tagging

The PR workflow performs only the lightweight contract check and does not invoke release packaging.
After this manually dispatched workflow is available on the default branch, use `Package & Release`
with both **publish** and **diagnostic_logs** unchecked (their defaults) for an ordinary-package
dry run and the full two-platform validation:

- macOS: locked arm64/x86_64 builds, universal app assembly, metadata and deployment-target checks,
  ad-hoc signature verification, DMG mount inspection, and portable archive inspection;
- Windows: locked static-CRT build, CLI smoke test, CUI/GUI subsystem and dependency inspection,
  modern WiX build, MSI validation/administrative extraction, real fixture
  per-machine upgrade lifecycle, default per-user install/uninstall lifecycle,
  shortcut-target, installer-context, and machine ARP checks, and portable ZIP inspection.

Dry runs upload seven-day workflow artifacts but cannot publish because the `publish` job is
disabled unless the explicit publication input is true, diagnostic logging is false, and the
selected ref is the exact version tag.

Once the workflow is present on the default branch, run a dry run for the selected branch or tag:

```bash
gh workflow run release.yml --ref <branch-or-tag> -f publish=false -f diagnostic_logs=false
gh run list --workflow release.yml --limit 5
gh run view <run-id>
```

### Diagnostic Test Packages

For a test package that automatically saves local DEBUG logs and model text, open **Actions →
Package & Release → Run workflow**, select the reviewed branch or tag, leave **publish** unchecked,
and check **diagnostic_logs**. This is a build-time choice, not a runtime configuration switch;
there is no configuration migration or environment-variable setup. Ordinary packages remain
capped at INFO (plus WARN/ERROR), do not record model bodies, and do not create, write, or clean up
diagnostic files.

The equivalent dispatch, only when a packaging run has been authorized, is:

```bash
gh workflow run release.yml --ref <branch-or-tag> -f publish=false -f diagnostic_logs=true
```

Both modes retain optimized `--release` builds and all the same packaging validations. Diagnostic
mode adds the nondefault Cargo feature `diagnostic-logs` to both macOS architectures and the Windows
CLI and GUI launcher (`windows-app,diagnostic-logs`). Subsequent cargo smoke tests and cargo-bundle
metadata use the same feature selection; macOS bundling keeps `CARGO_BUNDLE_SKIP_BUILD=1` and
assembles the universal binary from those builds rather than rebuilding an ordinary executable.
Preflight rejects `publish=true, diagnostic_logs=true` before either packaging job starts, and
publication also explicitly excludes diagnostic mode. `publish=false, diagnostic_logs=false`
continues to produce ordinary packages without changing their names or release upload contract.

Download the `release-macos-diagnostic` and `release-windows-diagnostic` workflow artifacts. Their
distribution filenames are:

- `ViberWhisper-v<version>-macos-universal-diagnostic.dmg`
- `ViberWhisper-v<version>-macos-universal-diagnostic.tar.gz`
- `ViberWhisper-v<version>-windows-x86_64-diagnostic.msi`
- `ViberWhisper-v<version>-windows-x86_64-diagnostic.zip`

These are test distributions, not GitHub Releases. App/executable names and installation identity
stay unchanged; in particular, Windows diagnostic and ordinary installers are not promised to
coexist. Return to an ordinary package when testing is complete.

Launching a diagnostic package automatically writes UTF-8 JSONL directly beside `config.json`,
not in a `logs/` subdirectory or the current working directory:

- macOS: `~/Library/Application Support/com.b1indsight.viberwhisper/`
- Windows: `%APPDATA%\ViberWhisper\`

The file is named `viberwhisper.YYYY-MM-DD.jsonl` for the machine's **current local calendar date**.
Same-day restarts append to that file. At diagnostic startup and on the next write after a date
rollover, the app stops using the old file and deletes other valid dated ViberWhisper log files,
including future-dated files after a clock rollback. Config, history, memory, and files outside
the exact application-log naming pattern are not deleted. There is no background cleanup while
the app is stopped: an old file can remain until the next diagnostic startup/write. Cleanup
failures are reported, not silently counted as successful retention; the current day's file has
no size cap. Reverting to an ordinary package does not delete existing diagnostic files.
Initialization failures stop startup and use the existing CLI/native desktop error boundary.
Later I/O failures use emergency stderr reporting, which may be invisible in the console-free
Windows launcher; disk failure or abrupt process termination is not a durability guarantee.

**Sensitive data warning:** diagnostic logs can contain complete model prompts, context/memory,
recognized text, and model outputs, including personal or work secrets. Use only in a trusted test
environment, inspect and redact before sharing, and delete files when no longer needed. Diagnostic
logging does not save raw audio/WAV/PCM, base64 audio, or authentication headers. Known-key redaction
is not a guarantee that secrets spoken or embedded in model text are safe. Logs are never
automatically uploaded; workflow artifact uploads contain packages, not testers' runtime logs.

## 3. Create the Release Tag

After the version PR is merged, fetch the latest remote state and identify the exact `master`
commit to release:

```bash
jj git fetch --remote origin
jj log -r master@origin -n 1
```

Before the first release, create an active GitHub tag ruleset targeting `v*` with **Restrict
updates** and **Restrict deletions** enabled and no routine bypass. Keep that ruleset active for all
release tags. The workflow resolves annotated tags to their commit both before draft creation and
again before publication, but the ruleset closes the remaining update window outside those checks.

Also keep the `release` GitHub Environment configured with `b1indsight` as its sole required
reviewer, self-review allowed, no wait timer or secrets, and a custom deployment policy that accepts
only tags matching `v*`. The Environment is an explicit confirmation boundary after candidate
artifacts exist; the tag ruleset and workflow preflight remain the controls that protect tag
identity and release eligibility. Audit the repository-side policy before publishing:

```bash
gh api repos/b1indsight/viberwhisper/environments/release
gh api repos/b1indsight/viberwhisper/environments/release/deployment-branch-policies
gh api repos/b1indsight/viberwhisper/environments/release/secrets
```

Create one annotated tag whose name exactly matches `v` plus the Cargo version, then push only that
tag. Replace the example version and commit with the reviewed values:

```bash
git tag -a v0.1.0 <master-commit-sha> -m "ViberWhisper v0.1.0"
git push origin refs/tags/v0.1.0
```

Pushing the tag does not trigger the Release workflow.

Preflight rejects the tag before packaging when:

- it differs from `v<package.version>`;
- `Cargo.toml` and `Cargo.lock` disagree;
- its target is not contained in `origin/master`;
- a GitHub Release already exists for the tag.

Never force-move a release tag.

## 4. Explicitly Publish and Monitor

Only after an explicit release request, dispatch the workflow from the exact version tag with
**publish** checked and **diagnostic_logs** unchecked:

```bash
gh workflow run release.yml --ref v0.1.0 -f publish=true -f diagnostic_logs=false
gh run list --workflow release.yml --limit 5
gh run view <run-id> --log-failed
```

After the metadata and both package jobs succeed, the `publish` job waits for approval before any
of its steps start. Open the workflow run in GitHub, inspect the three completed jobs, and download
the `release-macos` and `release-windows` candidate artifacts if a manual inspection is warranted.
Confirm that the run uses the intended immutable tag and commit, then choose **Review deployments**
and approve the `release` Environment. Reject the deployment if the tag, commit, logs, or artifacts
are unexpected; rejection prevents publication, leaves the seven-day workflow artifacts available
for investigation, and requires a new workflow run when the release is ready to retry.

After approval, the publish job:

1. accepts exactly four non-empty distribution files;
2. creates and verifies `SHA256SUMS`;
3. records GitHub build provenance for all five assets;
4. creates a draft release whose tracked distribution header precedes GitHub-generated change
   notes, and uploads every asset;
5. revalidates that the remote tag still resolves to the workflow event commit;
6. publishes the complete draft with generated notes.

## 5. Verify the Published Release

Download the assets into an empty directory, then verify their checksums:

```bash
gh release download v0.1.0 --dir release-download
cd release-download
sha256sum --check SHA256SUMS
```

On macOS, use `shasum -a 256 -c SHA256SUMS` when `sha256sum` is unavailable. Verify GitHub
provenance for each asset, for example:

```bash
gh attestation verify ViberWhisper-v0.1.0-macos-universal.dmg \
  --repo b1indsight/viberwhisper
```

Before announcing the release, perform manual smoke checks:

- mount the DMG, copy the app to `/Applications`, launch it, and verify the tray icon plus
  microphone/accessibility prompts;
- install and uninstall the MSI as a standard user on a clean Windows x86_64 machine, confirm
  neither action requests administrator credentials, launch from the Start Menu, and verify the
  tray appears without a console window or flash;
- when an older per-machine test build exists, explicitly upgrade in the per-machine context and
  verify no installer-owned files remain after uninstall; switching that installation to the new
  per-user default requires uninstalling it first;
- confirm the portable archives start from an extracted directory, including a console-free
  `viberwhisper-app.exe` launch and normal `viberwhisper.exe --help` output on Windows;
- confirm release notes begin with the tracked distribution header and retain the generated change
  list.

## 6. Recover from Failure

- If packaging fails before release creation, fix the source in a new PR. If no release exists and
  the tag still points to the intended immutable commit, rerun failed jobs; otherwise publish a new
  patch version.
- If the Environment deployment is rejected, inspect the retained workflow artifacts and logs. Once
  the concern is resolved, dispatch a new run from the same tag only when it still passes every
  preflight condition; otherwise publish a new patch version.
- If publication leaves a draft, inspect its assets and workflow logs. A maintainer may remove that
  unpublished draft in the GitHub UI and rerun the same tag workflow. Do not delete or move the tag.
- If a release was already published, do not replace its tag or assets. Correct the problem in a
  new patch release.

After the first complete release proves the draft-first path, enable immutable releases under the
repository's Releases settings. Once enabled, published tags and assets are intentionally not
replaceable.
