# Audio Module Architecture

## Purpose

Captures microphone input and saves it as a WAV file for transcription. Handles device enumeration, stream management, sample format conversion, and cleanup of old recordings.

## Key Struct

### `AudioRecorder` (`src/audio/recorder.rs`)

```rust
pub struct AudioRecorder {
    recording: Arc<Mutex<bool>>,
    buffer: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    sample_count: Arc<AtomicUsize>,
    gain: f32,
    sample_rate: u32,
}
```

| Field | Description |
|---|---|
| `recording` | Shared flag controlling the capture callback |
| `buffer` | Accumulates mono i16 PCM samples across callbacks |
| `stream` | The active `cpal` input stream (held to keep it alive) |
| `sample_count` | Atomic counter for progress logging |
| `gain` | Microphone amplification multiplier |
| `sample_rate` | Detected at stream-open time (typically 44100 Hz) |

## Key Methods

### `new(gain: f32) -> Result<Self>`

Creates the recorder. Queries `cpal::default_host()` for the default input device and logs available devices at `DEBUG` level. Does **not** open a stream yet.

### `start_recording(&mut self) -> Result<()>`

1. Returns early (no-op) if already recording.
2. Opens a `cpal` input stream using the device's default config.
3. Supports `I16` and `F32` sample formats; converts multi-channel to mono by averaging channels, then applies gain.
4. Sets `recording = true` before playing the stream to avoid dropping initial frames.
5. Clears the buffer and resets `sample_count`.

### `stop_recording(&mut self) -> Result<String>`

1. Sets `recording = false`, then sleeps 200 ms to let in-flight callbacks drain.
2. Drops the stream.
3. Writes the buffer to `./tmp/recording_<unix_timestamp>.wav` using `hound` with spec: 1 channel, native sample rate, 16-bit signed PCM.
4. Calls `cleanup_old_recordings("./tmp", 10)` to keep at most 10 WAV files.
5. Returns the file path string.

### `is_recording(&self) -> bool`

Returns the current recording state. Used by the main loop and tests.

## Dependencies

| Crate | Usage |
|---|---|
| `cpal` | Cross-platform audio I/O; device enumeration and stream creation |
| `hound` | WAV file writing (`WavWriter`, `WavSpec`) |
| `tracing` | Structured logging via `instrument` macro |
