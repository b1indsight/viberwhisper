# Audio Module Architecture

## Purpose

The audio module produces complete, independently decodable WAV chunks in memory. Microphone
capture and local WAV files are two producers of the same `WavChunk` payload; neither producer
writes intermediate chunk files.

## Module Layout

```text
src/audio/
  chunk.rs     — WavChunk, ChunkError, WAV encoding, and shared capacity calculation
  recorder.rs  — cpal microphone capture and live WavChunk production
  wav_file.rs  — streaming local-WAV reader and fallible chunk iterator
```

## `WavChunk`

`WavChunk` owns an `Arc<[u8]>` containing one complete WAV payload. It deliberately contains no
path, session id, index, retry policy, or transcription state. Clones share the immutable bytes,
which lets retries create fresh multipart readers without copying the payload.

## Chunk Capacity

Production uses one module-owned policy for both live recording and offline conversion: a chunk is
limited to 30 seconds or 23 MiB, whichever produces fewer complete frames. These safety values are
not persisted configuration.

`max_frames_per_chunk(max_duration_secs, max_size_bytes, output_spec)` is the only duration/size
conversion. It works in complete audio frames and accounts for channel count, sample width, and
the 44-byte or 68-byte header that `hound` emits for the output spec. Its explicit parameters keep
the capacity calculation independently testable; production callers pass the module policy.

- `Ok(None)`: both limits are disabled; the producer does not proactively slice.
- `Ok(Some(0))`: a nonzero size limit can hold no more than 0.5 seconds; the producer emits no
  chunk and therefore makes no STT request.
- `Ok(Some(frames))`: producers slice at the smaller valid duration/size capacity.

## Live Recorder

The cpal callback downmixes input to mono `i16`, applies microphone gain, appends PCM to the shared
buffer, and updates the number of complete chunks. It never performs WAV encoding, channel waits,
disk I/O, or network I/O.

The main loop polls `take_ready_chunk()`. For each ready boundary the recorder copies one complete
PCM slice, encodes it with `hound` into a `Cursor<Vec<u8>>`, drains the encoded samples only after
successful encoding, and returns `ReadyChunk { session_id, chunk }`. On stop, complete remaining
slices and the final tail are encoded the same way. `RecorderStopOutcome::Stopped` therefore owns
`Vec<WavChunk>` rather than file paths.

## Local WAV Reader

`WavChunkReader::open(path, duration_limit, size_limit)` opens the source once. Its `chunks()` method
returns an `Iterator<Item = Result<WavChunk, ChunkError>>` that reads, encodes, and yields one chunk
at a time. Integer and float sample formats, channel count, sample rate, and bit depth are preserved.

The iterator has no chunk-count cap. A decode error is yielded once and is terminal; subsequent
`next()` calls return `None`. With both limits disabled, a non-empty file is returned as one chunk.

## Dependencies

| Crate | Usage |
|---|---|
| `cpal` | Cross-platform microphone capture |
| `hound` | WAV decoding and in-memory encoding |
| `tracing` | Structured recorder diagnostics |
