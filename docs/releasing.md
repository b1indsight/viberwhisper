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
`viberwhisper.exe`. Python/Gemma files under `server/` are deliberately excluded, so packaged
artifacts support the API inference profile only. Run Local mode from a source checkout until a
future release explicitly adds a packaged Python runtime.

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
required distribution-guidance markers, and plist/WiX XML syntax. Focused fixtures also confirm that
each missing guidance category is rejected. CI does not build DMG or MSI artifacts. Do not create a
release tag from an unmerged change.

## 2. Validate Packaging Before Tagging

The PR workflow performs only the lightweight contract check and does not invoke release packaging.
After this manually dispatched workflow is available on the default branch, use `Package & Release`
with `publish=false` for the full two-platform validation:

- macOS: locked arm64/x86_64 builds, universal app assembly, metadata and deployment-target checks,
  ad-hoc signature verification, DMG mount inspection, and portable archive inspection;
- Windows: locked static-CRT build, CLI smoke test, CUI/GUI subsystem and dependency inspection,
  modern WiX build, MSI validation/administrative extraction, real fixture
  install-to-upgrade-to-uninstall lifecycle, shortcut-target/ARP checks, and portable ZIP
  inspection.

Dry runs upload seven-day workflow artifacts but cannot publish because the `publish` job is
disabled unless the explicit boolean input is true and the selected ref is the exact version tag.

Once the workflow is present on the default branch, run a dry run for the selected branch or tag:

```bash
gh workflow run release.yml --ref <branch-or-tag> -f publish=false
gh run list --workflow release.yml --limit 5
gh run view <run-id>
```

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

Only after an explicit release request, dispatch the workflow from the exact version tag with the
publication input enabled:

```bash
gh workflow run release.yml --ref v0.1.0 -f publish=true
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
- install the MSI on a clean Windows x86_64 machine, launch from the Start Menu, verify the tray
  appears without a console window or flash, then uninstall;
- when an older test build exists, verify the new MSI upgrades it and leaves no installer-owned
  files after uninstall;
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
