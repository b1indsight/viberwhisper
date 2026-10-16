# Realtime dictation

`dictation.enabled` selects the generic OpenAI Realtime WebSocket path. Configuration is
file-only. `core::recognition` builds a `SessionOrchestrator` with an HTTP or Realtime worker.
The orchestrator owns active-session identity, routing, start/finish/cancel and result delivery
for both paths. Delivery disables the old LLM postprocessor.
Raw prompt-lab capture deliberately retains the HTTP transcriber.

## Ownership

- `config`: typed optional v3 configuration, endpoint validation, environment authentication,
  strict prompt components, full-string overrides and user-owned long-term terms. Debug output omits content.
- `engine`: one persistent async worker and reusable connection, recording-scoped I/O and cancellation,
  bounded backlog, ordered 30-second response segments, final text concatenation without deduplication.
- `audio::PcmResampler`: shared stateful 24 kHz conversion with filter-delay and final-tail compensation.
- `transport`: standard session/response events, explicit audio-item references and context input,
  out-of-band responses, correlation, delta/final consistency, completion status and item deletion.
- `result`: strict complete JSON boundary and independently validated optional context/term metadata.
  Delivery uses the returned final text; no separate verbatim transcript is requested.
- `feedback`: a single-use confirmation capability; dropping it discards learning, successful
  delivery returns edited text and waits briefly for the worker's persistence acknowledgement.
- `correction`: bounded grapheme alignment, whole-candidate feedback, reversible context-scoped
  single-pass replacement and word boundaries. Source, shown and confirmed text remain distinct.
- `memory`: 24-hour observations with provenance and independent-recording support counts; bounded
  per-topic recent history, candidate retrieval, persistent user corrections and user-configured
  terms; one writer and atomic storage.
- `offline`: conversion and regression with frozen context and no live-memory persistence.

## Prompt configuration

`assets/dictation-prompt.default.json` is the sole source of default instruction text. The JSON
resource is embedded for portable macOS/Windows packaging; Rust contains only its strict seven-field
type, source selection, validation and composition. Users edit `dictation.prompt_components` in
their existing config file and restart; no compilation or auxiliary files are needed.

Components are joined in the fixed order task, language, numbers, cleanup, output_format, context,
memory. Each string is trimmed at its edges; empty components are skipped; remaining components
are separated by a newline. Content is literal, with no template substitutions. All seven keys
are required for a custom object; unknown keys and non-string values are rejected.
The final instructions retain the 4096-Unicode-character limit, including separators.

The bundled default distributes the selected candidate's rules across all seven components.
`cleanup` includes word-fidelity, correction and independent-negation examples;
`output_format` includes JSON examples for questions and unfinished fragments. Context selection
and memory extraction remain in their respective fields. Composition follows the fixed order
above, so the instructions are reordered relative to the full-string prompt used in evaluation.

`prompt` and `prompt_components` default to null. With both absent/null, the embedded JSON provides
the default components. A full-string `prompt`, including an empty string, replaces all components;
explicitly configuring both sources is an error. Old full-string configs remain valid and are
not rewritten on read. Config examples include complete editable default components.
The same resolved string is used for session instructions, response instructions and offline
report metadata. Prompt-lab overrides operate on a cloned configuration, clear component selection
and disable personal memory; saved configuration is untouched.

## Audio and lifecycle

The existing recorder emits 200 ms WAV envelopes internally in Realtime mode. This avoids adding
a second recorder/callback protocol; shared `audio::decode_mono` uses the existing validated WAV
reader, and the network worker maintains one resampler per recording. The small headers never reach the service. The microphone callback only
collects/downmixes input and notifies readiness; network and inference stay off the UI thread.

Recorder backlog and the 300-frame worker channel are bounded separately. Overflow is an explicit
failure and stops recording on the next readiness event. Fault-triggered cancellation retains
already completed, validated segment text as a partial failure; explicit user cancellation
suppresses it. Both discard the recording's memory updates. The existing fault reason distinguishes
these cases without introducing another cancellation state. During response generation, subsequent
frames queue locally. No unbounded server or client queue is assumed. Audio up to each segment's
end is uploaded before commit; stop flushes the resampler and retains short voiced tails. Complete
segments are checked using the shared energy/VAD gate; silent segments clear the server buffer
without generating text. Already-uploaded frames are not retrospectively normalized or removed.

