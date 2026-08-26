# 33 - STT Prompt Regression Suite

## Status

**Implemented.** The two-change implementation provides native dataset capture and curation,
fresh full-dataset STT regression, versioned WER/proper-noun metrics, coding-agent semantic review,
three independent gates, compatible-run comparison, and JSON-only reports. It targets the prompt
sent directly to the STT endpoint and never evaluates or modifies the LLM post-processing prompt.

## Context

ViberWhisper already sends `transcription.prompt` with every OpenAI-compatible multipart STT
request, but prompt changes are currently judged from ad-hoc live recordings. There is no stable
way to retain the audio behind an observed result, correct that result into a reference transcript,
rerun all prior recordings with a candidate prompt, or distinguish a real improvement from a
regression on an older sample.

The requested workflow has two uses:

1. Run a dedicated recording mode that saves one complete WAV and its raw STT result under one
   sample ID. Afterwards, correct samples one by one and annotate expected proper nouns to build a
   reusable test dataset.
2. When prompt tuning is requested, rerun every ready historical recording through STT with a new
   prompt, compare each result with its human reference, emit one JSON report, and let the coding
   agent inspect that report, explain the result, revise the prompt, and repeat until all requested
   thresholds pass or further prompt-only progress is no longer credible.

Only the STT prompt changes between comparable runs. Post-processing is never invoked, old STT
outputs are never reused as candidate results, and reference transcripts are never sent to the STT
endpoint.

## Goals

1. Add a cross-platform prompt-lab recording mode that reuses the existing tray and Hold/Toggle
   controls while saving one complete WAV and one raw STT capture record per completed session.
2. Keep audio, initial recognition, corrected reference text, and proper-noun annotations linked by
   a collision-safe sample ID and verifiable audio digest.
3. Provide explicit sample list/show/correct/validate commands so a pending recording can be
   corrected repeatedly and becomes regression-ready only after validation.
4. Evaluate every ready sample against an in-memory candidate STT prompt without mutating the
   persisted `transcription.prompt` setting.
5. Calculate word error rate and proper-noun accuracy locally, then let the coding agent apply the
   fixed rubric to every reference/result pair and supply the third metric: an LLM similarity score
   capped at 100.
6. Save every run as a versioned JSON report containing aggregate metrics, per-sample results,
   structured differences, request metadata, dataset identity, thresholds, and failure details.
7. Support an optional comparison to a compatible prior run so prompt improvements and regressions
   are visible per sample without treating the prior STT output as a cache.
8. Make the report and CLI deterministic enough for a coding agent to run the loop, read the JSON,
   explain the outcome to the user, and prepare the next prompt candidate.
9. Preserve the normal listener, transcription history, text injection, post-processing, and
   configuration behavior outside explicit `prompt-lab` commands.

## Non-goals

- Automatically asking an LLM inside ViberWhisper to rewrite the STT prompt. Prompt diagnosis,
  editing, stopping for a plateau, and final user communication remain agent-driven.
- Evaluating or tuning `post_process.prompt`, or passing STT output through `PostProcessor` during
  capture or regression.
- Generating Markdown, HTML, dashboards, charts, or a GUI report. The suite writes JSON only; the
  coding agent supplies the human-readable summary in conversation.
- Adding built-in audio playback or a graphical annotation editor. Sample commands expose the
  exact WAV and sidecar paths; any normal audio player can play the WAV.
- Treating the initial capture transcript as ground truth. Only a validated human reference is
  scored.
- Caching a transcription across candidate prompts, embedding reference answers in an STT prompt,
  or evaluating only the samples that failed in the preceding run.
- Combining the three metrics into a weighted score. All configured thresholds must pass
  independently.
- Calling a separately configured Judge API/model, storing Judge credentials, or reusing the
  post-processing LLM as an evaluator. The coding agent performs semantic review in the first
  version.
- Adding automatic train/holdout splitting, repeated reviewer voting, or concurrent STT scheduling
  in the first version.
- Supporting simultaneous prompt-lab processes against one dataset, filesystem locking, atomic
  publication, or crash-safe recovery. One process owns a dataset at a time; interrupted files are
  detected and handled manually.
- Automatically writing the winning prompt back to `config.json`. Promotion remains an explicit
  user decision after reviewing the result.

## User Workflow

### 1. Capture recordings

