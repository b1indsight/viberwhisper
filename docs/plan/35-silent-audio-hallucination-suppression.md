# 35 - Silent-Audio Hallucination Suppression

## Status

**Implemented on 2026-08-27.** The two planned child changes and one review-follow-up child change
were completed. Formatting, tests, checks, Clippy, the native build, and the Windows GNU
cross-check passed. The private silent diagnostic was suppressed before upload with empty output.
A private speech sample crossed the gate and reached the configured STT endpoint; the endpoint
then returned 401 for the current API key, while the local HTTP regression independently verified
the complete audible upload/response path.

Review simplified the approved 20 ms plus accumulated-100 ms policy to one 50 ms RMS window above
-50 dBFS. This deliberately trades more false-positive uploads for less code and a lower risk of
suppressing short utterances: any audible window opens the gate, while invalid or uncertain local
analysis also fails open. The provider-independent boundary, threshold, empty-result contract, and
all workflow coverage remain unchanged.

## Context

Prompt-lab testing exposed a repeatable failure mode: a several-second microphone capture with no
intentional speech was still sent to the configured STT endpoint, which returned a fluent stock
sentence instead of an empty transcription. Prompt instructions explicitly asking for empty output
on silence did not change that result. Prompt wording is therefore not a reliable safety boundary
for this case.

The application already treats an empty realtime transcription as a normal no-op: it skips LLM
post-processing, history persistence, and text injection. Ordered chunk merging also ignores empty
segments. What is missing is a provider-independent decision before the request. Every production
backend, including Local mode, reaches the same `ApiTranscriber` with a complete in-memory
`WavChunk`, so that boundary can suppress near-silent chunks without changing recording,
orchestration, or provider response schemas.

The observed diagnostic capture and the existing speech samples have a large energy separation,
but one private dataset is not sufficient evidence for a general speech detector. This change will
therefore implement only a conservative low-energy gate for effective silence, with explicit
synthetic boundary tests and fail-open behavior. It will not claim to recognize every kind of
non-speech noise.

## Goals

1. Detect complete WAV chunks that contain too little sustained audio energy to plausibly carry an
   utterance, before any STT network request is built.
2. Return the existing successful empty-string result for a suppressed chunk so realtime and
   offline multi-chunk merging keep voiced chunks in order and do not enter retry/error paths.
3. Ensure an entirely suppressed realtime recording produces no post-processing, history entry, or
   injected text, and an entirely suppressed `convert` input cannot reach LLM post-processing.
4. Cover API and Local profiles, live recording, offline conversion, and prompt-lab through their
   existing shared `ApiTranscriber` boundary.
5. Keep the policy deterministic, cross-platform, dependency-free, and deliberately conservative
   about quiet speech.

## Non-goals

- General voice-activity detection, speaker detection, noise classification, or removal of music,
  fans, clicks, and other audible non-speech input.
- Inspecting or trusting provider-specific `no_speech_prob`, segment metadata, or hallucinated-text
  blacklists.
- Changing the STT prompt, model, temperature, response schema, retry budget, chunk sizing, or LLM
  cleanup prompt.
- Adding a persisted threshold/configuration field, CLI flag, calibration workflow, native audio
  dependency, or model download.
- Changing prompt-lab's rule that a ready sample needs a non-empty human reference, or promoting a
  silent diagnostic sample into WER/semantic scoring.
- Reclassifying malformed WAV data as silence; classifier failure must preserve the current upload
  and error behavior.

## Technical Approach

### 1. Classify sustained low energy in the audio module

Add one pure signal-classification function beside `WavChunk` rather than teaching the recorder,
orchestrator, or HTTP client how to decode WAV samples. The classifier will:

- decode the complete in-memory WAV with the existing `hound` dependency;
- normalize integer and float samples to full-scale amplitude while preserving channel/frame
  timing;
- divide the audio into 20 ms windows and compute RMS energy per window;
- consider a chunk audible only after at least 100 ms of windows exceed a conservative internal
  threshold of -50 dBFS;
- stop scanning once the required active duration is reached; and
- return a decoding error rather than guessing when the WAV cannot be analyzed.

The 20 ms window prevents one isolated sample peak from defeating the gate, while accumulated
active duration permits natural micro-pauses and short utterances. These constants are
module-owned policy, not user configuration. The first implementation will change them only with
new test/field evidence; it will not add speculative calibration machinery.

The classifier name and documentation will describe **audible/speech-like energy**, not semantic
speech detection. A quiet but valid utterance near the boundary is the primary false-negative risk,
so the threshold remains intentionally below ordinary active speech levels and synthetic tests
cover values immediately on both sides.

### 2. Gate the shared transcriber before upload

At the start of `ApiTranscriber::transcribe`:

- an effectively silent chunk logs compact signal metadata and returns `Ok(String::new())` without
  constructing or sending an HTTP request;
- an audible chunk follows the existing upload, retry, parsing, and trimming path unchanged; and
- a classifier/decode error logs a warning and fails open into the existing upload path.

No `NoSpeech` error variant or new outcome type is needed. Empty text is already the application's
normal no-output contract, and a suppressed chunk must not consume retry/convergence failure
budgets. Because both remote API and Local profiles use `ApiTranscriber`, the gate applies to both
without endpoint-specific branches.

Per-chunk gating is intentional. A long recording can contain silent and voiced chunks; silent
chunks become empty successful segments, while the existing ordered merge retains every voiced
result. Waiting until session completion would be too late because live chunks are transcribed in
the background during recording.

### 3. Keep every empty-result consumer side-effect free

Realtime finalization already returns before post-processing, history persistence, and text
injection when the merged STT result is empty. Preserve that path and add focused regression
coverage at the shared/orchestrator boundary rather than introducing a parallel silence state.

