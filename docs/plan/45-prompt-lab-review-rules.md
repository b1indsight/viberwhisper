# Prompt Lab Shared Review Rules

## Status

Implemented in [PR #123](https://github.com/b1indsight/viberwhisper/pull/123).

## Implementation Result

`AgentSampleReview::validate`, `EvaluationReport::review_aggregate`, and
`GateResults::evaluate` / `all_passed` now own the shared rules in `regression.rs`. Review
application retains input identity and coverage checks; completed-report loading still validates
payloads and compares saved aggregates and gates against their derivations. Gate evaluation
retains the validated persisted mean after the existing float-tolerance check.

Boundary regression tests were written and passed against the original implementation before
the refactor. They cover both JSON entry points, failed-application byte preservation, unequal
sample scores, inclusive/failing thresholds, tolerated rounding at an exact threshold, and
altered saved means, counts, gates, and overall results.

Local macOS validation passed: 35 Prompt Lab tests, the complete 183-test suite,
`cargo fmt --check`, `cargo build --locked`, and `cargo clippy --locked -- -D warnings`.
The PR records the independent code review and hosted platform-check results.

## Problem

`review::apply_review` and `EvaluationReport::validate_complete_review` independently implement
the same agent-review rules: valid scores and explanations, mean-score aggregation, threshold
comparisons, and the overall pass result. These paths must agree because a report written after
applying a review is later read and validated by the report and comparison workflows.

The duplication is intentional at the validation boundaries, but the rule implementations must
change together. This change gives those rules one implementation while preserving validation
at both boundaries.

## Approach

Keep the shared rules beside the existing report-domain types in `src/prompt_lab/regression.rs`.
The change does not introduce a service, trait, configurable policy, or a new serialization layer.

1. Give `AgentSampleReview` one validation method for the shared payload rules: score at most
   100, nonblank reason, at least one structured difference below 100, and nonblank difference
   categories and explanations. Include the sample ID in validation errors.
2. Give the report one canonical calculation of the LLM aggregate from its per-sample reviews.
   Retain integer accumulation followed by division, and retain the scored-sample count.
3. Give `GateResults` one calculation from the local aggregates, LLM mean, and `Thresholds`,
   plus the combined pass result. Preserve inclusive `<=` for WER and `>=` for the other gates.
4. Convert each incoming review sample into `AgentSampleReview` and call its validator before
   attaching reviews. Keep schema/rubric/run identity, nonempty sample ID, duplicate detection,
   and exact coverage checks in `review.rs`, where the input document is owned.
5. Have `apply_review` use the shared aggregate and gate calculations when completing a report.
   Have `validate_complete_review` use the same payload validator and calculations, then compare
   the stored values against the recomputed values.

The report contract remains the trust boundary for stored reports. A shared calculation must not
turn validation into merely accepting saved aggregates or booleans.

## Compatibility and Scope

- Preserve CLI commands, JSON field names and nesting, `deny_unknown_fields`, report and review
  schema versions, scoring-policy version, and rubric version.
- Preserve report states, review coverage rules, comparison deltas, timestamp consistency,
  `float_eq` tolerance, and failure-before-write behavior.
- Preserve all current acceptance/rejection decisions. Shared payload errors may use one
  consistent wording across both entry points while retaining sample identity and the cause.
- Keep WER/proper-noun scoring, dataset handling, STT execution, LLM post-processing, and session
  orchestration outside this change.
- The separate removal of `PlatformInterface` is an independent small refactor and belongs to
  its own PR.

## File Changes

| File | Change |
| --- | --- |
| `src/prompt_lab/regression.rs` | Own shared review validation, LLM aggregation, and gate calculations; reuse them in report validation |
| `src/prompt_lab/review.rs` | Reuse report-domain rules during review application and retain input-specific validation |
| Tests in those modules | Cover both external JSON boundaries and derived-value consistency |
| `docs/architecture/prompt-lab.md` | Record the shared rule ownership and independent validation boundaries |
| `docs/README.md`, this plan, `changelog` | Track the plan and completed refactor |

## Implementation Order

1. Establish the existing Prompt Lab test baseline and add focused boundary regression coverage.
2. Centralize payload validation and call it from review application and completed-report loading.
3. Centralize aggregation and gate evaluation, preserving stored-value verification.
4. Remove the superseded rule implementations and update the architecture documentation.
5. Run the relevant suite, normal Rust checks, and the required independent code review gate;
   update this same PR with the implementation and validation results.

## Validation

Use local fixtures and the existing test doubles; no external STT/LLM calls are needed.

- Exercise invalid payloads through both `apply_review` and `EvaluationReport::read`: a score
  above 100, a blank reason, a below-100 score without differences, and a blank difference field.
  Failed application must leave the original report bytes unchanged.
- Exercise a valid multi-sample report with a known, noninteger mean to protect aggregation and
  JSON round-trip behavior. Preserve existing fractional local-metric coverage.
- Check all three gates at an exact threshold and on the failing side, with explicit expected
  values independent of the shared helpers.
- Verify completed-report loading still rejects altered LLM mean/count, gate values, and the
  overall pass flag. Retain identity, coverage, comparison, and metric-tampering tests.
- Reuse existing representative cases and add only missing coverage; do not duplicate the same
  rule checks in separate helper-level suites.

Run `cargo test --locked prompt_lab::` during implementation, then `cargo fmt --check`,
`cargo build --locked`, `cargo test --locked`, and `cargo clippy --locked -- -D warnings`.
The normal PR workflow provides hosted macOS and Windows validation. Review the final diff and
verify documentation links before marking the implemented PR ready.

## Acceptance Criteria

Both entry points use the same payload rules, aggregate calculation, and gate calculation.
Incoming and persisted JSON remain independently validated. Existing valid reports continue to
round-trip, invalid reviews fail before writing, and stored derived-value tampering is rejected.
