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
`text::merge_texts` helper.

## `ApiTranscriber`

`ApiTranscriber` is compatible with OpenAI-style multipart transcription endpoints. Construction
consumes endpoint, authentication, model, language, prompt, temperature, and retry configuration.
Chunk duration and size limits are producer configuration and are not stored in the transcriber.

Each request builds a multipart form containing `model`, `temperature`,
`response_format=verbose_json`, optional `language` and `prompt`, plus an `audio.wav` file part. The
part reads from a new `Cursor<Arc<[u8]>>`; retries share the original bytes and only recreate the
reader and request body.

## Retry Budget

- 4xx responses fail immediately.
- 5xx and network errors retry up to the configured maximum.
- The maximum is three retries, for at most four attempts.
- Each request has a five-second timeout; 1s, 2s, and 4s backoffs keep the worst-case window near
  27 seconds and below 30 seconds.

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
