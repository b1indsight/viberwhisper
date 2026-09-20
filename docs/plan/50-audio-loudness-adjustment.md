# 50 - Bounded Audio Loudness Adjustment and Voice Activity Detection

## Status

Approved and implemented on PR #137 on 2026-09-18. The shared STT entry point now
applies the existing silence gate, local VAD, and bounded upload gain. Source audio
is preserved. Independent review passed with no critical findings. Both platform
CI jobs and the macOS/Windows release dry run passed; no release was published.

### Implementation decisions and local validation

- Embedded Silero v6.2 with its checksum and attribution. Pinned ort/ort-sys rc.9
  and CPU ONNX Runtime 1.20.0 for static linkage: inspected rc.10's arm64 archive
  requires macOS 13.3, while rc.9 retains the promised macOS 11 target. Rubato 0.16.2
  handles anti-aliased analysis resampling with filter-delay/tail compensation.
- WAV validation/preparation lives in `audio/preprocess.rs`, shared by gain and VAD.
  Unsupported VAD rates outside 8–192 kHz fail open; uploads retain source format.
- New tests were first run before implementation and failed on missing behavior.
  Final local suite: 205 passed, one opt-in dataset test ignored by default.
  Formatting, Clippy (all targets), release-contract validation, native release
  build and macOS bundle creation passed. Native release reports macOS 11.0 and
  contains statically linked inference plus embedded model; notices are bundled.
  Intel release also built successfully; the combined universal app contains both
  x86_64 and arm64 slices at deployment target 11.0 and its CLI launch passed.
- Opt-in dataset test passed separately on all 16 locally available ready samples:
  none were entirely rejected. On this machine the optimized preparation median
  was 8.19 ms and maximum 49.06 ms (including initial model setup). Native release
  executable size was 33,383,728 bytes. These are measurements, not performance SLAs.
- Real model speech/noise, phase-inverted stereo, short/tail frame policy,
  resampling anti-aliasing, WAV integer/float formats, bounded gain, immutability,
  no-request rejection, error bypass and identical retry payloads are covered.
- Online recognition comparison could not start: the existing local configuration
  uses schema v2, while current master requires v3. It was left unchanged. No WER
  improvement or representative Chinese/noise false-rejection rate is claimed.
  The 16-sample retention check does not establish each chunk or word survived.
- Local Windows MSVC cross-compilation cannot build existing C dependencies because
  this macOS host lacks Windows SDK headers. Hosted Windows CI uses the normal
  development CRT and runs real model inference; release dry runs additionally
  check both packaged executables and the absence of external runtime DLLs.
- The first hosted run exposed dynamic-CRT references in the prebuilt Windows
  inference archive. Release packaging now builds CPU-only ONNX Runtime from its
  pinned v1.20.0 commit with the static CRT. Ordinary development builds retain
  the toolchain's default CRT. The macOS packaging run passed universal-binary and
  model checks but exhausted the automatically sized DMG volume; it now reserves
  256 MiB before compression to accommodate the bundled native runtime.
