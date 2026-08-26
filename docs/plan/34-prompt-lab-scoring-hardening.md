# 34 - Prompt-Lab Scoring Hardening

## Status

**Implemented on 2026-08-27.** The two planned child changes were completed without material design
deviations. Native formatting, tests, checks, Clippy, build, and the Windows GNU cross-check passed,
and the private six-sample workflow produced a complete reviewed v2 baseline plus a complete
compatible candidate comparison.

## Context

The first end-to-end prompt-lab run against a six-sample corrected dataset exposed two independent
failures in the scoring/report boundary:

1. `evaluate` successfully wrote an `awaiting_agent_review` report whose sample WER was derived
   from two errors over thirteen reference tokens. A later `report apply-review` process rejected
   that same report because strict `serde_json::Value` equality treated the last-bit difference
   between the serialized/deserialized percentage and the freshly derived percentage as metric
   tampering. The canonical report remained unchanged, but the workflow could not progress to
   `complete`.
2. Exact Latin proper nouns next to Han text, including forms such as `OpenAI`, `macOS`, and
   `Windows`, were scored as misses. Rust's Unicode `char::is_alphanumeric` classifies Han
   characters as alphanumeric, so the current boundary rule treats `测试OpenAI的` as one token and
   rejects the embedded exact Latin span. Normal Chinese STT commonly omits spaces around Latin
   terms, making the proper-noun gate materially under-report accuracy.

The failures are local and reproducible. They do not involve STT request construction, dataset
identity, agent-review scoring, configuration, or the persisted report schema.

## Goals

1. Let a report written by `evaluate` survive a JSON write/read boundary and accept a valid,
   complete agent review without weakening structural or integer metric validation.
2. Count canonical and accepted proper-noun forms when a Latin/digit edge touches Han text, while
   continuing to reject a shorter Latin form embedded inside a larger Latin token.
3. Apply boundary checks to the first and last scored character even for multiword or punctuated
   forms such as `Visual Studio Code` and `Cargo.toml`.
4. Deliberately version the changed proper-noun semantics so old and new runs cannot be compared as
   though they used the same scoring policy.
5. Preserve all current prompt-lab privacy, dataset, failure, review, and JSON-only contracts.

## Non-goals

- Changing WER tokenization, the Jieba dictionary, or the semantic-review rubric.
- Relaxing report validation for counts, alignments, matched forms, annotations, sample identity,
  gates, comparison metadata, or agent-review coverage.
- Editing old run JSON in place or making v1 and v2 scoring runs comparison-compatible.
- Teaching the STT prompt specific product names, automatically promoting a candidate prompt, or
  changing any `transcription.prompt` configuration.
- Adding a new CLI command, report schema version, configuration field, dependency, or migration.

## Technical Approach

### 1. Validate metric structure independently from derived floating-point formatting

Replace the per-sample `serde_json::Value` equality check in `EvaluationReport::validate_contract`
with typed comparison helpers:

- WER reference-token/edit counts and the full ordered alignment remain exact.
- Proper-noun totals, annotation order, canonical names, expected/matched/missed counts, and matched
  forms remain exact.
- Only `wer_percent` and `accuracy_percent`, which are deterministically derived from those integer
  counts, use the existing bounded `float_eq` comparison already used for aggregate validation.
- Missing metrics on a successful sample and present metrics on a failed sample remain invalid.

The tolerance is not a source of truth: exact counts and detailed structures still prove the
metric, while the percentage is checked against a fresh derivation within a small floating-point
round-trip allowance. No report field is ignored, and a failed review still leaves the original
report byte-for-byte unchanged.

### 2. Treat Han/Latin transitions as token boundaries

Replace the current all-or-nothing `form.iter().all(char::is_alphanumeric)` boundary rule with edge
classification:

- Han characters form one word class.
- Other Unicode alphanumeric characters (including Latin letters and digits) form another class.
- Punctuation and whitespace are boundaries.
- A form edge requires a boundary only when that edge itself is a scored word character. A
  neighboring character in the same class blocks the match; a different class permits it.

This makes `测试OpenAI的`, `使用Codex检查`, and `打开Cargo.toml然后` score their exact forms, while
`MyCodexTool` still cannot satisfy `Codex`. Checking the form's edges rather than requiring the
entire form to be alphanumeric also prevents multiword and punctuated forms from bypassing suffix
or prefix protection. Existing longest-first overlap consumption and expected-occurrence caps are
unchanged.

### 3. Version and compatibility

Increment the scoring policy from `stt-prompt-scoring-v1` to `stt-prompt-scoring-v2` because the
same hypothesis can receive a different proper-noun score. The report schema and semantic rubric
versions do not change. New v2 runs reject v1 comparison baselines through the existing policy
compatibility check; old JSON remains an honest v1 artifact and is not rewritten.

## Proposed Change Stack

