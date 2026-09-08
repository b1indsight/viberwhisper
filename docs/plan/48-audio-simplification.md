# Audio implementation simplification

## Scope and approval

The user approved the audio review's items 1, 2, 3, 5, and 6 on 2026-09-08,
explicitly excluding item 4. This plan records that approved scope, including
the smaller WAV output-buffer preallocation improvement.

Keep the existing 200 ms stop delay, chunk limits, notification semantics,
sample conversion rules, and live-encoding failure recovery behavior.

## Design

1. Build I16 and F32 input callbacks through one private helper. Keep their
   numeric conversions explicit, while sharing downmixing, scratch-buffer reuse,
   PCM publication, and chunk notifications. Reuse an owned scratch vector
   between callbacks instead of allocating a fresh vector for every input buffer.
2. Represent the recorder's main-thread lifecycle with `Option<ActiveRecording>`.
   Keep the callback's atomic recording switch with the active session's shared
   buffers and counters. Session identity, stream, sample rate, and chunk progress
   belong to the active recording; configuration and the notifier belong to the
   recorder. Stop and cancel take ownership of the matched active session. Keep
   live PCM until successful encoding and remove unreachable stop outcomes.
3. After shutting down the stream, move the remaining PCM out of its mutex with
   `mem::take` and encode outside the lock. Stopping still flushes complete chunks
   followed by the tail, and distinguishes empty sessions from fully flushed ones.
4. Resolve a requested device directly from the readable device/name pairs,
   preserving first exact-match selection and skipping unreadable names.
5. Implement `Iterator<Item = Result<WavChunk, ChunkError>>` directly on
   `WavChunkReader`. Update production callers and tests, removing the forwarding
   `WavChunks` type. Decode errors remain terminal and occur only once.
6. Reserve encoded WAV capacity from its header and sample count with checked
   arithmetic. Apply this to live I16 encoding and offline format-preserving
   encoding, without changing payload bytes or limits.

## Files and implementation order

1. Strengthen audio tests to cover converted PCM values, callback reuse and
   notification boundaries, ordered live/stop output, and repeated lifecycle calls.
2. Refactor `src/audio/recorder.rs`, retaining its public start/stop/cancel outcomes.
3. Simplify `src/audio/wav_file.rs` and migrate callers in `src/application.rs`
   and `src/prompt_lab/regression.rs`.
4. Add output-buffer reservation in `src/audio/chunk.rs` and the offline encoder.
5. Update `docs/architecture/audio.md`, this plan, and `changelog`.

## Validation

- Run focused audio tests before and after implementation. New tests must protect
  sample order and numeric behavior rather than private field layout.
- Run `cargo fmt --check`, `cargo build --locked`, `cargo test --locked`, and
  `cargo clippy --locked -- -D warnings`.
- Review the final diff and run the required independent review gate before
  pushing implementation to this plan's bookmark and PR.
- Use GitHub CI for the macOS and Windows build/test/lint matrix. Unit tests do
  not establish real-device callback shutdown behavior; the excluded stop delay
  and backend shutdown policy stay unchanged.
