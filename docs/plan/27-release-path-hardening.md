# 27 - Packaging and Release Path Hardening

## Status

**Implemented; PR validation pending.** No release tag or GitHub Release is created as part of
implementation. The lightweight package contract runs in normal CI; full cross-platform packaging
and publication remain explicit maintainer actions after this workflow is available on `master`.

## Context

Plan 11 introduced cross-platform CI and a tag-triggered release workflow, but the publishing path
has never run: the repository has no release workflow runs, tags, or GitHub Releases as of
2026-08-10. Several release-only assumptions are therefore still unverified:

- Windows generates `wix/main.wxs` dynamically after a release tag is pushed instead of reviewing
  and testing the installer definition in the repository.
- The workflow installs the modern WiX CLI but lets `cargo-wix` default to the legacy WiX path, so
  the selected toolchain is ambiguous.
- `Cargo.lock` is ignored even though ViberWhisper is an application, and packaging tools are
  installed without versions, making the release inputs drift over time.
- The release tag is not checked against the Cargo package version, nor is the tagged commit
  required to belong to `master`.
- macOS metadata declares a minimum of 11.0 without building against the same deployment target.
  The current workflow also hand-assembles a universal binary even though current `cargo-bundle`
  supports repeated target arguments for that purpose.
- Raw executable uploads do not define a portable directory layout, and a bare Unix executable
  does not retain an executable-mode contract through every download/unpack path.
- Packaging cannot be exercised by a manual dry run, and the workflow does not
  inspect bundle metadata, packaged resources, architectures, installer contents, or checksums
  before publishing.
- Publication creates a normal release directly. That does not provide a safe draft-first boundary
  for repositories that enable immutable releases.

The aim of this work is a trustworthy first-release path, not merely a workflow that can upload
files.

## Goals

1. Produce tested macOS and Windows installable artifacts from one reviewed source revision.
2. Reject invalid release refs before expensive packaging begins.
3. Add a lightweight package-contract check to the existing CI, while keeping actual package builds
   in an explicit manual dry run before a tag is created.
4. Publish a complete, versioned asset set atomically through a draft GitHub Release, with SHA-256
   checksums and GitHub build provenance.
5. Document an explicit maintainer runbook for preparing, validating, publishing, verifying, and
   recovering a release.

## Non-goals

- Apple Developer ID signing, notarization, or stapling. These require an Apple Developer account,
  certificate, app-specific credentials, and an approved secret-handling policy.
- Windows Authenticode signing. This requires a signing certificate or an external signing service.
- Automatic updates, Homebrew/Winget publication, Linux packages, or ARM64 Windows packages.
- Packaging or validating the Python/Gemma local runtime. This release carries only the Rust
  executable and supports the API inference path; `local install`, `local start`, and Local profile
  operation remain source-tree workflows until a later packaging change includes `server/`.
- Automatically choosing or bumping the next product version.
- Creating a real tag or GitHub Release from this implementation PR.

Unsigned artifacts must be labeled honestly in the user documentation. The workflow will be laid
out so signing steps can be inserted between package validation and publication without changing
the release contract.

## Release Contract

For Cargo version `<version>` and tag `v<version>`, a successful release contains exactly these
assets:

| Asset | Contents |
|---|---|
| `ViberWhisper-v<version>-macos-universal.dmg` | Universal macOS application disk image |
| `ViberWhisper-v<version>-macos-universal.tar.gz` | Portable `.app` bundle preserving Unix modes |
| `ViberWhisper-v<version>-windows-x86_64.msi` | x86_64 Windows installer |
| `ViberWhisper-v<version>-windows-x86_64.zip` | Portable executable plus license |
| `SHA256SUMS` | SHA-256 digest for every distributable above |

The DMG and portable macOS archive contain the same `.app`. The MSI and portable Windows archive
contain the same release executable. Source archives remain GitHub-generated assets and are not
duplicated by the workflow. None of these artifacts includes the Python server payload; packaged
releases are API-mode distributions for this iteration.

Release versions are stable numeric `major.minor.patch` values within Windows Installer's
`255.255.65535` field limits. Preflight rejects prerelease/build suffixes and `0.0.0` rather than
publishing artifacts whose Cargo and MSI upgrade ordering differ.

## Design

### 1. Lock release inputs and package metadata

- Stop ignoring the root `Cargo.lock`, add it to version control, and run every build with
  `--locked`.
