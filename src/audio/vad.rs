//! Local Silero inference on an analysis-only 16 kHz copy; no source audio is trimmed.

use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, OnceLock},
};

use anyhow::{Context, Result, anyhow, ensure};
use ort::{session::Session, value::Tensor};
use rubato::{FftFixedInOut, Resampler};

use super::{PcmResampler, preprocess::DecodedWav};

const MODEL: &[u8] = include_bytes!("../../assets/vad/silero-v6.2.onnx");
const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = 512;
const CONTEXT: usize = 64;
pub(super) const SPEECH_THRESHOLD: f32 = 0.2;

/// An analysis frame's source interval, independent of when resampling returns it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct FrameScore {
    pub(super) source_start: u64,
    pub(super) source_end: u64,
    pub(super) probability: f32,
}

impl FrameScore {
    fn new(index: u64, probability: f32, rate: u32, source_end: u64) -> Self {
        let frame_samples = FRAME as u64 * u64::from(rate);
        Self {
            source_start: index * frame_samples / SAMPLE_RATE as u64,
            source_end: ((index + 1) * frame_samples)
                .div_ceil(SAMPLE_RATE as u64)
                .min(source_end),
            probability,
        }
    }
}

fn detector() -> Result<MutexGuard<'static, Detector>> {
    static DETECTOR: OnceLock<Result<Mutex<Detector>, String>> = OnceLock::new();
    let detector = DETECTOR.get_or_init(|| {
        Detector::new()
            .map(Mutex::new)
            .map_err(|e| format!("{e:#}"))
    });
    detector
        .as_ref()
        .map_err(|e| anyhow!("VAD initialization: {e}"))?
        .lock()
        .map_err(|_| anyhow!("VAD lock poisoned"))
}

