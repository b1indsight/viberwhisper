# Realtime dictation

`dictation.enabled` selects the generic OpenAI Realtime WebSocket path. Configuration is
file-only. `core::recognition` builds a `SessionOrchestrator` with an HTTP or Realtime worker.
The orchestrator owns active-session identity, routing, start/finish/cancel and result delivery
for both paths. Delivery disables the old LLM postprocessor.
Raw prompt-lab capture deliberately retains the HTTP transcriber.

Realtime is disabled by default. An omitted URL resolves to
`ws://127.0.0.1:8080/v1/realtime`; explicitly configured endpoints are preserved.

## Ownership

- `config`: typed optional v3 configuration, endpoint validation, environment authentication,
  strict prompt components, full-string overrides and user-owned long-term terms. Debug output omits content.
- `engine`: one persistent async worker and reusable connection, recording-scoped I/O and cancellation,
  bounded backlog, ordered 30-second response segments, final text concatenation without deduplication.
- `audio::PcmResampler`: shared stateful 24 kHz conversion with filter-delay and final-tail compensation.
- `transport`: standard session/response events, explicit audio-item references and context input,
  out-of-band responses, correlation, final-event consistency, completion status and item deletion.
- `text_stream`: provisional top-level JSON `text` decoding across escaped strings and surrogate pairs;
  nested metadata is skipped and complete-result validation remains authoritative.
- `progress`: one latest-body snapshot and coalesced wake-up per recording; closing rejects late
  updates, while cancellation also interrupts a finalizer already detached from the orchestrator.
- `result`: strict complete JSON boundary and independently validated optional context/term metadata.
  Delivery uses the returned final text; no separate verbatim transcript is requested.
- `feedback`: a single-use confirmation capability; dropping it discards learning, successful
  delivery submits edited text without waiting for persistence; the serial worker commits it before its next job.
  Normal shutdown closes pending feedback, drains accepted text and joins the worker before returning.
  After the worker returns, `Runtime::shutdown_background` prevents outstanding blocking DNS
  tasks from delaying runtime teardown; it runs after feedback handling and persistence.
- `correction`: bounded grapheme alignment, whole-candidate feedback, reversible context-scoped
  single-pass replacement and word boundaries. Source, shown and confirmed text remain distinct.
- `memory`: 24-hour observations with provenance and independent-recording support counts; bounded
  per-topic recent history, candidate retrieval, persistent user corrections and user-configured
  terms; one writer and atomic storage.
- `learning`: HTTP cleanup's serial memory owner, reusing the same snapshot, result validation,
  reversible preview and confirmation APIs without connecting to Realtime. The selected desktop
  recognition path owns the single writer; both paths use `dictation.memory` and the same schema-2
  file. See [post-processing](postprocess.md) for request and privacy boundaries.
- `offline`: conversion and regression with frozen context and no live-memory persistence.

## Prompt configuration

`assets/dictation-prompt.default.json` is the source of default Realtime instruction text and the
shared cleanup/protocol rules used by ordinary HTTP post-processing. The JSON resource is embedded
for portable macOS/Windows packaging; Rust defines its strict seven-field type, source selection,
validation, composition and a text-specific post-processing task. Users edit `dictation.prompt_components` in
their existing config file and restart; no compilation or auxiliary files are needed.

Components are joined in the fixed order task, language, numbers, cleanup, output_format, context,
memory. Each string is trimmed at its edges; empty components are skipped; remaining components
are separated by a newline. Content is literal, with no template substitutions. All seven keys
are required for a custom object; unknown keys and non-string values are rejected.
The final instructions retain the 4096-Unicode-character limit, including separators.

The bundled default distributes the selected candidate's rules across all seven components.
`task` protects short words, unfinished fragments and meaningful repetition; `cleanup` restricts
self-correction to explicitly withdrawn values while preserving independent negation, operation
objects and later clauses. `output_format` requests context, memory_candidates, then text in one
JSON object. Context selection and memory extraction remain in their respective fields, with
the first eight proper-noun occurrences requested, including repeated mentions. All 16 replacement
examples and one repeated-name example follow the memory rules at the very end, so no example
appears before a later rule. The fixed composition order and
stable/dynamic context guidance mean this default is an adaptation of the evaluated full-string
prompt, not byte-identical to it; historical recognition scores do not validate the adapted string.