Offline `convert` currently invokes its post-processor even when all chunk texts merge to empty.
Add an explicit empty-result short circuit there so a suppressed WAV yields empty stdout or an
empty requested output file without making an LLM request. Prompt-lab evaluation already bypasses
post-processing; prompt-lab capture will keep archiving the WAV and record its existing empty-STT
failure state, but it will no longer store a provider hallucination for an effectively silent
capture.

## Proposed Change Stack

The approved plan remains the first change. Implementation will use two child changes so the pure
audio policy can be reviewed independently from its behavioral integration.

### 1. `feat(audio): classify effectively silent WAV chunks`

- add the windowed normalized-RMS classifier using `hound` and module-owned constants;
- cover integer and float WAVs, channel/frame timing, empty/zero/sub-threshold audio, threshold
  boundaries, isolated impulses, minimum active duration, and malformed input;
- expose only the narrow crate-internal classification result needed by the transcriber.

This change adds deterministic classification behavior but does not yet alter any STT request.

### 2. `feat(transcriber): skip effectively silent audio`

- call the classifier before upload and return an empty successful result for suppressed chunks;
- prove with a local HTTP stub that silence sends zero requests, audible input still uploads once,
  and classifier errors retain the existing upload behavior;
- lock down empty-segment orchestration/merge behavior and short-circuit empty offline conversions
  before LLM post-processing;
- synchronize user and architecture documentation, this plan's status, and the changelog.

## File Impact

| File | Planned change |
|---|---|
| `src/audio.rs` | Declare and narrowly export the signal classifier. |
| `src/audio/signal.rs` | Decode WAV samples, apply the windowed energy policy, and host focused unit tests. |
| `src/transcriber/api.rs` | Gate `transcribe` before upload and add zero-request/audible/fail-open HTTP tests. |
| `src/core/orchestrator.rs` and/or `src/text.rs` | Add the smallest regression proving empty chunks remain successful and do not disturb voiced ordering. |
| `src/application.rs` | Skip LLM post-processing when offline STT merging produces empty text. |
| `README.md` | Describe provider-independent silent-audio suppression and empty-output behavior. |
| `docs/architecture/audio.md` | Document signal classification ownership, format handling, and fixed policy. |
| `docs/architecture/transcriber.md` | Document pre-upload gating, empty success, and fail-open behavior across profiles/workflows. |
| `docs/plan/35-silent-audio-hallucination-suppression.md` | Preserve the approved design and record implementation status or material deviations. |
| `docs/README.md` | Index this plan and track its status. |
| `changelog` | Record silent-upload suppression and its no-output behavior. |

Configuration/schema files, dependencies, provider response parsing, retry errors, recorder capture,
session state, post-processing implementation, history storage, platform injection code, and
prompt-lab schemas/metrics are not expected to change.

## Test Strategy

Implementation starts with failing tests at the two new behavior boundaries.

### Signal policy

- Classify digital silence and sustained samples below -50 dBFS as not audible.
- Classify samples just above the threshold only after the accumulated active duration reaches
  100 ms; keep shorter impulses suppressed.
- Exercise representative integer widths, float WAV data, sample rates, and multiple channels so
  normalized energy and duration do not depend on encoding layout.
- Reject malformed WAV bytes from the classifier instead of treating them as silence.
- Keep fixtures synthetic and generated in memory; no private recording enters the repository.

### Transcriber and consumers

- Use the existing local HTTP stub to assert an effectively silent valid WAV returns `Ok("")` and
  the server observes zero requests.
- Assert an audible valid WAV follows the normal multipart path and returns the server text.
- Assert a classifier failure still attempts the request and preserves its existing result/error.
- Verify ordered merging ignores suppressed empty chunks between voiced chunks and that an
  all-empty successful session finalizes as empty rather than `PartialFailure`.
- Verify offline empty STT bypasses post-processing and retains the documented empty output.

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

Then rerun the existing private silent diagnostic through the production offline path and confirm
the gate logs suppression, sends no STT request, and produces no text. Rerun representative short,
quiet, and ordinary speech recordings to confirm they still reach STT and produce their normal
results. Private WAVs, transcripts, credentials, and prompt-lab JSON remain outside the repository.

## Documentation Impact

- Add this proposal to `docs/README.md` during Planning.
- During Implementation, update the main README because users gain visible no-output behavior for
  silent recordings.
- Update audio and transcriber architecture docs because ownership and the request boundary change.
- Add one changelog entry and mark this plan Implemented after validation.
- Do not add a configuration reference: the initial policy has no user-facing setting.
- Do not rewrite prompt-lab plans 33 or 34; they remain historical records for dataset/scoring
  behavior and the silent sample remains a private diagnostic rather than a scored reference.

## Acceptance Criteria

- A valid WAV chunk below the sustained-energy policy returns an empty successful transcription
  without any HTTP request or retry.
- Audible WAV chunks, including short speech above the minimum active duration, use the unchanged
  multipart upload and response path.
- WAV classification failures fail open and cannot silently discard input.
- Mixed silent/voiced chunk sessions preserve the order and content of voiced results; all-silent
  realtime sessions perform no LLM cleanup, history write, or text injection.
- All-silent offline conversion performs no STT or LLM request and emits only the requested empty
  output form.
- API mode, Local mode, live recording, offline conversion, and prompt-lab share the same gate with
  no provider-specific branch.
- No configuration/schema, dependency, provider response, retry, recording, session-state,
  prompt-lab scoring, history, or platform-injection contract changes.
- Automated checks pass, architecture/user documentation matches implementation, and private
  silence plus representative speech smoke tests confirm the expected split without committing
  private data.
