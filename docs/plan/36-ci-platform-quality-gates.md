# 36 - CI Formatting and Windows Clippy Gates

## Status

**Proposed; awaiting explicit plan approval.** This is the first PR in a four-PR maintenance
series. It changes only the two requested Rust CI gates.

## Context

Normal CI currently builds and tests Rust on macOS and Windows, but it has two coverage gaps:

- the repository runbook requires `cargo fmt --check`, while hosted CI does not enforce it;
- Clippy runs only on macOS, so modules selected by `cfg(target_os = "windows")` are compiled and
  tested but are not linted with warnings denied.

These are independent of the broader release automation improvements requested after `v0.1.0`.
Keeping this PR narrow makes its CI effect easy to review and revert.

## Goals

1. Run `cargo fmt --check` in normal hosted CI.
2. Run Windows Clippy with warnings denied in normal hosted CI.
3. Preserve the existing macOS/Windows build and test commands and Discord failure notifications.

## Non-goals

- Upgrading or pinning GitHub Actions; that is the second PR in the series.
- Adding a protected release environment or human approval; that is the third PR.
- Automating Release Notes; that is the fourth PR.
- Signing packages or automating interactive native-device smoke tests.
- Changing Rust application behavior, dependencies, packaging, or release publication.

## Design

### macOS formatting gate

Update the macOS Rust toolchain setup to request both `rustfmt` and the existing `clippy`
component. Add a named `Format` step before build/test/Clippy that runs:

```bash
cargo fmt --check
```

Formatting is target-neutral, so running this once is sufficient.

### Windows Clippy gate

Update the Windows Rust toolchain setup to request `clippy`. Add a named `Clippy` step after the
existing build and test steps that runs:

```bash
cargo clippy --locked -- -D warnings
```

This command runs natively on the Windows hosted runner, so it covers Windows-selected modules
without adding cross-compilation or another target configuration.

## Change Boundary

After approval, preserve this plan as the first change and add one implementation child change:

1. `ci(rust): enforce formatting and Windows Clippy`
   - update `.github/workflows/ci.yml` with the two gates;
   - record the maintainer-visible CI contract in `changelog`;
   - validate locally and through hosted PR CI.

The implementation is one cohesive outcome; splitting format and Windows Clippy into separate
changes would not improve review or rollback.

## Independent PR Set

Submit three other, non-stacked PRs independently against `master`:

1. upgrade maintained Actions to Node 24-compatible releases, pin full SHAs, and add Dependabot;
2. add the protected `release` Environment and required publication approval;
3. prepend validated distribution guidance to automatically generated Release Notes.

Each PR owns only its named concern. If another PR merges first, rebase the remaining PRs onto the
new `master` and resolve only the mechanical overlap in `.github/workflows/release.yml`; do not fold
the merged feature into another PR's scope. Signing and native-device test automation remain
deferred.

## File Impact

| Path | Planned change |
|---|---|
| `.github/workflows/ci.yml` | Add macOS format and Windows Clippy components/steps |
| `docs/plan/36-ci-platform-quality-gates.md` | Preserve this approved design and implementation status |
| `docs/README.md` | Index this plan |
| `changelog` | Record the maintainer-visible CI gate change |

No README, architecture, configuration, application source, release workflow, installer, or
package documentation changes are needed because runtime and release behavior do not change.

## Test Strategy

### Local checks

- `cargo fmt --check`
- `cargo clippy --locked -- -D warnings` on the available macOS host
- parse `.github/workflows/ci.yml`
- run `actionlint` against `.github/workflows/ci.yml`
- `git diff --check`

### Hosted checks

- macOS CI shows a successful named `Format` step;
- Windows CI installs Clippy and shows a successful named `Clippy` step;
- existing Python, macOS build/test/Clippy, Windows build/test, and package-contract checks remain
  successful.

No new Rust unit test is appropriate because this PR changes workflow orchestration rather than
application logic. The hosted steps are the executable behavior under test.

## Documentation Impact

- This plan and the plan index change to preserve the reviewed CI contract.
- `changelog` changes with the implementation because the repository records maintainer-visible
  CI changes there.
- `docs/releasing.md` remains unchanged: its required local command list already includes
  `cargo fmt --check` and Clippy, and release publication is not modified by this PR.

## Risks and Controls

### Windows-specific lint failures

The new Windows Clippy gate may expose existing target-specific warnings. Any necessary correction
must be minimal, remain in this PR with the gate that requires it, and preserve behavior. If it
expands beyond a narrow warning fix, stop and revise the plan.

### CI duration

Windows Clippy reuses the existing Cargo cache and compiled dependency graph. The extra cost is
accepted because it validates code that macOS Clippy cannot select.

## Acceptance Criteria

- [ ] macOS CI requests `rustfmt` and passes `cargo fmt --check`.
- [ ] Windows CI requests Clippy and passes `cargo clippy --locked -- -D warnings`.
- [ ] Existing CI build, test, package-contract, Python, and notification behavior is preserved.
- [ ] No GitHub Action version, release workflow, application behavior, or package content changes.
- [ ] Local workflow checks and all hosted PR checks pass.
- [ ] The plan status, index, and changelog match the implemented scope.