The dataset path is always explicit so private audio is not silently placed in the repository or
application-data directory:

```text
viberwhisper prompt-lab record --dataset /path/to/my-stt-dataset
```

This command starts the existing native tray/hotkey interaction in prompt-lab mode. Each completed
Hold or Toggle session:

1. archives the complete microphone session as one WAV;
2. transcribes it with the current configured STT backend and prompt;
3. stores the raw STT result and sanitized request metadata in the matching sample sidecar; and
4. prints the sample ID plus audio/sidecar paths.

Prompt-lab recording does not post-process, append normal `history.jsonl`, or inject text into the
focused application. A partial or failed initial STT request is recorded as capture metadata rather
than discarding a valid WAV; the user may still supply a reference and use that audio in later
regressions.

### 2. Correct and annotate samples

The CLI exposes a small curation surface:

```text
viberwhisper prompt-lab sample list --dataset /path/to/my-stt-dataset --status pending
viberwhisper prompt-lab sample show --dataset /path/to/my-stt-dataset <sample-id>
viberwhisper prompt-lab sample correct --dataset /path/to/my-stt-dataset <sample-id> \
  --reference-file expected.txt --proper-nouns-file proper-nouns.json
viberwhisper prompt-lab dataset validate --dataset /path/to/my-stt-dataset
```

`correct` also accepts a short inline `--reference`; inline text and `--reference-file` are mutually
exclusive. The optional proper-nouns file is a JSON array. Rerunning `correct` validates the full
replacement in memory and then rewrites the same sidecar directly. A non-empty reference, valid
annotations, an existing readable WAV, and a matching audio digest move the sample to `ready`;
otherwise it remains `pending` or is reported as invalid. Regression never silently treats the
captured STT output as the reference.

### 3. Run a baseline or candidate prompt

```text
viberwhisper prompt-lab evaluate --dataset /path/to/my-stt-dataset \
  --prompt-file prompts/candidate.txt \
  --max-wer-percent 8 \
  --min-llm-score 95 \
  --min-proper-noun-percent 98
```

Omitting `--prompt-file` uses the configured STT prompt for a baseline. `--no-prompt` explicitly
omits the multipart prompt field; it is mutually exclusive with `--prompt-file`. A candidate file
must contain valid, non-empty UTF-8, and the exact content sent to STT is embedded in the report.
The file may be edited between runs, while persisted application configuration remains unchanged.

The STT backend, model, language, temperature, authentication, and fixed chunking/retry policies
come from the same resolved profile as `convert`; only the prompt is overridden. Every invocation
opens every ready WAV, recreates its normal chunks, and issues fresh STT requests before scoring.
No post-processing is constructed.

After fresh STT plus the two local metrics finish, the report has
`status="awaiting_agent_review"`, contains every reference/result pair, and leaves the per-sample
LLM review fields empty. The coding agent reads that JSON, applies the fixed rubric itself, and
creates a small versioned review JSON containing one score, reason, and structured difference list
for every sample. The suite then validates and incorporates it into the same run report:

```text
viberwhisper prompt-lab report apply-review \
  --report /path/to/my-stt-dataset/runs/<run-id>.json \
  --review /path/to/<run-id>.agent-review.json
```

`apply-review` requires the exact run ID and complete sample-ID coverage, rejects duplicates or
scores outside `0..=100`, calculates the LLM mean plus all three gates, and rewrites the
run to `status="complete"`. The review input is not a second generated report; it is agent-authored
structured input and its complete content is incorporated into the canonical run JSON. No Judge
URL, model, API key, or external Judge request exists in this version.

An optional `--compare-to <prior-run.json>` adds compatible run and per-sample deltas. Compatibility
requires the same dataset digest and ready sample IDs, STT endpoint/model/language/temperature,
scoring-policy version, and agent-review rubric version. The prompt is expected to differ.
The prior report must already be complete. Incompatible comparisons fail before STT calls instead
of producing misleading deltas; LLM-score deltas are added when the new agent review is applied.

The default report path is `<dataset>/runs/<run-id>.json`; `--output` may choose another JSON path.
Progress and the final report path may be printed to the terminal, but no prose or Markdown report
is generated.

One completed recording is one scored utterance and one per-sample result. A recording may contain
multiple grammatical sentences, but the first version does not guess sentence boundaries or align
independently segmented sentences; its WER alignment and LLM difference list still expose the exact
within-recording differences.

