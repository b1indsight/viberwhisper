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
  signal.rs    — compact normalized RMS-window check for effectively silent chunks
  wav_file.rs  — streaming local-WAV reader and fallible chunk iterator
```

## `WavChunk`

`WavChunk` owns an `Arc<[u8]>` containing one complete WAV payload. It deliberately contains no
path, session id, index, retry policy, or transcription state. Clones share the immutable bytes,
which lets retries create fresh multipart readers without copying the payload.

## Audible-Window Classification

`contains_audible_window` decodes one complete `WavChunk` and applies the same target-neutral
policy to live and offline audio. Integer samples are normalized by their declared bit depth;
floating-point samples are interpreted at full scale. Energy is measured across complete channel
frames so duration does not change with channel count.

The classifier uses 50 ms RMS windows and considers a chunk audible as soon as any complete or
trailing window exceeds -50 dBFS. The wider window smooths brief fluctuations, while the
any-window rule keeps short utterances on the normal STT path. The classifier exits at the first
audible window. These values are fixed module policy rather than persisted configuration.

Malformed WAV decoding returns `hound::Error`; invalid specifications, non-finite samples, and
unrepresentable window sizes are treated as audible. The transcriber also fails open on the decode
error, so uncertain local analysis always preserves the previous upload/error behavior.

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
buffer, and updates the number of complete chunks. Crossing a new complete-chunk boundary also
sends one readiness callback containing the active `SessionId`. The existing ready-chunk count
prevents repeated notifications within the same boundary, while the fixed chunk policy keeps the
wakeup rate independent of audio sample-buffer traffic. The callback never performs WAV encoding,
channel waits, disk I/O, network I/O, or session transitions.

The readiness callback becomes an application event that wakes winit. The event-loop handler drains
`take_ready_chunk()` until it returns `None`. For each ready boundary the recorder copies one
complete PCM slice, encodes it with `hound` into a `Cursor<Vec<u8>>`, drains the encoded samples only
after successful encoding, and returns `ReadyChunk { session_id, chunk }`. The in-memory fixed-format
encoder's error path is handled inside the recorder: it logs the failure, returns `None`, and leaves
the PCM buffered for stop-time recovery.

Each boundary notification is independent, so a boundary published while the listener is draining
queues its own wakeup without a pending flag, re-arm protocol, or timer retry. Extra notifications
are harmless because the listener drains until `take_ready_chunk()` returns `None`, and events
from an old `SessionId` are ignored after stop or session replacement. On stop, complete remaining
slices and the final tail are encoded the same way.
`RecorderStopOutcome::Stopped` therefore owns `Vec<WavChunk>` rather than file paths.

Prompt-lab recording optionally clones these immutable chunks into a session-owned, capacity-two
archive channel after they leave the recorder. Its worker validates one stable integer 16-bit WAV
format, decodes the chunks in arrival order, and writes their PCM samples into one final dataset
WAV. Stop-time chunks use the same path. The callback remains free of disk work, and normal listener
mode does not construct the archive channel or worker. Closing a completed session finalizes the WAV
before its digest and JSON sidecar are written; an interrupted process may leave a partial or
unreferenced WAV for dataset validation to report.

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
| `hound` | WAV decoding, in-memory encoding, and normalized signal classification |
| `tracing` | Structured recorder diagnostics |