The approved plan remains the first change. Implementation will use two child changes because the
fixes have independent behavior, tests, and rollback value.

### 1. `fix(prompt-lab): validate round-tripped report metrics`

- add a failing report write/read/apply-review regression with a non-terminating percentage;
- replace JSON-value equality with typed exact-plus-float comparisons;
- prove structural/count tampering is still rejected without mutating the report.

This restores the two-phase report lifecycle without changing metric results.

### 2. `fix(prompt-lab): score mixed-script proper nouns`

- add failing Han/Latin, dotted-form, multiword-form, and same-token embedding tests;
- implement edge-class boundary matching while preserving overlap and cap behavior;
- bump the scoring policy to v2 and prove v1/v2 comparisons are rejected;
- synchronize prompt-lab architecture, plan status, and changelog documentation.

This changes proper-noun scoring semantics and therefore depends only on the existing metric
contract, not on the report-round-trip fix.

## File Impact

| File | Planned change |
|---|---|
| `src/prompt_lab/metrics.rs` | Classify form/neighbor edges for mixed-script boundaries and add focused metric regressions. |
| `src/prompt_lab/regression.rs` | Compare typed per-sample metrics safely across JSON round trips and bump the scoring-policy version. |
| `src/prompt_lab/review.rs` | Add or extend the end-to-end write/read/apply-review regression where it best exercises the public workflow boundary. |
| `docs/architecture/prompt-lab.md` | Document v2 mixed-script boundary semantics and round-trip-safe derived metric validation. |
| `docs/plan/34-prompt-lab-scoring-hardening.md` | Preserve the approved design and record implementation status or material deviations. |
| `docs/README.md` | Index this plan and track its status. |
| `changelog` | Record the restored review lifecycle and corrected mixed-script proper-noun metric. |

`README.md`, `AGENTS.md`, configuration/schema files, dependencies, STT request code, dataset JSON,
report schema, semantic rubric, audio capture, and platform code are not expected to change. The
user workflow and commands remain the same; only correctness at documented internal boundaries is
being restored.

## Test Strategy

Implementation starts with the smallest failing regression at each boundary.

### Report round trip

- Build an awaiting-review report whose WER or proper-noun ratio has a non-terminating binary/JSON
  representation, write it, read it in the review path, and successfully apply complete review.
- Assert the final report is `complete`, its exact count/alignment structures are preserved, and
  its gates are derived normally.
- Mutate one exact count, alignment entry, annotation field, or matched form and assert validation
  fails while the original target report remains byte-for-byte unchanged.
- Keep the existing aggregate and invalid-review coverage; do not duplicate every validation case.

### Mixed-script proper nouns

- Match an exact Latin canonical form between Han characters with no spaces.
- Match case-insensitive accepted forms and representative multiword/dotted forms next to Han.
- Reject `Codex` inside `MyCodexTool` and preserve longest-first overlap consumption.
- Verify occurrence caps and reference-side annotation counting still behave as documented.
- Assert the scoring-policy fixture/version is v2 and a v1 report cannot serve as a comparison
  baseline.

### Validation

Before PR readiness, run:

```text
cargo fmt --check
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
cargo build
cargo check --target x86_64-pc-windows-gnu
git diff --check
```

Then rerun the local six-sample prompt-lab baseline, apply the coding-agent review, run one v2
candidate with `--compare-to`, and confirm the canonical JSON reaches `complete` with credible
mixed-script proper-noun totals. Private audio, transcripts, credentials, and run JSON remain
outside the repository.

## Documentation Impact

- Add this plan to `docs/README.md` during Planning so the proposal is discoverable.
- During Implementation, update `docs/architecture/prompt-lab.md` because the documented scoring
  policy and report-validation behavior change.
- Add one maintainer/user-relevant `changelog` entry with the behavior fix.
- Do not change the main README: command syntax, privacy boundaries, thresholds, and user workflow
  are unchanged.
- Do not rewrite plan 33: it remains the historical design record for v1; this plan records the v2
  correction.

## Acceptance Criteria

- A freshly written awaiting-review report with a non-terminating percentage can be read and
  completed by `report apply-review` in a later process.
- Integer counts, ordered WER alignment, proper-noun annotation results, and other report identity
  fields remain strictly validated; meaningful tampering is rejected before any write.
- Exact Latin/digit proper nouns adjacent to Han text count as matches without requiring STT to
  insert spaces.
- A Latin form embedded in a larger Latin token remains a miss, including for multiword and
  punctuated annotations at their outer edges.
- The scoring policy is v2, and comparisons cannot cross the v1/v2 boundary.
- No report schema, semantic rubric, CLI, configuration, dependency, STT request, dataset, privacy,
  normal listener, history, or text-injection behavior changes.
- Automated checks pass, documentation matches the implemented policy, and the private six-sample
  workflow reaches a complete reviewed baseline plus one compatible candidate comparison.