### 4. Agent-driven tuning loop

For each requested tuning task, the coding agent will:

1. run a baseline against all ready samples with the user's three thresholds;
2. read the awaiting-review JSON, judge each reference/result pair under the fixed 0–100 rubric,
   apply the structured agent review, and verify the resulting three aggregate values;
3. report failed thresholds, important sentence-level differences, and likely error classes;
4. prepare a new STT prompt candidate without copying reference answers into it;
5. rerun the full ready dataset, normally comparing against the current best compatible run;
6. explain aggregate changes plus improved and regressed samples; and
7. repeat until all three gates pass or stop when repeated candidates plateau, trade improvements
   for material regressions, or leave errors attributable to audio/model capability rather than the
   STT prompt.

The CLI records whether thresholds pass; the agent owns the judgment that further prompt-only work
is no longer productive.

## Dataset Design

### Directory layout

```text
my-stt-dataset/
  dataset.json
  audio/
    <sample-id>.wav
  samples/
    <sample-id>.json
  runs/
    <run-id>.json
```

`dataset.json` contains only dataset-level schema/version metadata. One sidecar per sample avoids
rewriting an ever-growing manifest during correction and makes the WAV/metadata relationship
obvious. All paths stored in dataset files are relative and must remain underneath the canonical
dataset root; absolute paths, `..` traversal, symlink escape, duplicate IDs, and duplicate audio
ownership are rejected.

The sample sidecar has this logical shape (exact field naming may be refined without changing the
contract):

```json
{
  "schema_version": 1,
  "id": "sample-1786612345678-1",
  "audio": {
    "path": "audio/sample-1786612345678-1.wav",
    "sha256": "..."
  },
  "capture": {
    "created_at_unix_ms": 1786612345678,
    "transcription": {
      "status": "success",
      "text": "初次识别结果",
      "model": "whisper-large-v3-turbo",
      "language": "zh",
      "temperature": 0,
      "prompt": "采集时使用的 STT prompt"
    }
  },
  "reference": {
    "status": "ready",
    "text": "人工校正结果",
    "proper_nouns": [
      {
        "canonical": "ViberWhisper",
        "accepted": ["Viber Whisper"],
        "case_sensitive": false,
        "expected_occurrences": 1
      }
    ]
  }
}
```

The canonical form is always accepted and need not be repeated in `accepted`. Expected occurrence
counts are explicit so repeated names have an unambiguous denominator. Validation also verifies
that the canonical/accepted forms occur that many times in the reference. It rejects empty or
duplicate accepted forms, zero/mismatched expected counts, ambiguous duplicate forms within one
sample, empty ready references, missing/mismatched audio, non-finite numeric metadata, and unknown
schema fields.

Audio and text are deliberately plaintext. Documentation will state that the configured STT service
receives audio and that the coding agent reads reference/candidate text from the local JSON while
performing the requested task. No additional Judge service is contacted. The dataset must contain
only material the user is willing to expose through those two paths. STT endpoint metadata is
sanitized before persistence by removing userinfo, query, and fragment components.

### Single-process persistence and interruption behavior

The first version assumes exactly one prompt-lab process owns a dataset at a time. Recording,
correction, evaluation, and review application must not overlap with another process using the same
root. The suite does not add a lock file, reader/writer coordination, generation checks, or atomic
replace protocol. This is an explicit operational contract rather than best-effort concurrency.

Prompt-lab mode adds an optional session archive beside the existing live `WavChunk` path. The
audio callback remains free of disk I/O and network work. As complete chunks leave the recorder,
shared immutable chunk bytes are sent to a session-owned archive worker, which validates a stable
WAV format and appends samples directly to the sample's final WAV path. The stop-time tail follows
the same path. Finalization closes the writer, computes the digest, and writes/updates the matching
sample sidecar directly.

Capture state is keyed by `SessionId`; stale chunks and stale finalization cannot attach to another
sample. A process interruption may leave a truncated WAV, malformed JSON, an unreferenced WAV, or a
sidecar whose digest no longer matches. `dataset validate` reports these conditions without repair;
they can never become `ready` or enter regression until the user deletes the damaged sample or
records/corrects it again. Normal listener mode does not construct the archive worker and keeps its
current in-memory behavior and cost.

## Scoring Contract

### 1. Word error rate

