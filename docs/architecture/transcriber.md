# Transcriber Module Architecture

## Purpose

The transcriber consumes exactly one in-memory WAV chunk and returns one transcription result. It
does not open local files, split audio, assign chunk indexes, or merge results.

## Interface

```rust
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, chunk: &WavChunk) -> Result<String, TranscribeError>;
}
```

`WavChunk` is produced by either `AudioRecorder` or `WavChunkReader`. Realtime ordered merging is
owned by `SessionOrchestrator`; offline conversion iterates `WavChunkReader` and uses the shared
`text::merge_texts` helper. Prompt-lab regression follows the same offline chunk reader and merge
path but deliberately bypasses post-processing.

## `ApiTranscriber`

`ApiTranscriber` is compatible with OpenAI-style multipart transcription endpoints. Construction
consumes endpoint, authentication, model, language, prompt, and temperature. Retry policy is owned
by the module; chunk duration and size limits are owned by the audio producer.

`TranscriberConfig::with_prompt` consumes one already resolved config and replaces only its prompt
in memory. Prompt-lab uses this for configured baselines, prompt files, and explicit no-prompt runs;
the persisted config document is never mutated. `metadata()` returns endpoint/model/language/prompt/
temperature for dataset and run JSON while removing endpoint userinfo, query, and fragment data and
never exposing `ApiAuth`.

Each request builds a multipart form containing `model`, `temperature`,
`response_format=verbose_json`, optional `language` and `prompt`, plus an `audio.wav` file part. The
part reads from a new `Cursor<Arc<[u8]>>`; retries share the original bytes and only recreate the
reader and request body.

## Retry Budget

- 4xx responses fail immediately.
- 5xx and network errors retry once, for at most two attempts.
- Each request has a 12-second timeout; the 1-second retry backoff keeps the worst-case window near
  25 seconds and below the 30-second session convergence budget.

`TranscribeError` preserves API status/body, network and response parsing failures, and the
orchestrator-owned timeout variant.

## Test Double

`MockTranscriber` implements the same `&WavChunk` interface and returns fixed text without network
access. It is compiled only for tests.

## Dependencies

| Crate | Usage |
|---|---|
| `reqwest` | Blocking HTTP client and streaming multipart body |
| `serde_json` | Transcription response parsing |
