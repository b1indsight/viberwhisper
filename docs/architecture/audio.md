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
  preprocess.rs — shared upload preparation and validated WAV decoding
  vad.rs       — shared Silero inference, whole-chunk gate and recording-scoped streaming state
  segment.rs   — sample-exact pause/limit/stop boundaries for desktop recognition
  loudness.rs  — bounded constant-gain adjustment with peak protection
  wav_file.rs  — streaming local-WAV reader and fallible chunk iterator
```

## `WavChunk`

`WavChunk` owns an `Arc<[u8]>` containing one complete WAV payload. It deliberately contains no
path, session id, index, retry policy, or transcription state. Clones share the immutable bytes,
which lets retries create fresh multipart readers without copying the payload.
Live and offline WAV encoders reserve output capacity from the encoded header and sample count,
using checked arithmetic before allocating the byte buffer.

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

## Speech Detection and Upload Loudness

`ApiTranscriber` calls `prepare_for_transcription` once before its retry loop. The
processing order is original energy gate, original-audio VAD, then loudness
adjustment. A rejected chunk returns the existing successful empty transcription;
an accepted chunk retains every frame, channel and pause. Recording callbacks,
source WAVs and prompt-lab archives remain unchanged. This gate does not choose boundaries;
desktop endpointing below closes each segment before upload preprocessing.

Silero VAD v6.2 runs locally using CPU-only ONNX Runtime 1.20.0, statically linked
through pinned ort/ort-sys rc.9. The model is embedded in the binary. A process-wide
mutex serializes inference; recurrent state and context reset for every channel
and chunk in the whole-chunk gate. Streaming analysis owns separate per-recording state;
interleaved batch calls cannot reset it. Channels are analyzed separately to avoid phase cancellation. Rubato
performs anti-aliased conversion to 16 kHz for analysis only, compensating delay
and flushing its tail. Input rates outside 8–192 kHz bypass VAD with a diagnostic.

Any 512-sample frame scoring at least 0.3 accepts the entire chunk. A trailing frame
is zero-padded only for inference; inputs shorter than one frame pass conservatively.
Only a completely successful no-speech analysis suppresses an audible chunk. Model
or resampling failures bypass VAD. Decode/non-finite-data and gain-encoding errors
preserve original upload bytes. There is no first-launch model/runtime download.

Loudness uses RMS of energy-active 50 ms windows, excluding long silent pauses.
Target RMS is -20 dBFS, boost is capped at +12 dB, and sample peaks at -1 dBFS.
One gain applies to all samples/channels. Already-loud audio is attenuated only for
peak protection; unchanged audio reuses its original bytes. Changed WAVs preserve
sample rate, format, bit depth and frame count and remain within the upload limit.
Every network retry receives the same prepared bytes.

This does not repair capture clipping or remove noise. Background conversation
may pass VAD; quiet speech below the original -50 dBFS gate is still skipped. The
fixed VAD threshold favors speech retention, but can still reject speech. Verify
on representative recordings; loudness and model smoke tests do not establish an
improvement in word/character error rates. See `assets/vad/README.md` for model
provenance, checksum, runtime compatibility and distributed licenses.

## Chunk Capacity

Production uses one module-owned policy for both live recording and offline conversion: a chunk is
limited to 30 seconds or 23 MiB, whichever produces fewer complete frames. These safety values are
not persisted configuration.

`max_frames_per_chunk(output_spec)` is the only duration/size
conversion. It works in complete audio frames and accounts for channel count, sample width, and
the 44-byte or 68-byte header that `hound` emits for the output spec. It owns the fixed policy;
configuration and application callers do not carry limit fields or arguments.

- `Ok(0)`: the size limit can hold no more than 0.5 seconds; the producer emits no
  chunk and therefore makes no STT request.
- `Ok(frames)` with a positive count: producers slice at the smaller duration/size capacity.

Recording and offline reader capacities are plain integers. There is no unlimited mode;
short recordings and the final tail of longer files use their remaining frames.

## Desktop Pause Boundaries

Both desktop recognition workers use `Segmenter` in `PauseAware` mode. After speech evidence
(probability ≥0.3), 25 consecutive low-probability 32 ms frames close a segment (800 ms of
analyzed quiet). Renewed speech resets the quiet counter. Pure silence cannot repeatedly trigger
pause cuts. These are acoustic estimates, not sentence or tone classification; a long thinking
pause can still split a sentence. The threshold remains an internal initial value, not a setting.

`StreamingVad` analyzes an i16/32768-normalized 16 kHz copy using the existing model. A recording
owns its resampler, partial frame, context and recurrent state across packets and cuts. The shared
`PcmResampler` takes explicit input/output rates; Realtime output remains 24 kHz. Only model
inference holds the shared lock, never resampling, source-buffer assembly or network waits.

Source PCM advances in approximately 20 ms steps, with a partial step retained across packets.
Cuts refer to the current source cursor; consumers append each contiguous slice exactly once.
Nothing is trimmed, overlapped or retrospectively removed, and original silent pauses stay in
their segment. The hard capacity takes precedence even inside an analysis step. Stop immediately
closes a nonempty tail once, without waiting for quiet or fabricating analysis silence. Analysis
failure logs a warning and disables pause cuts for that recording; hard cuts and the tail remain.

HTTP analyzes native mono PCM; Realtime analyzes its existing 24 kHz output before append/commit.
Both use the canonical capacity calculation above. Offline conversion, prompt-lab evaluation
and raw dataset capture remain fixed-boundary paths. The English fixture verifies integration,
not Chinese recognition quality or real microphone endpoint accuracy.

## Live Recorder

`AudioConfig` carries an optional input-device display name. `None` resolves through cpal's system
default input device. A configured value enumerates input devices and selects the first exact name
match each time recording starts. Devices whose individual display names cannot be read are logged
and skipped; a missing configured name is still an operational error rather than a silent fallback.
The shared name reader accepts concrete cpal devices and yields device/name pairs lazily.
Setup collects all readable names, while configured-device lookup stops at its first exact match.
The setup wizard uses the recorder module's device enumeration and the same resolver for its
verification recording. Its hotkey-controlled verification loop drains ready chunks while the
session remains active, matching the normal listener's bounded PCM-buffer behavior.

`AudioRecorder` keeps persistent configuration separately from `Option<ActiveRecording>`, which
owns the session ID, stream, and chunk progress. Each session gets its own shared `RecordingBuffer`,
which fixes its sample rate and chunk capacity at construction and owns the PCM samples, callback
recording switch, and sample/readiness counters. `push_mono` receives only PCM samples; live and
stop-time chunk encoding read the same buffer-owned rate and capacity. Failed starts never publish
an active session; stop and cancel consume the matching session without resetting reusable fields.

The I16 and F32 cpal callbacks share a pipeline that downmixes input to mono `i16`, applies microphone
gain with format-specific clipping, and reuses a callback-owned scratch vector outside the PCM lock.
It appends PCM to the shared buffer and updates the number of complete chunks.
Crossing a new complete-chunk boundary also
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
from an old `SessionId` are ignored after stop or session replacement. On stop, the existing
200 ms wait remains before stream destruction. The remaining PCM vector is then moved out of
the mutex without copying, and complete slices plus the final tail are encoded outside the lock.
`RecorderStopOutcome::Stopped` therefore owns `Vec<WavChunk>` rather than file paths.

Desktop HTTP and Realtime enable the existing 200 ms frame mode and bounded 60-second PCM backlog.
Stop-time residuals use the same small frame capacity; a recorder frame is not a recognition
segment. Overflow is sticky, stops further acceptance and is checked before each UI drain attempt,
including when no complete frame is available. Fixed raw capture retains complete WAV chunks.
WAV envelopes are produced outside the callback; decoding, endpoint inference and STT run on the
recognition worker. See [core](core.md#http-worker-concepts) for HTTP queue/EOF/failure semantics.

Prompt-lab recording optionally clones these immutable chunks into a session-owned, capacity-two
archive channel after they leave the recorder. Its worker validates one stable integer 16-bit WAV
format, decodes the chunks in arrival order, and writes their PCM samples into one final dataset
WAV. Stop-time chunks use the same path. The callback remains free of disk work, and normal listener
mode does not construct the archive channel or worker. Closing a completed session finalizes the WAV
before its digest and JSON sidecar are written; an interrupted process may leave a partial or
unreferenced WAV for dataset validation to report.

## Local WAV Reader

`WavChunkReader::open(path)` opens the source once. The reader itself
implements `Iterator<Item = Result<WavChunk, ChunkError>>`, reading, encoding, and yielding one chunk
at a time. Integer and float sample formats, channel count, sample rate, and bit depth are preserved.

The iterator has no chunk-count cap. A decode error is yielded once and is terminal; subsequent
`next()` calls return `None`. A 65-second file that fits the byte limit per chunk yields
30-, 30-, and 5-second WAV chunks from the same retained source reader.

## Dependencies

| Crate | Usage |
|---|---|
| `cpal` | Cross-platform microphone capture |
| `hound` | WAV decoding, in-memory encoding, and normalized signal classification |
| `tracing` | Structured recorder diagnostics |
