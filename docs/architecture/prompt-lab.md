# Prompt Lab Architecture

## Purpose and boundary

Prompt lab collects a human-corrected WAV dataset and evaluates **the production recognition
pipeline**, up to the editable, pre-delivery final result. Evaluation calls
`core::recognition::RecognitionPipeline::recognize_wav`, the same assembly and finalization used
by desktop recognition. Only the audio source and host feedback differ. It does not inject text,
open a native preview, write normal `history.jsonl`, contact a Judge service, or persist candidate
prompts into application configuration. One process owns a dataset at a time.

`record` is deliberately different: it archives original audio and raw HTTP STT material for human
correction. Its captured transcription is not a full-pipeline evaluation result and is never reused
as a fresh hypothesis. This distinction also applies when capture and evaluation use the same model.

## Modules

| Module | Responsibility |
| --- | --- |
| `capture.rs` | Session-owned archive worker and sample publication |
| `dataset.rs` | Strict manifests/sidecars, correction, validation and scoring identity |
| `metrics.rs` | Versioned WER alignment and proper-noun matching |
| `production.rs` | Production runner, stage identity, safe evidence and pure validation |
| `realtime.rs` | Explicit scenarios, feedback, stage assertions and historical report replay |
| `regression.rs` | Evaluation/report contract, scoring, comparison and review gates |
| `review.rs` | Coding-agent review identity, coverage and report completion |
| `core::prompt_lab` | CLI and saved-configuration assembly |

The domain modules contain no winit or native platform types. There is no lab-specific recognizer,
fixed-block recording lifecycle, alternative segmenter, or independent cleanup implementation.
Historical schema-1/2 validation remains read-only; historical fixture generation is test-only.

## Dataset and capture

An explicit dataset root contains `dataset.json`, `audio/<sample-id>.wav`,
`samples/<sample-id>.json`, and `runs/<run-id>.json`. Owned directories are canonicalized; symlink
storage, path escape, unexpected fields, invalid versions, malformed WAVs and digest mismatches are
rejected or reported. IDs combine Unix milliseconds and a process sequence.

Only `record` creates dataset storage. Evaluation, validation and sample commands require the
manifest and all owned directories; they do not create or repair missing storage. Sample commands
check regular files and containment before reading or changing sidecars.

Capture clones complete immutable `WavChunk` values into a bounded archive worker. It validates
the stable 16-bit integer WAV format, appends samples in session order, closes and hashes the WAV,
then directly writes a sidecar with raw merged STT and sanitized request metadata. Valid audio is
retained when STT fails or is partial. Only `sample correct` supplies a human reference and proper-
noun annotations. Silence is archived without requesting hallucination-prone silent STT.
Persistence is direct rather than transactional: interrupted files and orphans are diagnosed by
`dataset validate`, not automatically repaired.

## Production execution

Validation returns one ordered scoring snapshot, including WAV hashes and valid ready sidecars.
The dataset digest covers scoring-policy version, IDs, WAV hashes, human references and proper-
noun annotations; initial capture text is excluded. A run requires at least one ready sample,
scoreable reference token and expected proper-noun occurrence.

Each WAV is a **single production session**. The shared audio adapter decodes mono PCM at its
original rate, without applying recording gain again, and feeds production buffering, pause-aware
packaging and stop-tail handling. Realtime frames are resampled and pause-segmented by the normal
worker; file read blocks do not create independent recordings. Playback is paced at 1x by audio
offset and the real monotonic clock. Production bounded queues, overflows, transport errors and
convergence deadlines remain observable; lab neither floods the queue instantaneously nor waits
privately for full queues to become writable.

The saved configuration selects exactly the production stages:

| Configuration | Execution mode |
| --- | --- |
| HTTP STT, cleanup disabled | HTTP transcription, pass-through final |
| HTTP STT, cleanup enabled | HTTP transcription → structured text cleanup |
| Realtime, cleanup disabled | Integrated structured dictation |
| Realtime, cleanup enabled | Audio-only plain transcription → structured text cleanup |

Two-stage audio receives only its selected audio instructions and audio, not history, vocabulary,
JSON examples or prior segment text. The text stage receives the actual merged raw transcription
and its own production memory snapshot. Complete nonempty recognition can be cleaned up; empty,
partial, failed or cancelled recognition follows the shared production skip policy. Cleanup failure,
invalid JSON or empty text preserves raw text with an explicit fallback status. Shared preparation
applies already confirmed local corrections before the pre-edit final is recorded.

`--prompt-file` and `--no-prompt` affect **only the selected audio stage**, not cleanup or mode.
HTTP no-prompt omits its field; Realtime sends empty instructions. With no candidate, the actual
composed production instructions are used. Saved integrated `dictation.prompt` / components
conflicts in two-stage mode are validated **before** applying an experimental override. A candidate
cannot conceal an incompatible saved configuration and does not modify that configuration.