- Hosted release run [35335070706](https://github.com/b1indsight/viberwhisper/actions/runs/35335070706)
  passed on implementation commit `1fb767e5fb70`. Windows passed static linking,
  embedded-model inference, runtime dependency checks, MSI install/upgrade/uninstall,
  and portable archive checks; macOS universal packaging also passed. The Windows
  native runtime build took 31m16s; its complete packaging job took 37m42s on that
  hosted runner. These are first-build development costs, not end-user setup work.
  The script pins Eigen's original Git commit, accommodates current MSVC headers
  and diagnostics, and explicitly builds the re2 archive required by ort-sys.

## Goal and scope

Bring quiet, already-audible recordings to a more useful level before STT upload,
without introducing clipping or allowing normalization to bypass silence suppression.
This is an initial amplitude-normalization policy, not perceptual LUFS normalization,
noise removal, compression, or a guarantee of improved recognition accuracy.

Apply the same policy to microphone chunks, offline conversion, and prompt-lab STT
through their shared `ApiTranscriber` entry point. Preserve original captured WAVs so
future regression runs can reproduce preprocessing from the same source audio.
Keep the existing microphone gain configuration and recording callback unchanged.
Previously clipped microphone samples cannot be recovered by this processing.

## Loudness policy

1. Run the existing silence classifier on the original chunk. Keep its 50 ms,
   -50 dBFS threshold and successful empty-result behavior.
2. For an audible chunk, analyze all finite normalized samples. Compute RMS using
   only windows above the existing activity threshold, weighted by their sample
   counts, including a partial final window. Measure peak across all samples.
   These are energy-active windows, not a speech detector.
3. Propose -20 dBFS active-window RMS as the target, +12 dB as the maximum boost,
   and -1 dBFS as the output peak ceiling. These are initial engineering choices
   to validate on representative recordings, not provider requirements.
4. Use one constant gain for the entire chunk and all channels:
   `min(max(1, target_rms / active_rms), max_boost, peak_ceiling / peak)`.
   Only the peak constraint can attenuate already-loud audio. No per-window gain
   changes or compressor state; existing chunk boundaries remain unchanged.
5. Keep unchanged chunks byte-identical. For changed chunks, retain sample rate,
   channel count, frame count, sample format and bit depth using existing `hound`
   support. Check integer rounding bounds and the encoded upload-size limit.
6. If analysis or encoding fails, or data contains unsupported/invalid metadata
   or non-finite samples, log the reason and upload the original bytes, preserving
   the existing fail-open contract. Silent/zero-energy input needs no gain division.
7. Prepare the adjusted chunk once before the retry loop; retries reuse its bytes.

Long pauses must not drive excessive amplification. Audible noise can still be
amplified and isolated peaks can limit the boost; both are accepted limitations of
this first change. Speech below the existing silence threshold remains suppressed.
Each chunk is independent, so adjacent chunks may receive different gains.

## Voice activity detection

Add a local Silero VAD gate to reject audible chunks containing no detected speech.
Processing order is original-audio energy gate, original-audio VAD, bounded loudness
adjustment, then STT upload. VAD decides whether to upload the entire chunk; it does
not trim words, remove interior pauses, alter recording controls, or change chunking.
Speech mixed with background noise still uploads as a complete chunk.

Use a pinned Silero ONNX model with CPU inference through Rust `ort` bindings.
Bundle model bytes and required runtime libraries with releases, including licenses
and a recorded model checksum; do not download models on first launch. Add a tested
anti-aliasing resampler such as `rubato` for a private 16 kHz analysis copy. Uploads
and archived recordings keep their existing sample rates and channel layouts.
Analyze channels separately and accept speech in any channel, avoiding cancellation
from averaging opposite-phase stereo. Bound analysis memory by processing one
channel at a time. Reject invalid metadata through the existing fail-open path.

Use 512-sample frames at 16 kHz, padding the last frame for inference only. Initially
accept the chunk if any frame has speech probability at least 0.3; this deliberately
favors preserving brief/quiet speech over maximum noise rejection. Do not adopt a
minimum utterance duration that would discard short Chinese replies. The threshold
is a proposed fixed policy to validate, not a proven accuracy guarantee. Return
no-speech only after successful analysis of all frames in all channels. Inputs
shorter than one model frame pass through conservatively.

Reset recurrent state and model context for each channel and independent chunk.
Keep model access bounded and synchronized across parallel transcription calls;
do not leak one recording's inference state into another. Run inference off the
audio callback, at most once per chunk before network retries. Model initialization,
resampling or inference errors log a diagnostic and bypass VAD, retaining the
existing energy gate and normal upload behavior. A VAD rejection returns the same
successful empty result as silence. VAD cannot reliably distinguish the user's
voice from background conversation, television speech, or all kinds of music.

Before integrating, verify the pinned model/runtime combination builds and packages
on every supported macOS and Windows release target. Runtime size and native linking
are explicit costs of this choice. If packaging cannot be supported, revise this
plan rather than silently shipping a platform with VAD always bypassed.

References: [Silero model and license](https://github.com/snakers4/silero-vad),
[upstream Rust example](https://github.com/snakers4/silero-vad/tree/master/examples/rust-example).

## Module layout

- `src/audio/vad.rs`: model adapter, analysis resampling and chunk-level decision.
- `assets/`: pinned model and attribution; `Cargo.toml` / `Cargo.lock`: inference
  and resampling dependencies. Release packaging includes native runtime assets.
- `src/audio/loudness.rs`: WAV analysis and bounded constant-gain adjustment;
  focused tests beside the implementation.
- `src/audio/signal.rs`: share the existing activity-window policy where necessary
  without changing the classifier's early-exit behavior or thresholds.
- `src/audio.rs`: expose the preprocessing helper within the crate.
- `src/transcriber/api.rs`: VAD then normalize after silence classification, before retry;
  local HTTP-stub regression coverage for actual uploaded audio.
- `docs/architecture/audio.md` and `changelog`: document processing order, fixed
  policy, and the distinction from microphone capture gain.

No configuration migration or new CLI option. VAD introduces dependencies and may
require platform-specific packaging changes, but uses one shared detection policy.

## Implementation and validation

1. Add failing deterministic tests for quiet audible input, long pauses, bounded
   boost, peak protection, silence, and malformed/non-finite fallback. Include
   representative integer/float and stereo WAVs with preserved frame counts.
2. Verify model/runtime packaging, then implement the audio-owned adjustment,
   VAD adapter, and minimal shared window policy. Test VAD policy with deterministic
   probability sequences, including short speech, tail frames, multi-channel audio,
   resampling duration, state reset between chunks, and failure bypass. Include small
   licensed speech/noise fixtures to exercise the real bundled model offline.
3. Integrate once before upload retries. Use the existing HTTP stub to prove quiet
   input is adjusted, silence sends no request, original WAV bytes remain immutable,
   and repeated requests contain identical prepared audio.
   Also prove audible non-speech rejection avoids HTTP requests, detected speech
   preserves the full chunk, and VAD failure still permits transcription.
4. Update architecture documentation and changelog. Run focused tests, formatting,
   the full test suite, and repository-required native/cross-platform checks.
5. If a ready dataset and working STT credentials are available, compare unchanged
   source samples before/after with the same model and prompt, inspecting WER and
   semantic regressions. Report unavailable empirical validation explicitly; signal
   tests alone do not establish a recognition improvement.
   Compare baseline, loudness alone, VAD alone, and both on the same recordings.
   Include short Chinese replies, quiet speech, keyboard/fan noise and speech mixed
   with noise; report false rejections and non-speech uploads separately. Measure
   preprocessing latency and packaged size in addition to recognition quality.

Keep this plan and implementation on one bookmark and one PR against `master`.
