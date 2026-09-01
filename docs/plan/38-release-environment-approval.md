# 38 - Protected Release Environment Approval

## Status

**Implemented and validated.** This independent PR adds one publication approval boundary. It is
based directly on `master`, not on another feature PR.

## Context

The release workflow already requires an explicit `publish=true` dispatch from the exact Cargo
version tag, validates that the tag belongs to `master`, builds and checks both platforms, creates
checksums and provenance, and publishes through a complete draft. Once both packaging jobs succeed,
however, the publish job begins immediately.

The repository is public and currently has no GitHub Environments. Adding an Environment reference
without configuring required reviewers would create deployment metadata but would not provide the
requested human approval.

## Goals

1. Pause every `publish=true` run after both package jobs succeed and before any publication job
   step starts.
2. Require an explicit approval from repository owner `b1indsight`.
3. Restrict the publication Environment to `v*` tag deployments.
4. Keep `publish=false` dry runs non-interactive and preserve all existing release safeguards.

## Non-goals

- Requiring two different people or preventing maintainer self-review.
- Adding a wait timer, Environment secret, signing credential, or external deployment system.
- Upgrading GitHub Actions or changing CI quality gates.
- Changing Release Notes generation.
- Signing artifacts or automating native-device tests.
- Creating a tag or exercising a real publication solely to test this PR.

## Design

### Workflow gate

Add the GitHub Environment named `release` to the existing `publish` job only. Environment
protection is evaluated before the job starts, so the metadata, macOS, and Windows jobs complete
and upload their seven-day artifacts before the workflow enters `waiting`.

The approver can inspect the completed packaging jobs and download their candidate artifacts from
the workflow run, then approve or reject publication. Approval releases the unchanged publish job,
which downloads the artifacts, checks their exact set and checksums, attests them, creates the
draft, revalidates the tag, and publishes.

The publish job's existing `if: inputs.publish && github.ref_type == 'tag'` condition remains.
When `publish=false`, the job is skipped rather than queued, so dry runs never request approval.

### External Environment policy

Create the repository Environment `release` through the GitHub API or UI with:

- required reviewer: user `b1indsight`;
- `prevent_self_review: false`;
- wait timer: zero;
- no Environment secrets;
- custom deployment policies enabled;
- one tag policy with pattern `v*`;
- no branch policy.

Self-review is deliberate for this single-maintainer repository: the gate provides a second,
explicit decision after artifacts exist, not two-person separation of duties. The workflow's
preflight remains responsible for exact `v<package.version>` equality, membership in `master`,
and absence of an existing Release.

## Change Boundary

After approval, preserve this plan and add one implementation child change:

1. `feat(release): require approval before publication`
   - reference the protected Environment from the publish job;
   - configure and verify the external Environment rules;
   - synchronize the release runbook, plan status, index, and changelog;
   - run local validation and a non-publishing release dry run.

This PR does not upgrade Action versions or change release-note composition. If another independent
workflow PR merges first, rebase and retain only the Environment reference.

## File and External-State Impact

| Path or setting | Planned change |
|---|---|
| `.github/workflows/release.yml` | Reference `release` from the publish job |
| GitHub Environment `release` | Require `b1indsight` approval and allow only `v*` tags |
| `docs/releasing.md` | Document the waiting/inspection/approval/rejection procedure |
| `docs/plan/38-release-environment-approval.md` | Preserve design and implementation status |
| `docs/README.md` | Index this plan |
| `changelog` | Record the publication confirmation gate |

No CI, application, package, installer, README, architecture, configuration, Action dependency, or
release-note content changes.

## Test Strategy

### Local and repository validation

- parse and actionlint `.github/workflows/release.yml`;
- run `bash scripts/validate-release-contract.sh`;
- inspect the workflow diff to confirm only `publish` references the Environment;
- query the GitHub API and verify one required reviewer, self-review allowed, no wait timer, custom
  policies enabled, and exactly one `v*` tag policy;
- run `git diff --check`.

### Hosted validation

- normal PR CI passes;
- one `publish=false` release dry run completes all three packaging/metadata jobs, skips publish,
  does not enter `waiting`, and creates no Release.

An end-to-end approval cannot be safely exercised without a new publishable version tag. The next
real release must record that the publish job waited for approval before starting. No disposable
tag or Release is created for this PR.

## Documentation Impact

- `docs/releasing.md` becomes the source of truth for inspecting artifacts and approving or
  rejecting a waiting publish job.
- This plan, the plan index, and `changelog` record the operational control.
- README and architecture documents remain unchanged because installation and runtime behavior do
  not change.

## Risks and Controls

### Environment policy can drift outside Git

The workflow reference alone is insufficient. Implementation is incomplete until API inspection
confirms all external rules, and the runbook records the expected policy for later audits.

### Self-approval is not account-compromise protection

The gate prevents accidental immediate publication but cannot protect against a compromised sole
maintainer account. Exact-tag checks, limited job permissions, attestations, tag protection, and
immutable Releases remain the integrity controls.

## Acceptance Criteria

- [x] Only the `publish` job references the `release` Environment.
- [x] `publish=true` cannot start publication steps without approval after packaging succeeds.
- [x] `publish=false` skips publication without waiting for approval.
- [x] The Environment requires `b1indsight`, allows self-review, has no timer/secrets, and accepts
      only `v*` tags.
- [x] Existing tag, permissions, asset, checksum, attestation, draft-first, concurrency, and
      immutability behavior is preserved.
- [x] Normal CI and one full `publish=false` dry run pass without creating a Release.
- [x] Runbook, plan status, index, and changelog match the final configuration.

## Validation Evidence

- PR CI passed on macOS and Windows, along with Python lint and tests.
- [`publish=false` run 33470119101](https://github.com/b1indsight/viberwhisper/actions/runs/33470119101)
  completed metadata and both packaging jobs, skipped `Publish GitHub Release`, and created no new
  Release.