The service must accept `max_output_tokens=4096`, text delta/done events, cancellation and
item deletion. `response.create` uses explicit `input`/`item_reference`, `conversation=none`,
correlation `metadata` and `instructions`; structured JSON is a validated prompt contract.
Each commit creates an audio item. A single `response.create` references that item and a frozen
snapshot of relevant context, with `conversation=none`. `response.done` must be completed and
match the final text before delivery/learning. Validated text is retained before deleting the
finished audio item. Cleanup errors/timeouts retire the connection and return completed text as
a partial failure, without learning from the recording. Cancellation reuses the same item cleanup.
Each recording
works on a copy of memory; cancellation/failure discards its updates. Successful desktop recordings
retain source metadata with a single-use feedback handle. Only successful preview delivery commits
observations rebuilt from the final edited text and saves memory atomically. Dropping the handle,
clearing or cancelling the preview discards the recording. The worker answers connection control
traffic while awaiting feedback and processes the next recording only after feedback or discard.
The connection persists across successful recordings and topics; interrupted
or failed connections are retired without ambiguous audio replay.

Short-term observations are separate from config and ordinary history.jsonl. Repeated segments
from one recording do not count as independent support. A corrupt memory file falls back to
ephemeral state without overwriting it. A disabled memory configuration does not load the file.
Reference context is bounded to four topics and 6000 serialized characters, prioritizing manual
long-term terms, user corrections, model candidates, then history. Configured contexts are selected first in config order
(at most 16 configured terms per context). Budget trimming removes history across all contexts,
then candidates, then corrections, before touching configured vocabulary. There is no separate current-topic cache:
unexpired observations, persistent corrections and configured terms supply labels, including in long-running processes.
Model-returned observations of already
known terms refresh their short-term lifetime; merely including a term in the prompt does not.
Uncertain/new topics are model decisions, not proof
of accurate classification; unknown existing IDs are rejected. Candidates contain only kind and text;
no evidence field is required. Candidates must occur as a substring of the delivered text after
NFKC normalization and lowercasing. Unsupported candidates are discarded without losing text;
the original JSON remains available for prompt-lab metadata validation. This checks textual
support, not acoustic accuracy or whether a phrase is suitable for future memory.

The schema-2 memory file adds at most 512 persistent corrections with context, source spelling,
target spelling, recording provenance and last manual-edit time; v1 model observations retain their
original expiration. Confirmed proper-noun edits create whole-word rules only when alignment and
topic assignment are unambiguous. Model candidates are re-aligned to the final text; removed or
unassignable candidates are dropped. Phrase candidates remain short-term observations. Expensive
rewrites exceed a fixed alignment-cell budget and skip learning. Live text is always delivered
unchanged by feedback validation.

Snapshots expose correction pairs without provenance. A matching response topic enables local,
single-pass replacements before preview only within complete proper-noun candidates extracted by
that response. Missing candidates and phrase candidates never authorize local text replacement.
Learning and application share the same candidate spans: a containing name owns nested fragments;
crossing overlaps without one enclosing candidate are skipped. Rules match the entire candidate,
so a rule for a shorter name cannot edit part of a longer extracted name. Configured vocabulary
protects its full occurrences in that topic, including against intersecting candidate replacements;
the same rule can still apply outside those occurrences. Ambiguous or cyclic rules are skipped. Reverting a
displayed correction removes its rule, and editing it to another spelling updates its original
source mapping. Unedited automatic replacements do not refresh manual evidence. Disabling memory
also disables correction loading, learning and application. Offline and seeded evaluation retain
their existing isolation and never load the personal correction table.

While awaiting microphone frames, the existing worker also polls the connection for incoming
control traffic. Ping receives Pong and server errors fail the recording before stop. Other
protocol events are retained in order for their consumer, with a 32-event bound; exceeding it
fails explicitly rather than dropping events or growing memory without limit. No extra reader
task or separate connection lifecycle is introduced.

## Validation boundaries

One worker integration test exercises two recordings totaling 66 seconds on one socket, four
responses and repeated deltas. Focused regressions cover truncated responses and cancellation
before the response ID has been received. Live tests are opt-in via
`VIBERWHISPER_REALTIME_TEST_URL`. Standard protocol mock success is not a claim of live-model
accuracy or performance. The current Gemma server rejects the standard 4096-token configuration
and its source lacks further response controls; real quality/latency comparisons await that service.

Realtime's sampling behavior is provider-owned. The legacy report's numeric temperature metadata
does not imply that a Realtime temperature parameter is sent. Prompt metadata includes the full
the exact resolved task instructions. Realtime evaluation disables personal memory and overrides
the whole prompt (including an empty prompt), rather than appending supplemental instructions.
HTTP and Realtime use the same evaluation, metrics,
report writing and review flow.
Evaluation and report loading accept sanitized HTTP(S) and WS(S) endpoint metadata; userinfo,
query strings and fragments remain forbidden. Live delivery, conversion, setup verification and
prompt-lab use the shared `text::merge_texts` helper with `transcription.language`: Chinese
language codes add no separator, while other or unspecified languages add a space. Repeated
speech is preserved. Realtime metadata records the language used for joining; it is not sent
as a recognition-language constraint to the server.
