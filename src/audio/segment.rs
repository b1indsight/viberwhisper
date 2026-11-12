//! Sample-exact recognition boundaries. Analysis never trims, overlaps or owns source audio.

use super::vad::{SPEECH_THRESHOLD, StreamingVad};

// 25 consecutive 32 ms frames protect ordinary breaths while allowing early submission.
const PAUSE_FRAMES: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segmentation {
    #[cfg(test)]
    Fixed,
    PauseAware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutReason {
    Pause,
    Limit,
    Stop,
}

pub(crate) struct Cut {
    /// Exclusive end offset in the current push, never in an earlier packet.
    pub(crate) offset: usize,
    pub(crate) reason: CutReason,
}

/// One recording's counters and bounded analysis residual, independent of transport packets.
pub(crate) struct Segmenter {
    max_samples: usize,
    open_samples: usize,
    step_samples: usize,
    step: Vec<i16>,
    detector: Option<StreamingVad>,
    speech_seen: bool,
    quiet_frames: usize,
}

impl Segmenter {
    pub(crate) fn new(rate: u32, max_samples: usize, mode: Segmentation) -> Self {
        assert!(max_samples > 0);
        let detector = if mode == Segmentation::PauseAware {
            match StreamingVad::new(rate) {
                Ok(detector) => Some(detector),
                Err(error) => {
                    tracing::warn!(%error, "Endpoint detection unavailable; using fixed segments");
                    None
                }
            }
        } else {
            None
        };
        Self {
            max_samples,
            open_samples: 0,
            step_samples: (rate as usize / 50).max(1),
            step: Vec::new(),
            detector,
            speech_seen: false,
            quiet_frames: 0,
        }
    }

    /// Return strictly increasing cut offsets; consumers retain every contiguous source slice.
    pub(crate) fn push(&mut self, pcm: &[i16]) -> Vec<Cut> {
        self.push_with(pcm, StreamingVad::push)
    }

    fn push_with(
        &mut self,
        pcm: &[i16],
        mut analyze: impl FnMut(&mut StreamingVad, &[i16]) -> anyhow::Result<Vec<f32>>,
    ) -> Vec<Cut> {
        let mut offset = 0;
        let mut cuts = Vec::new();
        while offset < pcm.len() {
            let mut count = (self.max_samples - self.open_samples).min(pcm.len() - offset);
            if self.detector.is_some() {
                count = count.min(self.step_samples - self.step.len());
                self.step.extend_from_slice(&pcm[offset..offset + count]);
            }
            offset += count;
            self.open_samples += count;
            if self.step.len() == self.step_samples {
                let result = analyze(self.detector.as_mut().unwrap(), &self.step);
                self.step.clear();
                match result {
                    Ok(scores) => {
                        for score in scores {
                            if score >= SPEECH_THRESHOLD {
                                self.speech_seen = true;
                                self.quiet_frames = 0;
                            } else if self.speech_seen {
                                self.quiet_frames += 1;
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Endpoint detection failed; using fixed segments for this recording");
                        self.detector = None;
                        self.speech_seen = false;
                        self.quiet_frames = 0;
                    }
                }
            }
            let reason = if self.open_samples == self.max_samples {
                Some(CutReason::Limit)
            } else if self.speech_seen && self.quiet_frames >= PAUSE_FRAMES {
                Some(CutReason::Pause)
            } else {
                None
            };
            if let Some(reason) = reason {
                cuts.push(Cut { offset, reason });
                self.open_samples = 0;
                self.speech_seen = false;
                self.quiet_frames = 0;
            }
        }
        cuts
    }

    /// Close a nonempty tail once, without fabricating silence to reach the pause threshold.
    pub(crate) fn finish(&mut self) -> Option<CutReason> {
        let nonempty = self.open_samples != 0;
        self.open_samples = 0;
        self.step.clear();
        self.speech_seen = false;
        self.quiet_frames = 0;
        nonempty.then_some(CutReason::Stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_endpoint_preserves_breaths_and_packet_independence() {
        let wav = super::super::WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (_, speech) = crate::audio::decode_mono(&wav).unwrap();
        for rate in [16_000, 44_100, 48_000] {
            let voice = crate::audio::PcmResampler::new(16_000, rate)
                .unwrap()
                .push(&speech, true)
                .unwrap();
            for pause_ms in [600, 2000] {
                let mut pcm = voice.clone();
                pcm.extend(vec![0; rate as usize * pause_ms / 1000]);
                pcm.extend_from_slice(&voice);
                let mut packetizations = Vec::new();
                for packet_size in [317, rate as usize / 5] {
                    let mut segmenter =
                        Segmenter::new(rate, rate as usize * 30, Segmentation::PauseAware);
                    let mut base = 0;
                    let mut offsets = Vec::new();
                    for packet in pcm.chunks(packet_size) {
                        offsets.extend(
                            segmenter
                                .push(packet)
                                .into_iter()
                                .map(|cut| base + cut.offset),
                        );
                        base += packet.len();
                    }
                    assert_eq!(offsets.len(), usize::from(pause_ms == 2000));
                    if let Some(offset) = offsets.first() {
                        assert!((rate as usize * 3..rate as usize * 5).contains(offset));
                    }
                    packetizations.push(offsets);
                }
                assert_eq!(packetizations[0], packetizations[1]);
            }
        }
    }

    // Feed deterministic 32 ms probabilities through the same source-cursor policy.
    fn scripted(parts: usize, probabilities: &[f32]) -> Vec<usize> {
        let pcm = vec![1; probabilities.len() * 512 + 319];
        let mut segmenter = Segmenter::new(16_000, 480_000, Segmentation::PauseAware);
        let mut analyzed = 0;
        let mut ordinal = 0;
        let mut cuts = Vec::new();
        let mut base = 0;
        for packet in pcm.chunks(parts) {
            for cut in segmenter.push_with(packet, |_, step| {
                analyzed += step.len();
                let mut scores = Vec::new();
                while analyzed >= 512 {
                    analyzed -= 512;
                    scores.push(probabilities[ordinal]);
                    ordinal += 1;
                }
                Ok(scores)
            }) {
                assert_eq!(cut.reason, CutReason::Pause);
                cuts.push(base + cut.offset);
            }
            base += packet.len();
        }
        assert_eq!(segmenter.finish(), Some(CutReason::Stop));
        assert_eq!(segmenter.finish(), None);
        cuts
    }

    #[test]
    fn breaths_do_not_cut_and_resumed_speech_restarts_the_pause() {
        for quiet_frames in [6, 12, 18, 24] {
            let mut scores = vec![0.3, 0.7];
            scores.extend(vec![0.1; quiet_frames]);
            scores.push(0.3);
            scores.extend(vec![0.1; 25]);
            let endpoint = (scores.len() * 512).div_ceil(320) * 320;
            for packet in [1, 317, 3200, 19_773] {
                assert_eq!(scripted(packet, &scores), [endpoint]);
            }
        }
        assert!(scripted(317, &[0.0; 200]).is_empty());
    }

    #[test]
    fn hard_limits_and_stop_preserve_exact_sample_counts_at_all_rates() {
        for rate in [16_000, 24_000, 44_100, 48_000] {
            let limit = rate as usize * 30;
            for length in [
                limit - rate as usize / 100,
                limit - 1,
                limit,
                limit + 1,
                limit + rate as usize / 100,
                limit * 3 + 1,
            ] {
                let mut segmenter = Segmenter::new(rate, limit, Segmentation::PauseAware);
                let mut cuts = Vec::new();
                let mut base = 0;
                for packet in vec![7; length].chunks(317) {
                    for cut in segmenter.push_with(packet, |_, _| Ok(vec![0.0])) {
                        assert_eq!(cut.reason, CutReason::Limit);
                        cuts.push(base + cut.offset);
                    }
                    base += packet.len();
                }
                assert_eq!(
                    cuts,
                    (1..=length / limit).map(|n| n * limit).collect::<Vec<_>>()
                );
                assert_eq!(
                    segmenter.finish(),
                    (length % limit != 0).then_some(CutReason::Stop)
                );
                assert_eq!(segmenter.finish(), None);
            }
        }
    }

    #[test]
    fn analysis_failure_keeps_audio_and_falls_back_for_the_recording() {
        let mut segmenter = Segmenter::new(16_000, 1001, Segmentation::PauseAware);
        assert!(
            segmenter
                .push_with(&[1; 319], |_, _| unreachable!())
                .is_empty()
        );
        let cuts = segmenter.push_with(&[2; 1684], |_, _| anyhow::bail!("inference unavailable"));
        assert_eq!(
            cuts.iter()
                .map(|c| (c.offset, c.reason))
                .collect::<Vec<_>>(),
            [(682, CutReason::Limit), (1683, CutReason::Limit)]
        );
        assert!(
            segmenter
                .push_with(&[], |_, _| panic!("must stay fixed"))
                .is_empty()
        );
        assert_eq!(segmenter.finish(), Some(CutReason::Stop));
    }
}