- Complete the `[package]` metadata used by installers (`authors`, `description`, `license`, and
  `repository`) while keeping `[package.metadata.bundle]` the source of macOS bundle metadata.
- Pin `cargo-bundle`, the WiX CLI, and any packaging-specific GitHub Actions to reviewed versions.
  GitHub Actions references will use immutable commit SHAs with a version comment.
- Keep the Cargo package version as the sole release version. Installer and bundle versions are
  derived from it; no second checked-in version file is introduced.

This plan does not choose or apply the next version bump. The release-preparation PR that changes
the version must update both `Cargo.toml` and the tracked `Cargo.lock`.

### 2. Build and inspect the macOS distribution

- Set `MACOSX_DEPLOYMENT_TARGET=11.0` for both architecture builds so the binary and Info.plist make
  the same compatibility claim.
- Build both `aarch64-apple-darwin` and `x86_64-apple-darwin` targets with locked dependencies,
  use pinned `cargo-bundle` to create the canonical `.app` layout, and replace its executable with
  the deterministic universal `lipo` output before producing the DMG.
- Ad-hoc sign the final `.app` and verify the signature. This catches bundle mutations and malformed
  nested content but is not represented as Developer ID signing or notarization.
- Validate before upload:
  - `lipo` reports both required architectures;
  - Mach-O deployment metadata does not exceed macOS 11.0;
  - `plutil` accepts Info.plist and confirms the bundle identifier, version,
    `NSMicrophoneUsageDescription`, and `LSUIElement` values;
  - the generated application icon exists and no unintended development files enter the bundle;
  - the DMG mounts read-only, exposes the expected `.app`, and detaches cleanly;
  - the portable `.tar.gz` contains the same bundle and preserves executable mode.

### 3. Make the Windows installer deterministic

- Replace release-time `cargo wix init` with a reviewed `wix/main.wxs` using the modern WiX schema.
- Invoke a pinned modern WiX CLI directly. Remove the `cargo-wix`/legacy WiX ambiguity and pass the
  Cargo-derived version and built executable path as build variables.
- Build the x86_64 MSVC release executable with the static CRT so a clean Windows installation does
  not separately require the Visual C++ redistributable.
- Install into one per-machine ViberWhisper directory containing:
  - `viberwhisper.exe`;
  - `LICENSE`;
  - a Start Menu shortcut and a normal Add/Remove Programs entry.
- Use stable upgrade/component identifiers so upgrades replace the earlier version and uninstall
  removes only installer-owned files. Do not add an automatic-start entry or desktop shortcut.
- Commit a frozen `0.0.0` WiX baseline for CI upgrades. Its distinct executable overlay and
  fixture-only file make payload replacement and old-file removal observable without shipping the
  fixture.
- Generate and commit an `.ico` derived from the existing icon source for installer and shortcut
  presentation; do not maintain a separate visual design.
- Validate before upload:
  - the release executable starts far enough for `--help` to succeed;
  - its dependency list does not include the dynamic MSVC runtime;
  - WiX builds the committed source without generation or mutation;
  - an administrative MSI extraction contains the executable and license only;
  - a generated lower-version fixture installs, upgrades to the release MSI, and leaves exactly
    one release-version Add/Remove Programs entry;
  - the installed executable and Start Menu shortcut exist, and uninstall removes installer-owned
    files, shortcut, and registration;
  - the portable ZIP contains the same files in the documented layout.

### 4. Separate packaging validation from publication

Refactor `.github/workflows/release.yml` into four logical jobs:

```text
preflight
  ├── package-macos
  └── package-windows
          │
          └── publish (explicit tagged dispatch only)
```

The workflow has one `workflow_dispatch` entry point with a required boolean `publish` input:

- `publish=false`: run preflight and both packaging jobs for the selected branch or tag, upload
  short-lived CI artifacts, and never publish;
- `publish=true`: require the selected ref to be the exact `v<package.version>` tag, run the same
  packaging jobs, and publish only after both platform jobs succeed.

Pull requests and tag pushes do not trigger this workflow. The existing CI performs only locked
metadata, version, required-input, plist, and WiX XML checks; publication happens only after an
explicit maintainer request.

The preflight job uses `cargo metadata --locked` and full Git history to enforce for explicit
publication runs:

- the ref is exactly `v<package.version>`, and the package version is a stable numeric triplet in
  Windows Installer's supported range;
- `Cargo.toml` and `Cargo.lock` agree;
- the tag target is an ancestor of `origin/master`;
- no GitHub Release already exists for that tag.

