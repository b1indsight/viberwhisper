# STT Prompt Lab Architecture

## Purpose and Boundary

Prompt lab builds a reusable, human-corrected audio dataset and evaluates the prompt sent directly
to the configured STT endpoint. It does not construct `PostProcessor`, write normal
`history.jsonl`, inject text, call a Judge service, or persist a candidate prompt into application
configuration. One prompt-lab process owns a dataset at a time.

## Modules

```text
src/prompt_lab.rs
src/prompt_lab/
  capture.rs       — session-owned archive worker and sample publication
  dataset.rs       — strict manifests/sidecars, correction, validation, scoring digest
  metrics.rs       — versioned WER alignment and proper-noun matching
  regression.rs    — fresh WAV execution, report contract, shared review rules, comparison
  review.rs        — coding-agent input identity/coverage validation and report completion
src/core/prompt_lab.rs — CLI/config/backend assembly
```

The domain modules contain no winit or native platform types. `core::prompt_lab` reuses
`ui::listener` only for capture controls and assembles offline regression from the resolved STT
configuration, `WavChunkReader`, `ApiTranscriber`, and the shared language-aware merge helper.

## Dataset and Capture

An explicit dataset root contains `dataset.json`, `audio/<sample-id>.wav`,
`samples/<sample-id>.json`, and `runs/<run-id>.json`. The root and its three owned directories are
canonicalized; symlink storage, path escape, unexpected fields, invalid versions, malformed WAVs,
and digest mismatches are rejected or reported. IDs combine a Unix-millisecond timestamp and a
process sequence.

Only `record` creates dataset storage. Evaluation, validation and sample commands require an
existing manifest and all three owned directories; they fail without creating or repairing missing
storage. Sample show/list/correct share the regular-file and dataset-containment checks, so a
sidecar symlink is rejected before its contents are read or corrected.

Prompt-lab listener output clones complete immutable `WavChunk` values into a bounded archive
worker. The worker validates a stable 16-bit integer WAV format and appends decoded samples in
session order to one final WAV. Finalization closes that writer, hashes the WAV, and directly writes
one sidecar containing raw merged STT output and sanitized endpoint/model/language/temperature/
prompt metadata. A valid WAV is retained even when STT is partial or failed. Only `sample correct`
can replace the pending reference with validated human text and proper-noun annotations.

Capture and evaluation use the shared pre-upload silent-chunk gate. A silent capture is still
archived, but its empty STT result keeps the existing failed/pending diagnostic state rather than
persisting provider hallucination text. Because ready samples require a non-empty human reference,
silence can exercise the capture path manually but does not enter WER, proper-noun, or semantic
scoring.

Persistence is intentionally direct rather than atomic. A process interruption can leave a
truncated WAV/JSON or an unreferenced file; `dataset validate` reports it and performs no repair.

## Scoring Snapshot and Execution

Validation and scoring share one ordered scan of the sidecars and their audio. The scan returns
diagnostics and valid `ready` samples together, so evaluation does not reparse sidecars or rehash
audio to construct its snapshot. Sample listing keeps its separate fail-fast behavior.
The dataset digest covers only the scoring-policy version, IDs, WAV SHA-256 values, human
references, and proper-noun annotations. Initial capture text is excluded. At least one ready sample,
one scoreable reference token, and one expected proper-noun occurrence are required.
The evaluation request's STT snapshot owns the language used for both HTTP chunk merging and
Realtime segment merging, as well as the language persisted in the report.

The selected recognizer receives an in-memory prompt override. Realtime evaluates the full task
instructions, including the initial meta instructions and base prompt. `--prompt-file` replaces them;
`--no-prompt` sends empty instructions. Personal memory is disabled, and reports store the actual
prompt without embedding context data. Candidates do not modify application configuration.
Realtime resolves the fixed-order `dictation.prompt_components` object, or the embedded JSON
default components, through the same configuration-owned composition used by live requests.
Full-string and no-prompt evaluation overrides clear the cloned component selection, so a saved
component object cannot conflict with an invocation-scoped full override. Reports and prompt
digests describe the actual composed or overridden instructions, never just one component.
For each snapshot WAV,
the runner recreates production-sized chunks, makes fresh sequential STT requests, merges ordered
results, and computes local metrics. It continues after a sample failure so one `incomplete` report
contains all reachable outcomes, but returns nonzero and cannot be reviewed or compared as a
baseline. References never enter an STT request.

