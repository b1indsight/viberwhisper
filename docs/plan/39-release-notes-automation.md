# 39 - Validated Release Notes Automation

## Status

**Proposed; awaiting explicit plan approval.** This independent PR changes Release Notes
composition only. It is based directly on `master`, not on another feature PR.

## Context

The `v0.1.0` publish job used GitHub-generated notes. The generated body contained a broad first
release PR list but omitted three distribution facts required by the release runbook:

- macOS artifacts are ad-hoc signed rather than Developer ID signed/notarized;
- Windows artifacts are not Authenticode signed;
- packaged artifacts support the API inference profile only and omit the Local Python/Gemma
  runtime.

The body was corrected manually before repository-level immutable Releases were enabled. Future
publication must create the right draft body automatically and must not depend on a post-publication
edit. Recent repository PRs are routinely unlabeled, so label-based generated-note categories are
not a reliable source for mandatory guidance.

## Goals

1. Prepend stable, reviewed distribution guidance to every automatically generated Release body.
2. Fail release preflight if the canonical guidance file is absent, empty, or loses a required
   distribution statement.
3. Retain GitHub's automatically generated change list and the existing draft-first publication
   sequence.
4. Keep signing and packaged-runtime claims synchronized through an explicit release contract.

## Non-goals

- Changing artifact signing, notarization, package contents, or Local-mode support.
- Introducing PR auto-labeling or requiring maintainers to label every PR.
- Replacing the root `changelog` or generating version numbers.
- Adding a publication Environment or human approval.
- Upgrading GitHub Actions or changing normal CI gates.
- Editing the already immutable `v0.1.0` Release.

## Design

### Canonical distribution header

Add `.github/release-notes.md` containing a concise leading section that states:

- the macOS app is ad-hoc signed, not Developer ID signed or notarized, and Gatekeeper may prompt;
- Windows artifacts are not Authenticode signed and SmartScreen may prompt;
- packages support API inference only and Local mode still requires a source checkout;
- users can validate downloads with `SHA256SUMS` and GitHub artifact provenance.

The file contains only stable release-wide facts. Per-version highlights continue to come from the
generated change list and the project changelog; no placeholder replacement or second version
source is introduced.

### Compose before draft creation

The publish job will check out the exact tag commit already validated by preflight. It reads the
tracked header and supplies it as explicit notes while retaining `--generate-notes`. GitHub CLI
prepends explicit notes to generated notes, so the mandatory section appears before the automatic
PR/contributor list.

The existing order remains:

1. verify the remote tag;
2. download and validate exactly four distribution files;
3. create and verify `SHA256SUMS`;
4. attest all five assets;
5. create a complete draft with the composed body;
6. revalidate the tag and publish the draft.

No public Release exists before the composed body and all assets are attached.

### Validate the notes contract

Extend `scripts/validate-release-contract.sh` to require a non-empty
`.github/release-notes.md`. Validate stable markers covering:

- ad-hoc versus Developer ID/notarization;
- missing Authenticode signing;
- API-only packaged scope and source-checkout Local mode;
- `SHA256SUMS`;
- GitHub provenance.

The same validator runs in normal CI and release preflight, so an incomplete header fails before
cross-platform packaging. Assertions should protect meaning without requiring byte-for-byte prose
or duplicating the whole Markdown body in shell.

## Change Boundary

After approval, preserve this plan and add one implementation child change:

1. `feat(release): prepend validated generated notes`
   - add the canonical distribution header;
   - compose it with GitHub-generated notes before draft creation;
   - extend release-contract validation;
   - synchronize the runbook, plan status, index, and changelog;
   - validate locally and with normal hosted CI.

This PR does not add approval or upgrade Actions. If another independent workflow PR merges first,
rebase and use the Action references present on the new `master` without claiming those changes.

## File Impact

| Path | Planned change |
|---|---|
| `.github/release-notes.md` | Store mandatory release-wide distribution guidance |
| `.github/workflows/release.yml` | Read and prepend the header while retaining generated notes |
| `scripts/validate-release-contract.sh` | Require and semantically validate the header |
| `docs/releasing.md` | Document composed notes and required statements |
| `docs/plan/39-release-notes-automation.md` | Preserve design and implementation status |
| `docs/README.md` | Index this plan |
| `changelog` | Record deterministic Release Notes composition |

README remains unchanged because its current unsigned/API-only installation guidance is already
truthful. Application, architecture, configuration, packaging, WiX, and artifact contents do not
change.

## Test Strategy

### Deterministic checks

- run `bash scripts/validate-release-contract.sh` on the valid repository;
- exercise focused temporary fixtures that remove or alter each required notes marker and confirm
  the validator fails with a useful message;
- parse and actionlint `.github/workflows/release.yml`;
- verify the publish command combines explicit header text with generated notes and still creates a
  draft before publication;
- run `cargo fmt --check` and `git diff --check`.

### Hosted checks

- normal PR CI passes the extended release package contract;
- a `publish=false` dry run remains non-publishing and passes package validation when needed to
  validate the complete rebased workflow.

No disposable tag or Release is created. Full body composition is verified on the next real
release by confirming its draft begins with the tracked header before approval/publication.

## Documentation Impact

- `docs/releasing.md` documents the canonical header, automatic change list, preflight contract,
  and the rule that notes are correct before publication.
- This plan, the plan index, and `changelog` record the operational change.
- README requires no edit because distribution claims do not change.

## Risks and Controls

### Static guidance can become stale

The header is deliberately restricted to distribution invariants and is part of preflight.
Developer ID, Authenticode, or packaged Local-mode work must update the Markdown and semantic
assertions together.

### Generated change lists can still be noisy

Mandatory guidance is deterministic, but GitHub retains ownership of the automatic PR list. This
PR does not invent a labeling system that the repository does not use. Future categorization can
be added independently if maintainers adopt consistent labels.

## Acceptance Criteria

- [ ] A tracked non-empty notes header states macOS signing/notarization status, Windows
      Authenticode status, API-only scope, Local-mode source requirement, checksums, and provenance.
- [ ] Release preflight fails clearly when any required guidance category is absent.
- [ ] The publish job prepends the header to GitHub-generated notes before creating the draft.
- [ ] Existing assets, checksums, attestations, tag verification, draft-first publication, and
      immutability behavior is preserved.
- [ ] No PR labeling convention, product version source, signing claim, or package content changes.
- [ ] Local contract/workflow validation and hosted PR CI pass.
- [ ] Runbook, plan status, index, and changelog match the implemented behavior.
