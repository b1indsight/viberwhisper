# 37 - GitHub Actions Node 24 Maintenance

## Status

**Implemented locally on 2026-09-01; hosted validation pending.** This independent PR upgrades
workflow dependencies only. It is based directly on `master`, not on another feature PR.

## Context

The successful `v0.1.0` workflow emitted GitHub's Node 20 deprecation warnings. Normal CI also
uses mutable major tags such as `actions/checkout@v4`, while the release workflow pins older
Node 20-based Action commits. GitHub plans to remove Node 20 from hosted Actions runners, so leaving
these references unchanged turns a warning into a future CI/release failure.

The repository has no Dependabot configuration for the `github-actions` ecosystem, so pinned
workflow dependencies currently rely on manual discovery.

## Goals

1. Upgrade maintained JavaScript-backed Actions to stable Node 24-compatible releases.
2. Pin every `uses:` reference in repository workflows to a reviewed full commit SHA with a
   readable version comment.
3. Add weekly Dependabot proposals for GitHub Actions updates.
4. Preserve all existing CI, notification, packaging, attestation, and publication behavior.

## Non-goals

- Adding formatting or Windows Clippy gates; that is PR #108.
- Changing Rust/Python dependencies, toolchains, runner images, or package-tool versions.
- Adding a release Environment or publication approval.
- Changing Release Notes generation.
- Signing artifacts or automating native-device tests.
- Refactoring workflow scripts or changing job permissions.

## Design

### Review and pin every Action

Inventory every `uses:` reference under `.github/workflows/`. For GitHub-maintained JavaScript
Actions, select a current stable release whose action metadata declares the Node 24 runtime.
Review migration notes for changed inputs, outputs, permissions, artifact semantics, or runner
requirements before updating.

Resolve each selected public release tag to its exact commit and write:

```yaml
uses: owner/action@<full-commit-sha> # v<reviewed-version>
```

This applies consistently to normal CI, Discord PR feedback, and release packaging. Existing
release workflow SHA pinning remains the model; mutable tags in other workflows are removed.
Third-party setup Actions that are composite or otherwise do not embed Node still receive a
reviewed current release and full-SHA pin, without claiming Node runtime behavior they do not have.
For `dtolnay/rust-toolchain`, which publishes maintained toolchain branches instead of releases,
the reviewed `stable` branch snapshot is pinned and labeled accordingly.

No Action is upgraded merely to the repository's default branch. Version comments must match the
resolved tag, and existing `with:` inputs remain unchanged unless the selected major explicitly
requires a narrow migration.

The implementation selects these reviewed upstream references:

| Action | Reviewed reference | Commit SHA | Runtime |
|---|---|---|---|
| `actions/checkout` | `v7.0.1` | `3d3c42e5aac5ba805825da76410c181273ba90b1` | Node 24 |
| `actions/setup-python` | `v7.0.0` | `5fda3b95a4ea91299a34e894583c3862153e4b97` | Node 24 |
| `astral-sh/setup-uv` | `v10.0.1` | `20cfd1bf945f4377ade1205e4dbc17946fc9a30d` | Node 24 |
| `dtolnay/rust-toolchain` | `stable` snapshot | `4360b52568e2003a75bf9bc1d59f33a8e3fc893c` | Composite |
| `actions/cache` | `v6.1.0` | `55cc8345863c7cc4c66a329aec7e433d2d1c52a9` | Node 24 |
| `actions/github-script` | `v9.0.0` | `3a2844b7e9c422d3c10d287c895573f7108da1b3` | Node 24 |
| `actions/upload-artifact` | `v7.0.1` | `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` | Node 24 |
| `actions/download-artifact` | `v8.0.1` | `3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c` | Node 24 |
| `actions/attest` | `v4.2.2` | `1e69f48acb82d1966a394da916b4c1698aa569d6` | Node 24 |

`setup-uv` v10 changes the `prune-cache` default from `true` to `false`; the workflow sets it to
`true` explicitly so the existing cache lifecycle is preserved across the major-version upgrade.

### Keep pins maintainable

Add `.github/dependabot.yml` with one weekly `github-actions` update entry rooted at `/`.
Dependabot may propose newer versions, but review and hosted validation remain required before
merge; production workflows continue to execute exact SHAs.

## Change Boundary

After approval, preserve this plan and add one implementation child change:

1. `ci(actions): migrate workflows to maintained pinned Actions`
   - upgrade and pin every workflow Action;
   - add weekly GitHub Actions Dependabot configuration;
   - update the plan status and changelog;
   - validate ordinary CI and a non-publishing release dry run.

This PR does not absorb code or documentation from the other independent maintenance PRs. If one
of them merges first, rebase onto the new `master` and retain only Action-reference changes.

## File Impact

| Path | Planned change |
|---|---|
| `.github/workflows/ci.yml` | Upgrade and full-SHA pin setup/cache Actions |
| `.github/workflows/pr-feedback-discord.yml` | Upgrade and full-SHA pin github-script |
| `.github/workflows/release.yml` | Upgrade pinned checkout/artifact/attestation Actions |
| `.github/dependabot.yml` | Propose weekly GitHub Actions updates |
| `docs/plan/37-github-actions-node24.md` | Preserve design, selected versions, and implementation status |
| `docs/README.md` | Index this plan |
| `changelog` | Record workflow dependency maintenance |

No application, package, runbook, README, architecture, configuration, WiX, or release-note content
changes because behavior and operator procedures remain the same.

## Test Strategy

### Static validation

- parse every workflow and `.github/dependabot.yml` as YAML;
- run `actionlint` across `.github/workflows/*.yml`;
- search for every `uses:` reference and reject values that are not 40-character commit SHAs;
- verify each SHA resolves to the documented upstream release tag;
- inspect selected Action metadata for Node 24 where applicable;
- run `bash scripts/validate-release-contract.sh` and `git diff --check`.

### Hosted validation

- all normal PR CI jobs pass without Node 20 deprecation annotations;
- Discord workflow syntax and permissions remain valid;
- one manually dispatched `publish=false` release run on this branch passes metadata, macOS,
  Windows, artifact upload/download, and packaging without creating a Release;
- the dry run emits no Node 20 Action warning.

No product unit test is appropriate because this PR changes external workflow implementations
rather than application logic.

## Documentation Impact

- This plan and the plan index record the reviewed dependency policy.
- `changelog` records the maintainer-visible migration.
- `docs/releasing.md` remains unchanged because commands, approval, asset contracts, and recovery
  behavior do not change.

## Risks and Controls

### Major-version behavior changes

Node 24 migrations may ship as Action major versions. Review release notes and preserve every
existing input/output contract; normal CI plus the full release dry run exercise the paths with
the highest impact.

### Pinned Actions can become stale

Full SHAs protect execution integrity but do not discover updates. Weekly Dependabot PRs restore
visibility without making production references mutable.

## Acceptance Criteria

- [x] Every workflow Action uses a reviewed full commit SHA with an accurate version comment.
- [x] JavaScript-backed Actions use stable Node 24-compatible releases.
- [ ] No ordinary CI or release dry-run job emits a Node 20 deprecation warning.
- [x] Dependabot proposes weekly `github-actions` updates from the repository root.
- [x] Existing workflow inputs, permissions, commands, artifacts, and publication behavior remain
      unchanged.
- [ ] Normal hosted CI and one complete `publish=false` release dry run pass.
- [x] Plan status, plan index, and changelog match the implementation.