## Realtime Context and Memory Scenarios

`prompt-lab evaluate --realtime-cases cases.json` opts into seeded sequential Realtime evaluation.
It requires the enabled Realtime backend. Without this flag, evaluation retains frozen empty memory.
Use the normal dataset, prompt, thresholds, output and agent-review arguments.

The strict version-2 JSON file contains `cases`. Each case has an `id`, Unix-seconds `at`, optional
`seed` and ordered `steps`. Every ready dataset ID must appear exactly once across all cases.
Cases reset connection and memory; steps reuse them and learn only from successful recordings.
`advance_seconds` moves the simulated clock before that step. No personal memory file is opened.

```json
{
  "schema_version": 2,
  "cases": [{
    "id": "project", "at": 100,
    "seed": {"long_term": [
      {"context": "project", "kind": "proper_noun", "text": "ViberWhisper"}
    ]},
    "steps": [{
      "sample_id": "REPLACE_WITH_READY_ID",
      "expect": {
        "input": {"contexts_present": ["project"]},
        "context": "project",
        "candidates_present": ["ViberWhisper"],
        "memory": {"terms_present": [{"context": "project", "kind": "proper_noun",
          "text": "ViberWhisper", "supporting_recordings": 1}]},
        "forbidden_text": ["history-only sentinel"]
      }
    }]
  }]
}
```

Replace the ID and assertions with the actual recording's requirements and include all ready IDs.
A [two-step fixture](../../tests/fixtures/prompt-lab/realtime-cases.json) illustrates seeded history,
learning support and expiration; replace its `first`/`second` IDs before running against your dataset.
`seed.observations` entries have `recording`, `segment`, `at`, `context`, `text`, `terms` (the same
shape as long-term terms). Seed recording names cannot begin with `sample:`. Future timestamps,
invalid fields, excessive memory counts and initial snapshots over the production budget are rejected
before networking. Validation checks that production initialization retains every seed observation;
observations aged 24 hours or more are rejected, using production retention rather than copied rules.
Production snapshot limits still expose the newest three histories per context.

Both `input` and `memory` accept `contexts_present/absent`, `terms_present/absent` and
`history_present/absent`. History and `forbidden_text` use exact substring matching. Term assertions
match context, kind and text; an optional `supporting_recordings` requires a short-term candidate with
that exact independent-recording support count. `candidates_absent` complements `candidates_present`.
Both candidate assertions contain strings and check the retained, correction/learning-eligible
names across all segments, not every raw array entry; seed and snapshot term assertions retain
`kind`. `context` checks the final segment's exact raw string. Omitting it or using null skips that
assertion; `"未知语境"` explicitly asserts unknown context. This optional fixture assertion does not
change the model protocol: model output must always use a string. Metadata validity is always
checked using the production parser's verdict, including missing/wrong-type fields and discarded
entries. The raw array must cover the supplied names' owned occurrences in source order, or contain
their first eight when capped; client sorting cannot hide reordering. Nine mentions with eight
strings therefore pass metadata while the incomplete name group remains absent from retained
candidates. Invented names, ambiguous overlaps, overflow, shorter incomplete arrays and unknown
context with nonempty candidates fail metadata even when production preserves the text.

The response uses the production declared vocabulary reference, audio reference and inline dynamic
context, in that order. Segment traces retain both wire text parts and the reconstructed Snapshot;
replay validates them against the seeded memory and the recording's frozen vocabulary. References,
assertions and forbidden strings are never sent. Reports include seeds and history, so store them appropriately.
Schema-2 reports replay traces through production Memory and recompute assertions on read and review.
Malformed metadata or failed assertions preserve an awaiting-review report but fail `gates.realtime`;
transport or transcription errors yield an incomplete report. Semantic review must also inspect
history contamination beyond declared strings. Ordinary non-Realtime schema-1/schema-2 reports remain readable.
`--compare-to` requires identical case IDs, order, clocks and assertions; seeds may differ, recorded
by `comparison.realtime_inputs_changed` and per-case `realtime_seed_changes` with before/after seeds.
Ordinary and scenario reports cannot compare directly. String topics no longer distinguish creating
a topic from reusing a known label.

