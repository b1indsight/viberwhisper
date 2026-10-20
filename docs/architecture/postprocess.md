# Post-processing Module Architecture

## Purpose

Optional LLM cleanup after HTTP STT: punctuation, explicit self-correction and conservative
filler removal. Desktop cleanup also supplies validated context and term candidates to the
same confirmation-driven correction and memory implementation used by Realtime dictation.

## Module Layout

```
src/
  postprocess.rs      — processor facade, typed config and errors
  postprocess/
    llm.rs            — structured chat-completions requests and private cleanup sessions
  dictation/
    learning.rs       — HTTP cleanup's serial memory owner and confirmation wait
    memory.rs         — shared bounded snapshots and atomic schema-2 persistence
    correction.rs     — shared whole-candidate correction and final-edit alignment
    result.rs         — shared JSON and candidate-quote validation
```

## `PostProcessor`

```rust
pub enum PostProcessor {
    Disabled,
    Llm(Box<LlmPostProcessor>),
}

impl PostProcessor {
    pub fn new(config: PostProcessConfig) -> Self;
    pub fn process_text(&self, text: &str) -> Result<String, PostProcessError>;
    // Crate-internal desktop entry points:
    pub(crate) fn new_desktop(
        config: PostProcessConfig,
        memory: MemoryConfig,
        path: Option<PathBuf>,
        language: Option<String>,
    ) -> anyhow::Result<Self>;
    pub(crate) fn process_for_delivery(&self, text: &str) -> Result<Transcript, PostProcessError>;
    pub(crate) fn shutdown(&self);
}
```

`new`/`process_text` serve CLI conversion and setup with an empty snapshot and no personal-memory
I/O. `new_desktop` adds a memory worker only when both LLM cleanup and memory are enabled.
`process_for_delivery` returns text plus an optional single-use confirmation capability.
The listener preserves this capability through finalization and submits the user's final text
only after successful destination delivery. Disabled cleanup passes text through without a worker.

LLM client initialization failure selects `Disabled`, as before. Memory load failures use
ephemeral state without overwriting damaged or unknown-version storage. Worker initialization
failures propagate as startup errors. No network or memory file I/O runs on the UI thread.

## Requests, prompts and validation

All calls use blocking OpenAI-compatible chat completions with `stream: false` and a 60-second
request timeout. URL/model/auth still come from `PostProcessConfig::from_config`; disabled cleanup
does not resolve unused LLM settings or credentials. Bearer auth is omitted for `ApiAuth::None`.

`PromptComponents::postprocess_prompt` reuses default language, numbers and cleanup rules from
`assets/dictation-prompt.default.json`, including self-correction and independent-negation examples.
A text-specific task explicitly treats the STT text as data, cannot re-listen to audio, and forbids
answering instructions or completing unfinished speech. Realtime prompt composition is unchanged.

`post_process.prompt` replaces cleanup rules, not the fixed text task, structured-output contract,
context selection or candidate grounding instructions. Existing custom prompts that demand plain
text must be updated. No provider-specific `response_format` extension is required.

The system message carries these instructions; the user message contains a serialized JSON object:

```json
{"text":"STT text to clean","contexts":[]}
```

Desktop memory provides up to four contexts within the existing 6000-character snapshot budget.
The provider returns `memory_candidates`, `context`, and `text` in the same call; no second
metadata-extraction inference is added. `DictationResult::parse_transcript` shares complete JSON,
context-ID and individual candidate-quote validation with Realtime. HTTP cleanup processes a whole
merged transcript without Realtime's per-segment 16 KiB text / 64 KiB JSON budgets; the Realtime
parser retains both limits. Invalid optional metadata is discarded independently; invalid JSON,
empty text or request failures return an error and the listener/CLI keeps the original STT.
JSON is never injected as fallback text.

## Confirmation and memory ownership

`LearningWorker` owns the shared schema-2 `dictation-memory.json` on one background thread.
It supplies snapshots, prepares reversible previews using `Memory::prepare`, then waits for
`Confirmation` before processing the next command. `Memory::confirm` compares source metadata,
shown candidate spans and the final edited text. It does not infer evidence from raw STT diffs.
Snapshots for later recordings cannot overtake accepted feedback.

Realtime and HTTP recognition are mutually exclusive within the listener. The selected path owns
the only writer, so switching transports after restart reuses existing corrections without a schema
change or migration. HTTP memory validates vocabulary independently of unused Realtime URL settings.
`dictation.memory.enabled=false` disables loading, personal context upload, learning and local repair.
CLI/setup and raw capture do not create this worker.

Dropping a preview discards feedback. Failed injection can retry without submitting learning;
only its last successful revision is confirmed. On shutdown the listener cancels previews and
closes the worker's pending feedback channel, drains any accepted value and joins the memory
thread without waiting for user input or an in-flight HTTP call. A late request cannot learn
after the worker stops.

Personal snapshots include bounded history, vocabulary and corrections and go to the configured
cleanup provider, potentially a cloud service. No transcript or snapshot is logged. API error
bodies and structured-result schema error details are not retained/logged because they may echo
this input; safe status and error categories remain available.

## Private session modes

`LlmSession` dispatches to conservative or preheat state using the same immutable `LlmRequest`
and snapshot. Conservative mode accumulates text without adding separators and requests once at
finish. Preheat mode runs accumulated requests in background threads, keeps only the latest
generation through a mutex/condition variable, and retries a failed final generation once.
Discarded generations never reach preview preparation or memory.

The desktop currently supplies one complete transcription after STT convergence. Both modes
therefore begin cleanup at that point; the preheat setting does not yet provide recording-time
latency hiding. `process_text` remains a one-shot call. Empty input bypasses requests and learning.

## Error Handling

`PostProcessError` distinguishes transport, malformed response JSON, invalid structured results,
memory-worker failures, API status, missing content and empty content. Persistence failures are
logged on the memory worker after delivery and never replay already delivered text.

## Dependencies

| Crate | Usage |
|---|---|
| `reqwest` | Blocking HTTP client for LLM API calls |
| `serde_json` | JSON request/response serialization |
| `tracing` | Structured logging |
