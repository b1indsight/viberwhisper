# Fixed audio chunk capacity

## Scope and approval

After the audio simplification PR merged, the user approved removing the optional
capacity on 2026-09-08. Both microphone recording and offline WAV iteration use
the audio module's fixed 30-second / 23-MiB policy. Long offline files keep one
WAV reader and yield bounded chunks until EOF, including a shorter final tail.

## Design

- `max_frames_per_chunk(WavSpec)` returns `Result<u64, ChunkError>`. Remove
  disabled-limit handling and calculate the smaller duration/size capacity using
  the module-owned constants. Preserve zero capacity for formats whose size
  allowance holds no more than half a second, plus existing invalid-spec errors.
- Store plain integer capacities in the active recorder and offline reader.
  Remove unlimited-output branches; iterator EOF remains represented by `None`.
- `WavChunkReader::open` accepts only its source path. Remove fixed-limit fields
  and arguments from `AudioConfig`, `AudioRecorder`, prompt-lab evaluation, and
  their callers rather than continuing to forward constants.
- Keep the existing stop delay, sample encoding, live error recovery, notification
  boundaries, and terminal offline decode errors.

## Implementation and validation

1. Adapt capacity tests to exercise the fixed duration and size policy, and
   offline tests to cover a 65-second source split into 30/30/5-second chunks.
2. Remove optional capacities and the no-longer-variable limit plumbing.
3. Update the audio architecture document and changelog.
4. Run focused audio tests, formatting, build, the full test suite, Clippy,
   and diff whitespace checks. Run the independent review gate before pushing
   implementation, then verify macOS and Windows CI on this follow-up PR.

## Local result

The fixed-policy API and caller cleanup are implemented. All 27 audio tests and
the full 188-test suite passed. Build, formatting, Clippy with warnings denied,
and diff whitespace checks passed. No real-device recording was required for
this capacity/API change; the existing stop and callback behavior is preserved.