Packaging jobs receive the canonical version and asset prefix from preflight, create flat staging
directories with exact filenames, validate their contents, and upload artifacts with explicit
retention. They do not receive `contents: write`.

The publish job alone receives `contents: write`, `id-token: write`, and `attestations: write`. It:

1. downloads both platform artifacts into one staging directory;
2. rejects missing, extra, duplicate, or empty assets;
3. creates `SHA256SUMS` in stable filename order and verifies it locally;
4. generates GitHub build provenance for all five assets;
5. creates a draft release for the existing verified tag, attaches all assets, generates notes,
   resolves the remote tag again against the workflow event commit, and only then publishes the
   draft.

Use per-tag concurrency to prevent two publishers from racing. A failure before the final publish
leaves either no release or a recoverable draft, never an intentionally public partial release.
The runbook documents rerunning failed jobs and inspecting/removing a failed draft; the workflow
will never move or recreate an existing release tag.

### 5. Release operations and repository policy

Add a maintainer-facing release guide with this sequence:

1. prepare a PR that selects the version, updates the changelog, and updates the lockfile;
2. wait for normal CI plus packaging validation;
3. merge to `master` and verify the exact merge commit;
4. create and push an annotated `v<version>` tag at that commit without triggering publication;
5. after an explicit release request, dispatch the workflow from that tag with `publish=true` and
   monitor it;
6. download assets, run `sha256sum --check` (or the platform equivalent), and verify GitHub
   provenance;
7. perform first-launch smoke checks and record unsigned-package warnings;
8. recover only by rerunning the same immutable tag or releasing a new patch version—never by
   force-moving a published tag.

Before the first tag, configure an active `v*` tag ruleset that restricts updates and deletions
without a routine bypass. After the workflow has successfully produced a complete draft-first
release, also enable GitHub's repository-level immutable releases setting. These are external
administrative actions and are not performed by the implementation PR.

## File and Module Changes

| Path | Planned change |
|---|---|
| `.gitignore` | Stop ignoring the application lockfile |
| `Cargo.lock` | Track the resolved Rust dependency graph |
| `Cargo.toml` | Complete package and macOS bundle metadata |
| `assets/icon.ico` | Windows installer/shortcut icon derived from the current icon source |
| `wix/main.wxs` | Reviewed modern WiX installer definition and payload |
| `wix/upgrade-fixture.wxs` | Frozen distinguishable installer baseline for upgrade/cleanup CI |
| `.github/workflows/release.yml` | Preflight, dry-run packaging, validation, provenance, and draft-first publication |
| `.github/workflows/ci.yml` | Lightweight locked package-contract validation without building distributions |
| `scripts/validate-release-contract.sh` | Shared version and required-input contract for CI and release preflight |
| `docs/releasing.md` | Maintainer release and recovery runbook |
| `README.md` | Installation/download layout, unsigned-package warning, and API-only packaged-release scope |
| `AGENTS.md` | Replace obsolete ad-hoc packaging/tag commands with canonical validated commands/link |
| `docs/plan/11-packaging-and-ci.md` | Mark its untested release assumptions as superseded by this hardening plan |
| `docs/README.md` | Index this plan and the release guide |
| `changelog` | Record the release-path hardening when implemented |

No Rust application behavior, Python source, audio, transcription, session, configuration schema,
API request, or text-injection behavior is changed.

## Implementation Order

1. Track the lockfile and complete package metadata.
2. Add the Windows icon and committed modern WiX source; build and inspect the MSI on Windows CI.
3. Replace the manual macOS assembly with pinned universal bundling and add bundle/DMG inspections.
4. Add shared preflight outputs, PR/manual dry runs, exact asset staging, checksums, provenance, and
   draft-first tag publication.
5. Synchronize the release guide, user documentation, historical plan status, index, AGENTS
   instructions, and changelog.
6. Run the complete local and hosted validation matrix, review the final diff, and keep the same
   bookmark and PR for implementation.

## Test Strategy

### Local deterministic checks

- `bash scripts/validate-release-contract.sh`
- `cargo fmt --check`
- `cargo check --locked`
- `cargo test --locked`
- `cargo clippy --locked -- -D warnings`
- YAML parse plus `actionlint` for every workflow.
- `git diff --check` and searches for obsolete release commands and dynamic `cargo wix init`.

### Hosted packaging checks

- macOS job builds the universal app, validates metadata/resources/architectures/deployment target,
  verifies ad-hoc signing, mounts the DMG, and inspects the tar archive.
