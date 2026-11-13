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
  saved component/full-text source validation, pure prompt composition and user-owned long-term terms. Debug output omits content.
- `engine`: one persistent async worker and reusable connection, recording-scoped I/O and cancellation,
  bounded backlog, ordered pause-aware response segments capped at 30 seconds, final text concatenation without deduplication.
- `audio::PcmResampler`: shared stateful conversion with explicit output rate, filter-delay and final-tail compensation; Realtime output remains 24 kHz.
- `audio::segment`: shared source-cursor cuts; desktop uses pause detection, offline/evaluation explicitly retain fixed boundaries.
- `transport`: standard session/response events, explicit audio-item references and context input,
  out-of-band responses, correlation, final-event consistency, completion status and item deletion.
- `text_stream`: provisional top-level JSON `text` decoding across escaped strings and surrogate pairs;
  nested metadata is skipped and complete-result validation remains authoritative.
- `progress`: one latest-body snapshot and coalesced wake-up per recording; closing rejects late
  updates, while cancellation also interrupts a finalizer already detached from the orchestrator.
- `result`: validated leading JSON object with trailing output ignored, and independently validated optional context/term metadata.
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

[`config.example.json`](../../config.example.json) is the single source of new configuration and
preparation defaults, embedded at build time for portable macOS/Windows packaging. It directly
stores HTTP instructions, the complete LLM system text and Realtime `dictation.prompt_components`.
Preparation copies those prompt fields, sets Realtime `dictation.prompt` to null, removes retired
`dictation.short_prompt` and fills permitted missing fields from the example. It does not save
normal/short full texts or read a separate default resource. Persistent prompt customization is
paused. Desktop and ordinary conversion strictly prepare, atomically save only if changed, and
reload configuration before constructing recognition services. All explicit non-prompt settings
remain intact; parse/type or write failures cannot supply an unsaved replacement to recognition.
Read-only load and `config path/get/list/check` expose saved values without preparation.

During recognition, `PromptComponents::compose` joins saved components in the fixed order task,
language, numbers, cleanup, output_format, context, memory, examples. Each string is trimmed at its
edges; empty components are skipped; remaining components are separated by a newline. Content is
literal, with no template substitutions or bundled fallback. All seven rule keys are required,
`examples` is optional, and unknown keys or non-string values are rejected. `compose_for_segment`
only omits the separate `examples` component for short audio: it never replaces or adds content,
including an empty-result example. Missing or empty independent examples produce identical normal
and short instructions; example-like text in other components is left alone.
Both final combinations retain the 4096-Unicode-character limit, including separators.

The bundled default distributes the rules across seven components and separates the examples.
`task` protects short words, unfinished fragments and meaningful repetition; `cleanup` restricts
self-correction to explicitly withdrawn values while preserving independent negation, operation
objects and later clauses. `output_format` requests the complete text first, then context and
memory_candidates in one JSON object; short topic labels and candidate limits must not shorten
the body. Context selection and memory extraction remain in their respective fields, with
the first eight proper-noun occurrences requested, including repeated mentions. `memory` names
the eligible name categories, excludes generic categories and job titles, and requires candidates
to occur in the completed text. The normal prompt retains seven representative examples:
self-correction, independent negation, separate quantities, model suffixes, money units, quoted
instructions and object replacement. The model example includes its complete name as a candidate;
the other six have empty arrays. HTTP STT and complete LLM cleanup prompts are unchanged. The
short branch omits all independent examples and appends no replacement, including an empty-result
example. These instructions request the contract; runtime metadata checks still validate it.

`prompt()` and `short_prompt()` compose only saved components; a connection can precompute both
combinations before selecting by segment. Each Realtime segment strictly shorter than two seconds
selects the short combination; exactly two seconds and longer use normal instructions. Duration
includes captured pauses and excludes protocol padding: at 24 kHz, 47,999 samples are short and
48,000 are normal. No default content is loaded during selection.
Full text and components are mutually exclusive instruction sources. Both present is a conflict;
both absent requires configuration preparation. Full text alone, including an explicit empty string,
is sent verbatim for every duration. The old `short_prompt` key is parsed as `legacy_short_prompt`
only for retirement, never serialized or used as instructions; a non-null legacy value requires
preparation before direct recognition. Preparation replaces old full strings, empty strings, nulls
and component overrides with the example's saved components.
Session instructions are empty; each response carries its complete selected instructions, so a
short segment cannot inherit normal examples from session setup. Ordinary prompt-lab reports record
both actual combinations and their selection policy. The report's `stt.short_prompt` remains a
separate interface from the retired configuration field. Production confirmation stays in private memory.
Lab first saves and prepares an isolated configuration, then writes and reloads any literal
prompt-file/no-prompt candidate before creating services. Candidates clear components and the retired
short field so all durations use the same candidate. Neither the
main configuration nor source memory is written.

The default context component explicitly ties the natural-language history after audio to the same preceding audio.
It explains that matching topic names combine the configured vocabulary and runtime reference.
That saved guidance appears in both combinations. Lab candidates must supply any equivalent
guidance themselves. The full LLM system text has different semantics from old cleanup fragments;
downgrading requires the matching old configuration.

## Audio and lifecycle

The existing recorder emits 200 ms WAV envelopes internally in Realtime mode. This avoids adding
a second recorder/callback protocol; shared `audio::decode_mono` uses the existing validated WAV
reader, and the network worker maintains one resampler per recording. The small headers never reach the service. The microphone callback only
collects/downmixes input and notifies readiness; network and inference stay off the UI thread.