`prompt` and `prompt_components` default to null. With both absent/null, the embedded JSON provides
the default components. A full-string `prompt`, including an empty string, replaces all components;
explicitly configuring both sources is an error. Old full-string configs remain valid and are
not rewritten on read. Config examples include complete editable default components.
The same resolved string is used for session instructions, response instructions and offline
report metadata. Prompt-lab overrides operate on a cloned configuration, clear component selection
and disable personal memory; saved configuration is untouched.

The default context component explicitly ties the JSON after audio to the same preceding audio.
It also explains that stable and dynamic context fields combine by ID. Custom full prompts and
saved component objects retain their exact text; authors should include equivalent guidance for
the new input order. Default-component updates do not rewrite those user-owned overrides.

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

The service must accept `max_output_tokens=4096`, text item creation/acknowledgement, text delta/done
events, cancellation and item deletion. Before the first audio append of every segment, the client
creates a user text item containing configured vocabulary. It validates the returned content and
server-assigned ID, waiting for matching `conversation.item.added` / `conversation.item.done`
(or a completed `conversation.item.created`). Instructions are acknowledged at session setup.
The vocabulary bytes remain frozen for the recording, including its later segments.

After commit, `response.create` uses explicit `input` in this order: accepted vocabulary item
reference, committed audio item reference, inline dynamic-context text. It retains
`conversation=none`, correlation `metadata`, output settings and exactly the declared instructions.
The suffix contains candidates, support counts, history and persistent confirmed corrections;
matching context IDs combine the two text parts, and vocabulary content is sent only once.
Empty parts consistently serialize as `{"contexts":[]}`. No timestamps or random IDs enter text.
The joint business snapshot stays within four contexts / 6000 characters, reserving the frozen
vocabulary before trimming runtime content. Empty dynamic-only topics cannot evict accepted terms.

`response.done` must be completed and match the final text before delivery/learning. Validated text
is retained before deleting both audio and vocabulary items and awaiting their deletion events.
Cleanup errors/timeouts retire the connection and return completed text as a partial failure,
without learning from the recording. Silent segments clear audio and delete their text item;
recordings without audio create neither. Cancellation resolves pending item acknowledgements,
uses the same cleanup, and never replays an ambiguous audio commit. Reconnects declare full input
with fresh IDs. Cache misses and shorter common prefixes do not change the input sent; text-item
deletion makes no assumption about whether the server retains token-verified KV checkpoints.

Desktop progress combines validated preceding segments with the provisional current body using
the same language separator as final output. Delta text may differ from the completed result;
only agreement between `response.output_text.done` and completed `response.done`, followed by
the existing JSON/schema checks, authorizes final text. Existing final preparation applies local
corrections, then the UI reconciles its read-only draft before allowing edits or confirmation.
Progress never creates confirmation feedback, history or destination input. Requests still occur
only at a 30-second boundary or stop, so a short recording streams generation after stop rather
than during speech. Stable-prefix declarations and both item-deletion acknowledgements are unchanged.

Each recording
works on a copy of memory; cancellation/failure discards its updates. Successful desktop recordings
retain original term spellings and displayed ranges with a single-use feedback handle. Only successful preview delivery commits
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
Topic selection remains a model decision, not proof of accurate classification. The output uses
`context: String`, `memory_candidates: Vec<String>` and `text: String`, in that order. A topic label
can reuse a snapshot ID or name a new topic; the exact reserved string `"未知语境"` means unknown.
`DictationResult.context: Option<String>` maps that sentinel to `None`; malformed null, missing or
wrong-type context also degrades to `None`, but fails evaluation metadata checks. Unknown context
requires an empty candidate array and cannot apply or learn topic-scoped corrections, observations
or terms. Delivered text and global transcription history remain unaffected. Manual vocabulary
cannot use the sentinel as a topic: validation asks the user to rename it. Legacy sentinel-named
records remain readable and are excluded from snapshots before selecting four topics, without
rewriting stored records or changing generic label validation.

Candidates are complete proper-noun strings, at most 80 Unicode characters each, listed by source
occurrence with repeats and capped at eight entries. More than eight discards the candidate group;
malformed individual entries do not discard valid neighboring names. The result boundary groups
NFKC/lowercase-equivalent names, locates complete Unicode terms and processes longer names first.
Full names own their nested fragments even when their supplied count is invalid. A group is accepted
only when its supplied count equals all remaining eligible occurrences and none overlap themselves
or cross another name. The retained candidates preserve model spelling separately from UTF-8 source
ranges and are sorted by source position. The client cannot select one of two same-spelled mentions
from a single string: that whole name is skipped, rather than guessing an occurrence or globally
replacing the spelling. Custom prompts must use this new protocol; old context/candidate objects
degrade metadata without discarding valid text. The result boundary separately reports metadata
validity: fewer than eight entries must cover the supplied names' owned occurrences, while exactly
eight may be their grounded source-order prefix. Thus nine mentions with eight supplied strings
are protocol-valid, but that incomplete name group still cannot authorize correction or learning.
Original JSON remains available for exact raw-context assertions. These checks establish textual
support, not acoustic truth or whether a returned string is semantically a proper noun.