Realtime policy `realtime-context-memory-v5` requires cases schema 2, topic strings and proper-noun
occurrence strings, with the production NFKC/lowercase, full-name, count and overlap checks. It retains
the stable-prefix / audio / dynamic input layout and requires its wire input evidence in every segment
trace. Readers preflight versions before deserializing nested assertions, so old Realtime cases and
policy-v1/v2/v3/v4 reports fail with migration/rerun guidance instead of being rescored under the new
rules. Cross-policy comparisons are rejected before networking; migrate the cases and rerun the
baseline to compare with policy v5. All new Realtime evaluations,
including ordinary runs without scenarios, record `stt.input_layout=stable-prefix-audio-dynamic-v1`.
Legacy reports omit that optional field; different layouts cannot compare as identical request
identities. HTTP metadata retains its existing shape. Historical WER from context-before-audio
does not establish the quality of this input order.

## Metrics

The scoring policy is versioned as `stt-prompt-scoring-v2`:

- WER applies NFKC first. A reference containing Han characters selects the bundled Jieba
  dictionary with HMM disabled for both sides; other references use Unicode word boundaries.
  Punctuation is excluded, tokens are case-folded, Levenshtein backtracking records every equal,
  substitution, deletion, and insertion, and the aggregate is a micro-average.
- Proper-noun matching applies NFKC, canonical plus accepted forms, each annotation's case policy,
  expected-count caps, and global longest-first overlap consumption. Outer form edges use separate
  Han and other Unicode-alphanumeric word classes, so an exact Latin/digit form can touch Han text
  while a same-class character still blocks embedding in a larger token. The edge rule also applies
  to dotted and multiword forms. The aggregate is matched expected occurrences divided by all
  expected occurrences.
- Semantic similarity is not computed locally. The coding agent applies rubric
  `semantic-equivalence-v1` to each reference/hypothesis pair and supplies an integer score, reason,
  and structured differences. The aggregate is the unweighted sample mean.

## Two-Phase JSON Report

A successful local run directly writes schema-v1 JSON with status `awaiting_agent_review` and null
LLM/gate fields. Agent review input must use the exact run ID and rubric, contain each sample ID once,
use scores in `0..=100`, and provide non-empty reasons plus differences for every score below 100.
The review module validates the entire input and unchanged report contract before any write. Metric
counts, alignments, annotations, and matched forms must exactly equal fresh derivations; derived
percentages and the LLM mean use a bounded floating-point comparison so JSON round trips cannot
invalidate an otherwise unchanged report. The module then embeds the review content, calculates
the mean and inclusive gates, and directly rewrites that report as `complete`. Invalid input
leaves the report byte-for-byte unchanged.

The report-domain types in `regression.rs` own the shared rules: `AgentSampleReview::validate`
checks review payloads, `EvaluationReport::review_aggregate` summarizes the validated sample
reviews, and `GateResults::evaluate` / `all_passed` calculate the inclusive gates and overall
result. Review application and completed-report loading both use these methods. Input document
identity, duplicate IDs, and exact sample coverage remain the responsibility of `review.rs`.
Loading a completed report independently validates each review and recomputes the aggregate and
gates to check the stored values. Gate evaluation uses the validated persisted LLM mean so an
accepted JSON rounding difference does not change an exact-threshold decision during loading.

`meets_targets` is true only when WER is at or below its maximum and both 100-point metrics are at
or above their minima. There is no combined score. Optional comparison requires a complete report
with identical dataset identity, ready IDs, endpoint/model/language/temperature, scoring policy, and
rubric before STT starts. Local deltas are written in phase one; semantic deltas are filled when the
new agent review is applied.

## Privacy

Audio and all transcripts are plaintext. Audio is sent to the configured STT service; during an
explicit tuning task the coding agent reads reference and hypothesis text from the local report.
Endpoint metadata has userinfo, query, and fragment components removed, and neither dataset nor run
JSON can contain `ApiAuth`. No external Judge request, credential, model setting, Markdown mirror,
lock file, or automatic crash recovery exists.