References and annotations are passed only to scoring. Assertions and forbidden strings are local
checks, never model inputs. The runner interface receives only WAV, resolved settings and explicit
state/feedback; it cannot automatically turn the reference into confirmation text.

## Production state and explicit diagnostic scenarios

Ordinary evaluation uses main's resolved configuration, configured vocabulary and a read-only
snapshot of `dictation-memory.json`, including retained history and learned corrections. The source
is never rewritten. Disabled memory skips reading the file; missing or damaged storage uses the
same canonical recovery as main. The captured state initializes a temporary copy, not a truncated
visible-context reconstruction or the strict artificial scenario seed limits.

One production pipeline and its connections live for the entire ordered dataset, using the same
system clock as main. Each successful nonempty final is accepted unchanged, simulating Enter;
learning writes only the temporary copy and is visible to the next sample. References never enter
recognition or feedback. WAV playback replaces microphone capture, and real preview, focus, history
and native delivery are not exercised. This assumed-success feedback is not proof of native delivery.
Reports contain the initial memory snapshot and actual contexts: keep them private, especially
before sharing a report or attaching evidence to a PR.

`--realtime-cases cases.json` requires Realtime and cases **schema 3**. Every ready dataset sample
must occur exactly once. Each scenario has a separate production pipeline and explicit temporary
seed, reused by its ordered steps. These are explicitly diagnostic overrides, not ordinary
main-equivalent evaluations. Seed long-term vocabulary replaces configured personal terms;
observations are installed through production persistence, not a frozen-memory bypass. Seed
validation uses production retention, budgets and limits and rejects observations initialization
would silently discard. An isolated run is not evidence about the user's real personal state.

```json
{
  "schema_version": 3,
  "cases": [{
    "id": "project",
    "at": 100,
    "seed": {"long_term": [
      {"context": "project", "kind": "proper_noun", "text": "ViberWhisper"}
    ]},
    "steps": [{
      "sample_id": "REPLACE_WITH_READY_ID",
      "feedback": {"delivered": {"text": "使用ViberWhisper。"}},
      "expect": {
        "audio": {"audio_only": true},
        "cleanup": {
          "input": {"contexts_present": ["project"]},
          "context": "project",
          "candidates_present": ["ViberWhisper"]
        },
        "forbidden_text": ["history-only sentinel"]
      }
    }]
  }]
}
```

This example assumes the two-stage configuration and a matching recording. Replace the IDs and
assertions, and include all ready samples. The [two-step fixture](../../tests/fixtures/prompt-lab/realtime-cases.json)
also demonstrates expiration and failed delivery. `seed.observations` contains `recording`,
`segment`, `at`, `context`, `text`, and `terms`; seed recording names cannot start with `sample:`.

Feedback is `"discard"`, `"delivery_failed"`, or `{"delivered":{"text":"independent edit"}}`.
Omission means discard. Only delivered feedback invokes the production single-use confirmation.
An actual confirmation must exist; fallback, partial or memory-disabled text cleanup cannot invent
one. Integrated mode retains its production confirmation behavior even with memory disabled.
Unused handles are dropped before the snapshot barrier. Explicit feedback and that barrier finish
before the next step, so discard/failure cannot learn and delivered text can affect later correction.
These are simulated host outcomes, not proof of native application delivery. Scoring always uses
the **pre-edit final**, never the explicit delivered text.

The injected memory clock advances by real monotonic elapsed seconds during playback and inference.
`advance_seconds` is added before a step; after recognition the clock is frozen for explicit feedback
and the barrier. The next step begins there. Production network/playback deadlines still use real
monotonic time. Integrated traces retain the actual memory timestamp for each segment, including
expiration within a recording; readers require nondecreasing timestamps within the sample interval.
Pure ASR has no memory timestamp because that stage must not read a memory clock.

Assertions name `audio` or `cleanup`; legacy ambiguous `input`, `context` and candidate assertions
are rejected for new cases. Both stage inputs and feedback `memory` support context, term and
history presence/absence. History and forbidden text use exact substrings. Term assertions match
context/kind/text; optional `supporting_recordings` checks independent short-term support. Stage
context checks the raw exact string; candidate assertions check production-retained eligible names,
not invented or malformed raw array entries. Structured outputs receive strict metadata checks.
`raw_text` and `final_text` assert exact respective results. Pure ASR is a string without a synthetic
empty JSON context. Failed stage assertions remain visible and block final gates.

## Schema-3 identity, evidence and comparison

All new reports have schema 3 and execution policy `production-recognition-v2`. Identity includes:

- selected HTTP/integrated/transcription mode and per-stage input layout;
- sanitized endpoint, model, language, protocol temperature (absent for Realtime), actual composed
  prompt and prompt digest for each selected stage, plus cleanup preheat policy;
- explicit audio-candidate marker, production segmentation and 1x pacing policies;
- memory enabled switch, isolation policy and seed digest, memory-clock and feedback policies;
- scenario order/clock/assertion/feedback contract digest when applicable, and an overall digest.

Each sample preserves `raw_hypothesis`, pre-edit final `hypothesis`, recognition error and warning,
cleanup status/safe error, playback duration, stop-to-ASR convergence wait and cleanup duration.
Realtime evidence includes actual audio boundaries, stage instructions, input layout, per-segment
memory inputs/timestamps when applicable, and verified plain or structured outputs. Cleanup evidence
contains its actual input snapshot, structured output and prepared text. Before/after snapshots,
feedback and confirmation availability bind the learning transition. Snapshot timestamps and the
exact confirmation timestamp come from the production memory owner; ordinary time is never frozen
or inferred from the later report completion time. A confirmation acknowledgment covers handling
and a save attempt, not guaranteed durable storage. Reports omit audio
base64 and credentials. The evidence digest also binds sample identities, references and traces.

Report loading recomputes local metrics, validates identity/digests, stage/raw/final relationships,
and replays production Memory preparation and confirmation without files or network. It rejects
inconsistent final text, stage identity, feedback, trace inputs, clocks, snapshots and derived checks.
A partial result retains reachable text and its scores but the run remains `incomplete`: it cannot
be reviewed, become a successful baseline or pass final gates. Cleanup fallback keeps its raw
score but explicitly blocks the execution gate; it cannot masquerade as two successful stages.

`--compare-to` requires a complete compatible schema-3 baseline: same dataset/ready IDs, execution
policy, mode, noncandidate stages, memory switch, replay policies and scenario contract. Only an
explicit audio candidate may change in ordinary runs; a different initial memory snapshot requires
a new baseline. Diagnostic seed changes are listed per case. Ordinary and diagnostic runs cannot compare.
Later contexts may legitimately diverge after candidate outputs and their accepted feedback diverge;
the before/after snapshots expose that rather than treating later requests as independent samples.
Cross-mode ablations belong in separate reports, not in a same-pipeline comparison.

Historical ordinary schemas 1/2 and schema-2 `realtime-context-memory-v5` / cases-2 reports remain
readable under their original scoring/replay contract. No missing production traces are fabricated.
Old cases are rejected before networking with migration guidance. Historical and production reports
cannot compare: rerun the baseline. Earlier incompatible Realtime policies still require reruns.

## Metrics and coding-agent review

Scoring policy remains `stt-prompt-scoring-v2`:

- WER uses NFKC, bundled Jieba (HMM disabled) for Han references or Unicode word boundaries for
  others, excludes punctuation, case-folds, and records full Levenshtein alignment; aggregate WER
  is a micro-average.
- Proper nouns use NFKC, canonical/accepted forms, annotation case rules and expected counts,
  longest-first overlap consumption, and Han/non-Han word-edge rules. Aggregate accuracy is
  matched expected occurrences divided by all expected occurrences.
- Coding-agent semantic review uses `semantic-equivalence-v1`, integer scores in `0..=100`,
  nonempty reasons, and structured differences for scores below 100; the mean is unweighted.

A successful local run is `awaiting_agent_review`, with no semantic aggregate or final gates.
Production review input uses schema **2**, exact `run_id`, rubric, `execution_sha256` (the execution
identity's `sha256`), `evidence_sha256`, and every sample exactly once. Historical reviews retain
schema 1 without production digests. The entire unchanged report and review are validated before
writing; invalid input leaves the report untouched. Valid review completes the report and binds the
same identities in its review summary. Completed-report reads independently recompute review and
gates. Bounded floating-point equality permits JSON round trips without weakening exact counts or
alignment checks. Local comparison deltas are present before review; semantic deltas are filled after.

`meets_targets` requires WER at/below its maximum, semantic and proper-noun scores at/above their
minima, and any execution/scenario gate passing. There is no combined accuracy score.

## Privacy and evidence limits

Audio, references, candidate prompts, seeds, histories and traces are local plaintext. Audio is sent
to its configured audio service; optional cleanup sends real raw text and explicit test state to
its configured text service. Endpoint metadata removes userinfo, query and fragments. Error evidence
uses safe categories rather than provider bodies or private paths. Store reports appropriately:
explicit candidates and seeds may themselves contain private information.

No personal state/configuration changes, automatic reference confirmation, external Judge, native
delivery, or installed-app restart is performed. Shared-pipeline fixture tests establish production
execution contracts, not model quality, actual personal-state behavior or native preview/focus/
delivery success; those require separately authorized real-service and desktop validation.