/// Serialized inference only; every channel starts with fresh recurrent state.
pub(super) fn contains_speech(pcm: &DecodedWav) -> Result<bool> {
    for channel in 0..usize::from(pcm.spec.channels) {
        // Analyze separately so opposite-phase microphones cannot cancel speech.
        let samples: Vec<_> = pcm
            .samples
            .iter()
            .skip(channel)
            .step_by(usize::from(pcm.spec.channels))
            .map(|s| *s as f32)
            .collect();
        let samples = analysis_samples(&samples, pcm.spec.sample_rate)?;
        let mut state = [0.0; 256];
        let mut context = [0.0; CONTEXT];
        if classify_channel(&samples, |frame| {
            detector()?.predict_frame(frame, &mut state, &mut context)
        })? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Per-recording analysis state, continuous across packets and recognition cuts.
pub(super) struct StreamingVad {
    resampler: PcmResampler,
    pending: VecDeque<i16>,
    state: [f32; 256],
    context: [f32; CONTEXT],
    processed_frames: u64,
}

impl StreamingVad {
    pub(super) fn new(rate: u32) -> Result<Self> {
        drop(detector()?);
        Ok(Self {
            resampler: PcmResampler::new(rate, SAMPLE_RATE as u32)?,
            pending: VecDeque::new(),
            state: [0.0; 256],
            context: [0.0; CONTEXT],
            processed_frames: 0,
        })
    }

    /// Consume only new source PCM; filter and inference state survive recognition cuts.
    pub(super) fn push(&mut self, pcm: &[i16]) -> Result<Vec<FrameScore>> {
        self.pending.extend(self.resampler.push(pcm, false)?);
        let mut probabilities = Vec::new();
        while self.pending.len() >= FRAME {
            let mut frame = [0.0; FRAME];
            for (output, input) in frame.iter_mut().zip(self.pending.drain(..FRAME)) {
                *output = f32::from(input) / 32768.0;
            }
            let probability =
                detector()?.predict_frame(&frame, &mut self.state, &mut self.context)?;
            probabilities.push(FrameScore::new(
                self.processed_frames,
                probability,
                self.resampler.rate(),
                self.resampler.input_count as u64,
            ));
            self.processed_frames += 1;
        }
        Ok(probabilities)
    }

    /// Analyze only unfinished frames on copied inference state. `extra_source` is
    /// the segmenter's small-step residual, which the live detector has not consumed.
    pub(super) fn preview_tail(&self, extra_source: &[i16]) -> Result<Vec<FrameScore>> {
        let mut pending = self.pending.clone();
        pending.extend(self.resampler.preview_finish(extra_source)?);
        let mut state = self.state;
        let mut context = self.context;
        let source_end = (self.resampler.input_count + extra_source.len()) as u64;
        let mut scores = Vec::new();
        while !pending.is_empty() {
            let count = FRAME.min(pending.len());
            let mut frame = [0.0; FRAME];
            for (output, sample) in frame.iter_mut().zip(pending.drain(..count)) {
                *output = f32::from(sample) / 32768.0;
            }
            let probability = detector()?.predict_frame(&frame, &mut state, &mut context)?;
            scores.push(FrameScore::new(
                self.processed_frames + scores.len() as u64,
                probability,
                self.resampler.rate(),
                source_end,
            ));
        }
        Ok(scores)
    }
}

struct Detector {
    session: Session,
}

impl Detector {
    fn new() -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_memory(MODEL)
            .context("loading bundled Silero VAD")?;
        Ok(Self { session })
    }

    #[cfg(test)]
    fn classify(&mut self, samples: &[f32]) -> Result<bool> {
        let mut state = [0.0_f32; 256];
        let mut context = [0.0_f32; CONTEXT];
        classify_channel(samples, |frame| {
            self.predict_frame(frame, &mut state, &mut context)
        })
    }

    fn predict_frame(
        &mut self,
        frame: &[f32],
        state: &mut [f32; 256],
        context: &mut [f32; CONTEXT],
    ) -> Result<f32> {
        let mut input = Vec::with_capacity(CONTEXT + FRAME);
        input.extend_from_slice(context);
        input.extend_from_slice(frame);
        let output = self.session.run(ort::inputs![
            "input" => Tensor::from_array(([1, CONTEXT + FRAME], input))?,
            "state" => Tensor::from_array(([2, 1, 128], state.to_vec()))?,
            "sr" => Tensor::from_array((Vec::<usize>::new(), vec![SAMPLE_RATE as i64]))?,
        ]?)?;
        let (_, next_state) = output["stateN"].try_extract_raw_tensor::<f32>()?;
        ensure!(
            next_state.len() == state.len() && next_state.iter().all(|s| s.is_finite()),
            "invalid VAD state"
        );
        let (_, probability) = output["output"].try_extract_raw_tensor::<f32>()?;
        let probability = probability
            .first()
            .copied()
            .context("missing VAD probability")?;
        ensure!(
            probability.is_finite() && (0.0..=1.0).contains(&probability),
            "invalid VAD probability"
        );
        state.copy_from_slice(next_state);
        context.copy_from_slice(&frame[FRAME - CONTEXT..]);
        Ok(probability)
    }
}

fn classify_channel(
    samples: &[f32],
    mut predict: impl FnMut(&[f32]) -> Result<f32>,
) -> Result<bool> {
    if samples.len() < FRAME {
        return Ok(true);
    }
    for frame in samples.chunks(FRAME) {
        let mut padded = [0.0; FRAME];
        padded[..frame.len()].copy_from_slice(frame);
        let probability = predict(&padded)?;
        ensure!(
            probability.is_finite() && (0.0..=1.0).contains(&probability),
            "invalid VAD probability"
        );
        if probability >= SPEECH_THRESHOLD {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Anti-aliased conversion, compensating filter delay and flushing the last speech samples.
fn analysis_samples(samples: &[f32], rate: u32) -> Result<Vec<f32>> {
    ensure!(
        (8_000..=192_000).contains(&rate),
        "unsupported VAD input rate: {rate}"
    );
    ensure!(
        samples.iter().all(|s| s.is_finite()),
        "non-finite VAD input"
    );
    if rate as usize == SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    let expected = samples
        .len()
        .checked_mul(SAMPLE_RATE)
        .context("VAD duration overflow")?
        .div_ceil(rate as usize);
    let mut resampler = FftFixedInOut::<f32>::new(rate as usize, SAMPLE_RATE, 1_024, 1)?;
    let delay = resampler.output_delay();
    let input_size = resampler.input_frames_next();
    let mut output = Vec::with_capacity(expected + delay + resampler.output_frames_max());
    let mut input = samples;
    while output.len() < expected + delay {
        let count = input.len().min(input_size);
        let block = if count == 0 {
            resampler.process_partial::<&[f32]>(None, None)?
        } else {
            resampler.process_partial(Some(&[&input[..count]]), None)?
        };
        output.extend_from_slice(&block[0]);
        input = &input[count..];
    }
    let output = output[delay..delay + expected].to_vec();
    ensure!(
        output.iter().all(|s| s.is_finite()),
        "non-finite resampled audio"
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_preview_matches_terminal_inference_without_advancing_live_state() {
        // The stop tail needs the preceding recurrent state, while an internal cut's
        // padded analysis must not become part of the next segment's live context.
        let wav = crate::audio::WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (input_rate, speech) = crate::audio::decode_mono(&wav).unwrap();
        for rate in [16_000, 24_000, 44_100, 48_000] {
            let pcm = PcmResampler::new(input_rate, rate)
                .unwrap()
                .push(&speech, true)
                .unwrap();
            for cut in [1, rate as usize / 10 + 13, pcm.len() / 2 + 7] {
                let mut live = StreamingVad::new(rate).unwrap();
                let mut terminal = StreamingVad::new(rate).unwrap();
                let mut uninterrupted = StreamingVad::new(rate).unwrap();
                live.push(&pcm[..cut]).unwrap();
                terminal.push(&pcm[..cut]).unwrap();
                uninterrupted.push(&pcm[..cut]).unwrap();
                let extra = &pcm[cut..cut + 701];
                terminal
                    .pending
                    .extend(terminal.resampler.push(extra, true).unwrap());
                let mut expected = Vec::new();
                while !terminal.pending.is_empty() {
                    let count = FRAME.min(terminal.pending.len());
                    let mut frame = [0.0; FRAME];
                    for (out, sample) in frame.iter_mut().zip(terminal.pending.drain(..count)) {
                        *out = f32::from(sample) / 32768.0;
                    }
                    expected.push(
                        detector()
                            .unwrap()
                            .predict_frame(&frame, &mut terminal.state, &mut terminal.context)
                            .unwrap(),
                    );
                }
                let preview = live.preview_tail(extra).unwrap();
                assert_eq!(
                    preview.iter().map(|s| s.probability).collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(live.preview_tail(extra).unwrap(), preview);
                assert_eq!(
                    live.push(&pcm[cut..]).unwrap(),
                    uninterrupted.push(&pcm[cut..]).unwrap()
                );
            }
        }
    }

    #[test]
    fn streaming_probabilities_survive_packetization_and_batch_interleaving() {
        let speech = crate::audio::WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (rate, mut pcm) = crate::audio::decode_mono(&speech).unwrap();
        pcm.extend(vec![0; rate as usize * 2]);
        let expected = StreamingVad::new(rate).unwrap().push(&pcm).unwrap();
        let mut streaming = StreamingVad::new(rate).unwrap();
        let mut actual = Vec::new();
        for packet in pcm.chunks(317) {
            actual.extend(streaming.push(packet).unwrap());
            assert!(contains_speech(&DecodedWav::read(&speech).unwrap()).unwrap());
        }
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|p| p.probability >= SPEECH_THRESHOLD));
        assert!(
            actual[actual.len() - 25..]
                .iter()
                .all(|p| p.probability < SPEECH_THRESHOLD)
        );
    }

    #[test]
    fn short_inputs_pass_and_partial_tail_speech_is_preserved() {
        assert!(classify_channel(&[0.0; 100], |_| panic!("too short for VAD")).unwrap());
        for (probability, speech) in [(0.1999, false), (0.2, true), (0.3, true)] {
            assert_eq!(
                classify_channel(&[0.0; 512], |_| Ok(probability)).unwrap(),
                speech
            );
        }
        let mut calls = 0;
        let result = classify_channel(&[0.0; 513], |frame| {
            assert_eq!(frame.len(), 512);
            calls += 1;
            Ok(if calls == 2 { 0.3 } else { 0.1 })
        })
        .unwrap();
        assert!(result);
        assert_eq!(calls, 2);
        assert!(!classify_channel(&[0.0; 1_024], |_| Ok(0.1)).unwrap());
        assert!(classify_channel(&[0.0; 512], |_| Ok(f32::NAN)).is_err());
    }

    #[test]
    fn resampling_preserves_duration_and_flushes_tail() {
        for rate in [8_000, 16_000, 44_100, 48_000] {
            let mut samples = vec![0.0; rate / 10];
            let len = samples.len();
            samples[len - 10..].fill(0.5);
            let output = analysis_samples(&samples, rate as u32).unwrap();
            assert_eq!(output.len(), 1_600);
            assert!(output[1_580..].iter().any(|sample| sample.abs() > 0.1));
        }
    }

    #[test]
    fn bundled_model_rejects_noise_and_resets_between_chunks() {
        use sha2::{Digest, Sha256};
        assert_eq!(
            format!("{:x}", Sha256::digest(MODEL)),
            "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3"
        );
        let mut detector = Detector::new().unwrap();
        let quiet = vec![0.0; 16_000];
        assert!(!detector.classify(&quiet).unwrap());
        let noise: Vec<_> = (0..16_000)
            .map(|i| ((i * 7919 % 101) as f32 - 50.0) * 0.001)
            .collect();
        assert!(!detector.classify(&noise).unwrap());
        assert!(!detector.classify(&quiet).unwrap());
        let speech = crate::audio::WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let pcm = DecodedWav::read(&speech).unwrap();
        let samples: Vec<_> = pcm.samples.iter().map(|x| *x as f32).collect();
        assert!(detector.classify(&samples).unwrap());
        assert!(!detector.classify(&quiet).unwrap());
        // Opposite-phase stereo must not lose speech through downmix cancellation.
        let stereo = DecodedWav {
            spec: hound::WavSpec {
                channels: 2,
                ..pcm.spec
            },
            samples: pcm.samples.iter().flat_map(|s| [*s, -*s]).collect(),
        };
        assert!(contains_speech(&stereo).unwrap());
    }

    #[test]
    fn resampling_rejects_out_of_band_energy_instead_of_aliasing_into_speech() {
        let samples: Vec<_> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 12_000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let output = analysis_samples(&samples, 48_000).unwrap();
        let power = output[1_000..15_000].iter().map(|s| s * s).sum::<f32>() / 14_000.0;
        assert!(
            power < 0.0001,
            "12 kHz noise aliased into 16 kHz analysis: {power}"
        );
    }
}