Each ready sample is normalized and tokenized by one versioned, deterministic policy:

- Unicode compatibility normalization is applied first;
- punctuation and whitespace separate tokens but do not become scored words;
- a reference containing Han characters uses a fixed bundled Jieba dictionary with HMM disabled;
- other text uses Unicode word boundaries; and
- Latin text is compared case-insensitively while original text remains in the report.

The scorer uses standard Levenshtein alignment and records substitutions (`S`), deletions (`D`),
insertions (`I`), and reference word count (`N`). Per sample:

```text
WER = (S + D + I) / N
```

Dataset WER is the micro-average `(sum S + sum D + sum I) / sum N`, expressed as a percentage.
It is not clamped and may exceed 100% when insertions outnumber reference words. Empty ready
references are invalid, avoiding an undefined denominator. The report preserves the aligned edit
operations needed to explain each difference.

### 2. LLM similarity score

The coding agent evaluates each human reference/fresh STT pair after the program writes the initial
run JSON. While scoring, it uses only those two texts and a fixed, versioned rubric; it must not use
the STT prompt, proper-noun annotations, prior score, thresholds, or baseline result to justify a
score. The rubric defines:

- `100`: meaning and factual content are fully equivalent; only immaterial formatting differs;
- `90–99`: very small non-semantic wording, punctuation, or casing differences;
- `70–89`: core meaning is retained but one or more recognizable errors remain;
- `40–69`: omissions/substitutions materially change part of the meaning;
- `1–39`: most important content is wrong or missing; and
- `0`: no usable correspondence.

The agent-authored review JSON must contain an integer `score` in `0..=100`, a concise reason, and
structured difference entries for every sample. `apply-review` validates exact run/sample identity
and complete coverage; invalid or out-of-range input never changes the canonical report. Dataset
LLM score is the arithmetic mean of all accepted per-sample scores, so each completed recording has
equal weight.

The completed report records `review_source="coding_agent"` and the rubric version, but no Judge
endpoint, model, temperature, or credential. The same rubric version is required for comparable
runs. Because agent judgment can still vary, reasons and structured differences remain stored next
to every score for review rather than presenting the number as a deterministic local metric.

### 3. Proper-noun accuracy

For each annotation, the scorer searches the fresh STT text for the canonical form or any accepted
form under its declared case policy. Compatibility normalization is applied; alphanumeric forms
must respect token boundaries, and overlapping matches are consumed longest-first so one span
cannot satisfy two expected occurrences.

Per annotation, matches are capped at `expected_occurrences`. Dataset proper-noun accuracy is the
micro-average:

```text
proper noun accuracy = sum matched expected occurrences / sum expected occurrences
```

Samples without annotations do not enter this denominator. Because all three thresholds are
required, evaluation preflight rejects a ready dataset containing zero expected proper-noun
occurrences instead of reporting a misleading 100%. Per-sample JSON lists matched and missed
canonical names and the accepted form that matched.

### Three independent gates

The report contains no combined score. A complete run sets `meets_targets=true` only when:

```text
WER percentage <= max_wer_percent
LLM mean score >= min_llm_score
proper-noun percentage >= min_proper_noun_percent
```

Thresholds are validated before requests: WER must be non-negative, and both 100-point metrics must
be in `0..=100`.

## Regression Execution and Failure Semantics

Evaluation snapshots and validates the sorted ready sample set before making any API call. It
computes a dataset digest only from scoring inputs: schema/scoring version, ready sample IDs, audio
SHA-256 values, reference text, and proper-noun annotations. Initial capture transcripts and other
non-scoring metadata cannot invalidate a comparison. Later scoring-input changes cannot silently
alter the identity of an in-flight run.

Each sample is then processed in stable ID order:

```text
historical WAV
  -> current production WavChunkReader policy
  -> fresh STT requests carrying the candidate prompt
  -> existing language-aware chunk merge
  -> WER + proper-noun scorer
  -> awaiting-agent-review JSON
  -> agent scores every reference/result pair
  -> validated complete JSON
```

The first version is sequential to make provider load, report ordering, failure behavior, and cost
obvious. Existing STT timeout/retry behavior still applies to each chunk. There is no second HTTP
client or external semantic-evaluation request.