Each name search builds a Unicode word-boundary table for constant-time lookups in its matching
loop. Containment and crossing checks advance through source-ordered spans rather than restarting
from the first occurrence. With at most eight names of at most 80 characters, grounding takes
\(O(n)\) time and space in transcript length. Prefix validation sorts at most eight positions per
group (64 total), never all transcript occurrences.

The schema-2 memory file adds at most 512 persistent corrections with context, source spelling,
target spelling, recording provenance and last manual-edit time; v1 model observations retain their
original expiration. Confirmed proper-noun edits create whole-word rules only when alignment and
topic assignment are unambiguous. One displayed-to-final grapheme alignment projects every candidate;
removed or
unassignable candidates are dropped. New terms are stored as `ProperNoun`; existing stored phrases
and both kinds of manual vocabulary remain supported without a memory-schema change. Expensive
rewrites exceed a fixed alignment-cell budget and skip learning. Live text is always delivered
unchanged by feedback validation.

Snapshots expose correction pairs without provenance. A matching response topic enables local,
single-pass replacements before preview only within complete proper-noun candidates extracted by
that response. Missing or ambiguous candidates never authorize local text replacement.
Learning and application consume the same disjoint spans validated at the result boundary. Rules match the entire candidate,
so a rule for a shorter name cannot edit part of a longer extracted name. Configured vocabulary
protects its full occurrences in that topic, including against intersecting candidate replacements;
the same rule can still apply outside those occurrences. Rendering preserves this protection even
before any rules exist. Protected occurrences refresh short-term candidates but supply no fragment
rules or spelling-conflict evidence for standalone names. Ambiguous or cyclic rules are skipped.
Rendering retains original spellings and exact displayed ranges for all retained names.
Replacements use original source ranges in reverse order; candidate ranges translate directly with
the known byte changes. Prepared feedback
does not retain or rewrite model JSON. Confirmed terms become bounded memory observations directly.
Feedback compares displayed text with the confirmed revision; it never treats the
program's earlier replacement as a manual edit. All surviving unprotected occurrences participate in spelling
conflict detection across segments, and conflicting final spellings revoke the source mapping.
Each context/source key accumulates one Keep/Set/Remove decision. Equal spellings preserve manual
evidence; a conflict makes removal terminal. Memory applies each resulting decision once.
Deleting a mention supplies no spelling evidence and preserves its rule. Feedback uses only the
displayed coordinate system. Returning the original spelling revokes its rule without learning an
inverse mapping only when that edit satisfies the same candidate ownership checks as any other
spelling change. There is no model-to-final restoration path to borrow neighboring prose.
Changing to a third spelling updates the original source mapping.
The initial ownership policy accepts replacements and deletions contained in a displayed candidate,
and pure insertions strictly inside it. Insertions at either candidate edge cannot rename it;
prefix/suffix extensions and multiword restorations outside the displayed candidate are deferred.
Repeated graphemes can move an insertion through unchanged text without changing the output.
If any such equivalent position reaches a candidate edge, learning is skipped even when another
segment's edit made the global alignment place it inside. Shared segment edges use the same check.
An edit crossing a candidate or segment boundary skips the affected learning. Segment projection
still accepts unshared edge insertions for confirmed context text. Before interpreting a name's
grapheme edits, a retained complete name inside its
projected range anchors the unchanged spelling. Added surrounding text must be separated from
that occurrence; Unicode word boundaries between Han characters alone do not make an appended
character an unchanged name. Without an intact anchor, local edits that introduce new whitespace have ambiguous
scope and skip learning; an explicit whole-candidate rewrite can still name multiple words.
Segment-edge ownership uses the original sorted spans, including neighbors that cannot be projected
after deletion. A shared segment-edge insertion therefore skips both segments. Name-edge insertions
cannot supply a spelling change; an intact name may still refresh memory. Newly added sentence endings are
excluded from learned names; existing candidate
punctuation and name symbols remain. Unedited or punctuation-only confirmations do not refresh
manual evidence. Refreshed candidates are deduplicated and bounded to eight per segment. Disabling memory
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
