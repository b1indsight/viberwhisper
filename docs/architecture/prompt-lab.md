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
src/application/prompt_lab.rs — CLI/config/backend assembly
```

The domain modules contain no winit or native platform types. The application layer reuses the
existing listener only for capture controls and assembles offline regression from the resolved STT
configuration, `WavChunkReader`, `ApiTranscriber`, and the shared language-aware merge helper.

## Dataset and Capture

An explicit dataset root contains `dataset.json`, `audio/<sample-id>.wav`,
`samples/<sample-id>.json`, and `runs/<run-id>.json`. The root and its three owned directories are
canonicalized; symlink storage, path escape, unexpected fields, invalid versions, malformed WAVs,
and digest mismatches are rejected or reported. IDs combine a Unix-millisecond timestamp and a
process sequence.

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

Evaluation first validates all sidecars and snapshots every valid `ready` sample in stable ID
order. Its dataset digest covers only the scoring-policy version, IDs, WAV SHA-256 values, human
references, and proper-noun annotations. Initial capture text is excluded. At least one ready sample,
one scoreable reference token, and one expected proper-noun occurrence are required.

The resolved `TranscriberConfig` is consumed by an in-memory prompt override. For each snapshot WAV,
the runner recreates production-sized chunks, makes fresh sequential STT requests, merges ordered
results, and computes local metrics. It continues after a sample failure so one `incomplete` report
contains all reachable outcomes, but returns nonzero and cannot be reviewed or compared as a
baseline. References never enter an STT request.

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