Processing continues after a sample failure so the report captures every reachable issue. If any
ready sample fails STT or merge, the run is written with `status="incomplete"`,
`meets_targets=null`, and a non-empty failures array, then the command exits nonzero. Aggregate
local values may be included as diagnostics but the report cannot accept agent review, compare
against thresholds, or serve as a baseline. When all fresh STT results and local metrics succeed,
the command exits successfully with `status="awaiting_agent_review"`; it remains unable to pass or
serve as a comparison source until a valid complete agent review is applied.

## JSON Run Report

The versioned report contains:

- run ID, timestamps, `incomplete`/`awaiting_agent_review`/`complete` status, and scoring/rubric
  versions;
- dataset digest, ready sample IDs, and pending/invalid sample counts;
- sanitized STT endpoint, model, language, temperature, exact candidate prompt, and prompt digest;
- the three requested thresholds;
- WER edit totals and micro-average percentage;
- proper-noun matched/expected totals and micro-average percentage;
- empty agent-review fields while awaiting review, then review source/rubric, mean LLM score, scored
  sample count, per-sample score/reason/differences, and review timestamp after `apply-review`;
- `meets_targets` plus the individual gate results after review;
- for every sample: audio digest/path, reference, fresh hypothesis, WER alignment/counts,
  proper-noun matches/misses, agent review when present, and any error; and
- when requested, compatible aggregate and per-sample deltas against the prior run.

Reports and capture sidecars never contain STT credentials. The single owning process writes
pretty-printed JSON directly to its final path and rewrites that path after agent review. An
interrupted/malformed report is rejected on the next read and cannot serve as a comparison
baseline; there is no automatic recovery or Markdown mirror.

## Architecture and Module Layout

```text
src/
  prompt_lab.rs                    — domain facade, common errors, versioned public contracts
  prompt_lab/
    dataset.rs                     — layout, sample schemas, direct correction, validation, digest
    capture.rs                     — session archive worker and capture result persistence
    metrics.rs                     — normalization, WER alignment, proper-noun matching
    review.rs                      — agent-review schema, validation, aggregation, report finalizing
    regression.rs                  — full-dataset runner, thresholds, comparison, JSON report
  application/
    prompt_lab.rs                  — CLI command handlers and prompt-lab listener assembly
```

`core::cli` only parses command intent. `application::prompt_lab` loads configuration, resolves the
selected backend, starts Local when needed, and coordinates domain services. Dataset validation,
metrics, agent-review validation, and report schemas do not depend on winit or native platform
types.

The normal listener and prompt-lab recorder share the recording state machine, tray/hotkey mapping,
audio recorder, and session orchestrator. A small application-private listener output mode keeps
normal final delivery (`PostProcessor` -> history -> native typer) separate from prompt-lab capture
(`raw STT result` -> sample store). It carries `SessionId` explicitly and does not overload
`TextTyper` with dataset persistence.

`TranscriberConfig` remains module-owned. Runtime assembly adds a prompt-lab constructor that can
replace only its prompt in memory and also returns redacted metadata for the report. The persisted
v2 configuration schema and field catalog do not gain prompt-lab paths, thresholds, or reviewer
configuration.

## Planned Change Stack

The approved plan remains the first change. Implementation is expected to use these natural child
boundaries; they may be combined only if review shows that a boundary has no standalone behavior or
rollback value.

### 1. `feat(prompt-lab): collect and curate STT samples`

- add dataset schemas, path containment, validation, digesting, and single-process sidecar
  correction;
- add the session-scoped WAV archive and prompt-lab listener output mode;
- add `record`, `sample list/show/correct`, and `dataset validate` commands;
- prove prompt-lab capture saves raw STT without post-processing, normal history, or injection;
- keep capture/dataset tests and the README/audio/core architecture updates that describe this
  available behavior in the same change.

This change is independently useful as a reusable, human-corrected audio dataset builder.

### 2. `feat(prompt-lab): score full prompt regressions`

- add the in-memory prompt override, full historical-WAV runner, three metric implementations,
  agent-review application, thresholds, compatible-run comparison, and direct two-phase JSON
  reports;
- add focused metric/review/report tests plus end-to-end fake-STT regression tests;
- add the prompt-lab architecture document, documentation index entry, final README workflow,
  `AGENTS.md` structure update, plan status, and changelog entry.

This change depends on the dataset contract from change 1 and is independently revertible without
invalidating already captured WAV/reference samples.

