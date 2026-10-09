# Realtime dictation

`dictation.enabled` selects the generic OpenAI Realtime WebSocket path. Configuration is
file-only. `core::recognition` builds a `SessionOrchestrator` with an HTTP or Realtime worker.
The orchestrator owns active-session identity, routing, start/finish/cancel and result delivery
for both paths. Delivery disables the old LLM postprocessor.
Raw prompt-lab capture deliberately retains the HTTP transcriber.

## Ownership

- `config`: typed optional v3 configuration, endpoint validation, environment authentication,
  default/overridden complete task instructions and user-owned long-term terms. Debug output omits content.
- `engine`: one persistent async worker and reusable connection, recording-scoped I/O and cancellation,
  bounded backlog, ordered 30-second response segments, final text concatenation without deduplication.
- `audio::PcmResampler`: shared stateful 24 kHz conversion with filter-delay and final-tail compensation.
- `transport`: standard session/response events, explicit audio-item references and context input,
  out-of-band responses, correlation, delta/final consistency, completion status and item deletion.
- `result`: strict complete JSON boundary and independently validated optional context/term metadata.
  Delivery uses the returned final text; no separate verbatim transcript is requested.
- `memory`: 24-hour observations with provenance and independent-recording support counts; bounded
  per-topic recent history, candidate retrieval and user-configured persistent terms; atomic storage.
- `offline`: conversion and regression with frozen context and no live-memory persistence.

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
works on a copy of memory; cancellation/failure discards its updates. Successful recordings save
memory atomically. The connection persists across successful recordings and topics; interrupted
or failed connections are retired without ambiguous audio replay.

Short-term observations are separate from config and ordinary history.jsonl. Repeated segments
from one recording do not count as independent support. A corrupt memory file falls back to
ephemeral state without overwriting it. A disabled memory configuration does not load the file.
Reference context is bounded to four topics and 6000 serialized characters, prioritizing manual
long-term terms over short-term candidates. Configured contexts are selected first in config order
(at most 16 configured terms per context). Budget trimming removes history across all contexts,
then candidates, before touching configured vocabulary. There is no separate current-topic cache:
only unexpired observations and configured terms supply labels, including in long-running processes.
Model-returned observations of already
known terms refresh their short-term lifetime; merely including a term in the prompt does not.
Uncertain/new topics are model decisions, not proof
of accurate classification; unknown existing IDs are rejected. Candidates contain only kind and text; no evidence field or
text-evidence matching is required.

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