Desktop `Recording` applies the shared pause-aware segmenter to 24 kHz PCM before appending
each contiguous slice. After speech evidence, 800 ms of consecutive analyzed quiet can close a
segment; 30 seconds (or the canonical byte capacity) always closes it first, and stop flushes
the nonempty tail. No overlap, trimming or text deduplication is introduced. Acoustic VAD does
not determine sentence meaning or tone. Endpoint failures degrade to fixed cuts for that recording.
`turn_detection` remains null: the client owns all commits, prefix declarations and cleanup.
Raw capture, offline conversion and explicit scenario diagnostics retain fixed boundaries;
ordinary prompt-lab evaluation uses the desktop pause-aware path.

Recorder backlog and the 300-frame worker channel are bounded separately. Overflow is an explicit
failure and stops recording on the next readiness event. Fault-triggered cancellation retains
already completed, validated segment text as a partial failure; explicit user cancellation
suppresses it. Both discard the recording's memory updates. The existing fault reason distinguishes
these cases without introducing another cancellation state. During response generation, subsequent
frames queue locally. No unbounded server or client queue is assumed. Audio up to each segment's
end is uploaded before commit; stop flushes the resampler and retains short voiced tails. Complete
segments pass the energy gate, reuse settled streaming/tail evidence, and otherwise fall back to
whole-segment VAD; silent segments clear the server buffer
without generating text. Already-uploaded frames are not retrospectively normalized or removed.

The service must accept `max_output_tokens=4096`, text item creation/acknowledgement, text delta/done
events, cancellation and item deletion. Before the first audio append of every segment, the client
creates a user text item containing configured vocabulary. It validates the returned content and
server-assigned ID, waiting for matching `conversation.item.added` / `conversation.item.done`
(or a completed `conversation.item.created`). Session configuration is acknowledged at setup;
task instructions are supplied separately with every response.
The vocabulary bytes remain frozen for the recording, including its later segments.

After commit, `response.create` uses explicit `input` in this order: accepted vocabulary item
reference, committed audio item reference, inline dynamic-context text. It retains
`conversation=none`, correlation `metadata`, output settings and exactly the declared instructions.
The prefix and suffix use natural-language topic and word lists. The suffix includes candidate
spellings, numbered recent confirmed sentences and persistent confirmed word corrections;
matching topic names combine both text parts, and configured vocabulary is sent only once.
Candidate support counts still control retrieval internally but are not shown to the model, nor
are `kind` or JSON envelopes. Empty references explicitly say that no vocabulary/history is available.
No timestamps or random IDs enter text. Storage and business snapshots remain structured JSON.
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
at a 30-second boundary or stop, and now also at suitable sustained pauses while recording continues.
During responses, later input waits in the existing bounded queue; an early acoustic cut does not
promise immediate submission under backlog. Stable-prefix declarations and both item-deletion acknowledgements are unchanged.

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
Topic selection remains a model decision, not proof of accurate classification. The bundled prompt
requests `text: String`, `context: String` and `memory_candidates: Vec<String>`, in that order;
the result parser validates the fields independently of their JSON order. A topic label
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
replacing the spelling. Lab candidates must use this protocol; old context/candidate objects
degrade metadata without discarding valid text. The result boundary separately reports metadata
validity: fewer than eight entries must cover the supplied names' owned occurrences, while exactly
eight may be their grounded source-order prefix. Thus nine mentions with eight supplied strings
are protocol-valid, but that incomplete name group still cannot authorize correction or learning.
The first complete JSON object remains available for exact raw-context assertions. Realtime final
results and HTTP cleanup ignore any output after that object, including a second object; only the
first object's text and metadata participate in delivery and learning. A complete outer Markdown
fence is accepted when its opening line is three backticks, optionally followed by lowercase
`json`, and its closing line is three backticks. LF and CRLF line endings are supported.
Leading prose, other fence languages, missing closing
fences, malformed or incomplete objects, unknown fields and invalid required text still fail
validation. Realtime's 64 KiB input limit includes the fence and ignored suffix, and its 16 KiB
text limit is unchanged. Raw model output remains in diagnostic logs; accepting a fence does not
make the original response strict JSON or establish that its context or candidates are correct.
These checks establish textual support, not acoustic truth or whether a returned string is
semantically a proper noun.

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
Pure deletions contained in a candidate also skip learning when an equivalent position crosses
either candidate edge. Scanning stops at a mismatch or another edit; repetitions wholly inside
the name still allow real spelling corrections such as `Codexx → Codex`. This prevents deleting
neighboring repeated prose from training `张山 → 山` or `New York → York` without changing stored rules.
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
does not imply that a Realtime temperature parameter is sent. Prompt metadata includes the exact
composed instructions and any conditional short branch. Ordinary evaluation initializes private
memory from read-only source state and confirms actual finals; explicit scenario diagnostics keep
their frozen isolation. Literal candidates are saved and reloaded in isolated configuration before
service construction, replace the whole audio prompt and clear components and the retired short
field, including for an empty prompt.
HTTP and Realtime use the same evaluation, metrics,
report writing and review flow.
Evaluation and report loading accept sanitized HTTP(S) and WS(S) endpoint metadata; userinfo,
query strings and fragments remain forbidden. Live delivery, conversion, setup verification and
prompt-lab use the shared `text::merge_texts` helper with `transcription.language`: Chinese
language codes add no separator, while other or unspecified languages add a space. Repeated
speech is preserved. Realtime metadata records the language used for joining; it is not sent
as a recognition-language constraint to the server.