## File Impact

| File | Planned change |
|---|---|
| `Cargo.toml`, `Cargo.lock` | Add narrowly scoped hashing, Unicode normalization/boundary, and deterministic Chinese tokenization dependencies if the standard library cannot satisfy the locked scoring contract. |
| `src/lib.rs` | Register the private prompt-lab domain module. |
| `src/prompt_lab.rs`, `src/prompt_lab/*.rs` | Add dataset, capture, local metrics, agent-review validation, regression, comparison, errors, and versioned JSON contracts. |
| `src/core/cli.rs` | Parse the `prompt-lab` recording, sample, dataset, and evaluation command tree and its mutually exclusive prompt/reference inputs. |
| `src/application.rs`, `src/application/prompt_lab.rs` | Dispatch prompt-lab workflows, apply agent reviews, start the selected STT backend, and print report/sample paths. |
| `src/application/listener.rs`, `src/application/listener/event_loop.rs` | Reuse native recording controls with an explicit capture output mode while preserving normal delivery behavior. |
| `src/audio.rs`, `src/audio/recorder.rs`, prompt-lab capture module | Feed complete session chunks to the optional archive worker without adding callback disk I/O or cost to normal mode. |
| `src/runtime_config.rs`, `src/transcriber.rs`, `src/transcriber/api.rs` | Build an in-memory prompt override and redacted run metadata without mutating persisted configuration or changing STT wire behavior. |
| `README.md` | Document dataset capture/correction/evaluation commands, three metric meanings, agent-driven loop, plaintext/privacy boundary, and JSON-only output. |
| `docs/architecture/audio.md`, `docs/architecture/core.md`, `docs/architecture/transcriber.md` | Record optional archive ownership, CLI/runtime assembly, prompt override, and fresh-request regression behavior. |
| `docs/architecture/prompt-lab.md`, `docs/README.md` | Add the focused architecture reference and make it discoverable. |
| `AGENTS.md` | Add the prompt-lab modules and supported workflow to current project guidance after implementation. |
| `docs/plan/33-stt-prompt-regression-suite.md` | Record approval, implementation status, and material deviations. |
| `changelog` | Record the user-visible prompt-lab dataset and regression CLI. |

`config.example.json`, the strict v2 schema, post-process architecture, normal `history.jsonl`
format, native typer/tray contracts, packaging workflow, and release runbook are not expected to
change. Prompt-lab settings are invocation-scoped, its dataset is user-selected, and no new
production configuration key or packaged native asset is introduced.

## Test Strategy

Implementation starts with failing tests at each behavior boundary.

### Dataset and correction

- initialize a missing dataset and strictly reject unknown/newer schema versions;
- allocate collision-safe IDs and keep every sidecar paired with exactly one contained WAV path;
- reject absolute paths, parent traversal, symlink escape, duplicate IDs/audio ownership, missing
  WAVs, and digest mismatches;
- round-trip multilingual transcripts, newlines, quotes, accepted proper-noun forms, and repeated
  occurrence counts exactly;
- keep invalid corrections pending and perform no write until the replacement passes full
  in-memory validation;
- detect pending/invalid samples without allowing them into the ready-set digest; and
- detect truncated WAVs, malformed/truncated sidecars, digest mismatches, and unreferenced WAVs
  after an interrupted direct write without attempting automatic repair.

### Capture mode

- combine several same-format live/tail WAV chunks into one complete ordered recording;
- reject or surface mixed/corrupt chunks without publishing a ready sample;
- route archive chunks and finalization by `SessionId`, ignoring stale session data;
- persist successful, partial, failed, and interrupted initial STT states without treating any as a
  reference;
- document and test the one-process-per-dataset precondition without adding a dataset lock;
- prove capture output is raw STT and does not call post-processing, history persistence, or the
  native typer; and
- prove normal listener mode does not construct an archive worker and retains existing finalization,
  history, injection, cancellation, and shutdown behavior.

### Metrics

- verify exact-match WER zero and known substitution/deletion/insertion alignments;
- verify dataset WER uses summed edits/reference tokens rather than a mean of sample percentages;
- cover Chinese segmentation, mixed CJK/Latin text, compatibility forms, punctuation, whitespace,
  casing, repeated words, and WER above 100%;
- cover proper-noun canonical/accepted forms, case policies, token boundaries, overlaps, repeated
  expected occurrences, misses, and samples with no annotations;