- Windows job builds with static CRT, runs CLI help, builds the committed WiX source, extracts the
  MSI, exercises install/upgrade/uninstall plus shortcut/ARP behavior, and inspects the portable
  Rust payload.
- A manual `publish=false` run against the implementation branch proves both jobs complete without
  creating a release.

No real release is used as a test fixture. The first tag publication is a post-merge operational
step and must satisfy the workflow's preflight and asset checks.

## Documentation Impact

- `README.md` changes because users need truthful download/install instructions, portable layouts,
  the API-only scope of packaged releases, and unsigned artifact expectations.
- `docs/releasing.md` becomes the canonical maintainer procedure; AGENTS links to it rather than
  duplicating an unsafe one-line tag command.
- Plan 11 remains a historical decision record but points to this plan for the hardened release
  contract and records that its tag path was never exercised.
- `docs/README.md` indexes both the new plan and release runbook.
- `changelog` records the maintainer- and user-visible packaging changes after implementation.
- Configuration examples and other architecture documents are unaffected because no schema,
  runtime policy, or application module boundary changes.

## Implementation Notes

Pinned `cargo-bundle` 0.11.0 does not yet accept repeated `--target` arguments even though the
current upstream documentation describes that interface. The implementation therefore builds both
targets explicitly with `cargo build --locked`, uses `cargo-bundle` 0.11.0 to generate the canonical
`.app` structure, and replaces its executable with a deterministic `lipo` result before signing and
creating the DMG. This preserves the approved universal-bundle contract without depending on an
unreleased cargo-bundle revision.

## Risks and Controls

### Unsigned desktop applications

Ad-hoc signing does not satisfy Gatekeeper trust, and the MSI remains unsigned. Documentation must
state the expected platform warnings. Developer ID/notarization and Authenticode remain explicit
release blockers for a future broadly distributed stable release, not hidden success criteria here.

### Cross-platform tools drift

Pinned package-tool versions, a tracked lockfile, immutable Action references, committed WiX source,
the lightweight CI contract, and explicit dry-run packaging make drift visible before release.
Version bumps remain deliberate maintenance PRs.

### Installer upgrades and cleanup

Stable upgrade/component identifiers, administrative extraction, and an automated frozen-baseline
install/upgrade/uninstall test cover the installer lifecycle on the hosted Windows runner. The
test compares executable digests and verifies removal of a fixture-only file.
The first real installer still requires a manual clean-machine smoke test before announcement to
catch host-policy and SmartScreen behavior outside CI.

### API-only packaged scope

The executable still exposes existing Local commands, but this release intentionally does not ship
their Python runtime files. README and release notes must state that packaged artifacts support the
API profile only and direct Local users to source-tree setup. A later plan can add a complete Python
payload and packaged resource contract without blocking this release-path hardening.

### Immutable-release recovery

The active `v*` tag ruleset blocks updates/deletions while the workflow checks that the tag still
resolves to its event commit both before draft creation and immediately before publication.
Draft-first publication allows asset upload and verification before publication. Once immutable
releases are enabled and a release is published, corrections use a new patch version; tags and
assets are not replaced in place.

## Acceptance Criteria

- [ ] `Cargo.lock` is tracked and all release builds use `--locked`.
- [ ] CI validates locked release metadata, version rules, required inputs, plist, and WiX XML
      without building distributions or invoking the release workflow.
- [ ] Manual `publish=false` dry runs build and inspect both platform distributions without
      publishing.
- [ ] A tag is rejected unless it exactly matches Cargo version and points into `master` history.
- [ ] The macOS app contains both architectures, targets macOS 11.0, has required privacy/tray
      metadata, and passes bundle/DMG/archive checks.
- [ ] The Windows MSI is built from committed modern WiX source, installs the executable, license,
      and Start Menu shortcut, and supports clean upgrade/uninstall ownership.
- [ ] The portable archives contain complete documented layouts; no bare macOS executable is
      published.
- [ ] Tag publication produces exactly four distributions plus `SHA256SUMS`, build provenance, and
      generated notes through a draft-first release.
- [ ] Publication permissions exist only on the final job and concurrent publication is serialized
      per tag.
- [ ] Documentation distinguishes verified packaging from deferred platform signing/notarization.
- [ ] Rust, workflow, macOS packaging, and Windows packaging checks all pass on the PR.
- [ ] Neither PR activity nor a tag push publishes a release; publication requires an explicit
      tagged dispatch with `publish=true`.