- reject a regression dataset with no proper-noun denominator; and
- snapshot the scoring-policy version with deterministic fixture results so a tokenizer/rule change
  must deliberately bump report compatibility.

### Agent review

- write `awaiting_agent_review` with empty review fields after successful STT/local scoring;
- accept only a versioned review whose run ID and sample IDs exactly cover the target report;
- parse strict integer scores plus non-empty reasons and structured differences for every sample;
- reject missing/duplicate/unknown samples, malformed JSON, fractional/out-of-range scores, rubric
  mismatch, incomplete target runs, and a second review of an already complete report;
- leave the original report byte-for-byte unchanged after invalid review input; and
- directly rewrite the report after full review validation, compute the LLM mean and three gates,
  and mark it complete without any network call or reviewer credential.

### Regression and reports

- recreate chunks and issue fresh STT calls for every ready audio file on every run;
- apply the candidate prompt to every multipart chunk while leaving model/language/temperature
  unchanged and bypassing post-processing;
- preserve stable sample ordering and exact reference/hypothesis/difference data in JSON;
- calculate all three gates at their inclusive boundaries and never build a combined score;
- write `incomplete` plus failures and exit nonzero when fresh STT cannot complete;
- keep `meets_targets` null until a complete validated agent review is written;
- validate comparison compatibility before requests and report improvements/regressions against a
  compatible complete run; and
- prove no Markdown/report sidecar and no persisted config mutation are produced.

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

Interactive validation on macOS and Windows:

1. Capture Hold and Toggle samples and verify each session creates one playable complete WAV plus
   one matching sidecar without typing or entering normal history.
2. Correct pending samples containing Chinese, English, repeated proper nouns, aliases, numbers,
   and punctuation; verify list/show/validate status changes.
3. Run a configured-prompt baseline and a different prompt candidate; inspect the JSON to confirm
   every ready historical WAV was freshly transcribed and per-sample differences are complete.
4. Compare compatible runs, then change one dataset sample and verify the old comparison is rejected.
5. Trigger an STT failure and verify an incomplete JSON remains inspectable; then try invalid,
   partial, and out-of-range agent reviews and verify none can produce a threshold result or mutate
   the report.
6. Apply a complete agent review, verify all three gates and prior-run deltas, and confirm the
   dataset/report contain no API keys and no external Judge request is made.

Hosted macOS and Windows CI must pass on both code-bearing changes before the draft PR becomes
ready.

## Acceptance Criteria

- `prompt-lab record` uses existing native recording controls and saves one complete WAV plus one
  raw initial STT record under the same collision-safe sample ID for every archiveable session.
- Prompt-lab recording never invokes LLM post-processing, normal transcription history, or text
  injection; normal listener behavior remains unchanged.
- A sample becomes ready only through an explicit, validated human reference correction, and its
  proper nouns support canonical spelling, accepted variants, case policy, and expected count.
- Every evaluation uses all ready historical recordings, rereads their verified WAVs, makes fresh
  STT requests with the candidate prompt, and never sends reference answers to STT.
- Reports expose exactly the agreed primary metrics: micro-average WER, mean 0–100 LLM similarity,
  and micro-average proper-noun accuracy. They do not combine them into a single score.
- A run passes only when all three user-supplied thresholds pass; incomplete and
  awaiting-agent-review runs cannot pass or serve as comparison baselines.
- The coding agent, not a configured Judge API/model, supplies each 0–100 semantic score, reason,
  and structured difference under a fixed versioned rubric; validated review application makes no
  network call.
- Each run produces one directly written, versioned JSON report that progresses from local results
  awaiting agent review to final aggregate values, per-sample differences, redacted metadata,
  dataset/prompt identity, optional compatible deltas, and no Markdown mirror.
- One prompt-lab process owns a dataset at a time. No locking or atomic publication is implemented;
  validation excludes interrupted/truncated files until the user removes or rebuilds them.
- The coding agent can read the JSON, tell the user the result, revise a prompt file, and repeat the
  loop without changing product configuration or code between candidates.
- Dataset/report plaintext and external-service privacy boundaries are documented; no credential is
  written to dataset files, reports, or normal logs.
- Automated tests, local validation, hosted cross-platform CI, documentation synchronization, and
  native capture checks complete before the draft PR becomes ready.
