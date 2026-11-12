//! Sample-exact recognition boundaries. Analysis never trims, overlaps or owns source audio.

use super::vad::{FrameScore, SPEECH_THRESHOLD, StreamingVad};

// 25 consecutive 32 ms frames protect ordinary breaths while allowing early submission.
const PAUSE_FRAMES: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segmentation {
    Fixed,
    PauseAware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutReason {
    Pause,
    Limit,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cut {
    /// Exclusive end offset in the current push, never in an earlier packet.
    pub(crate) offset: usize,
    pub(crate) reason: CutReason,
    /// None requires the existing whole-segment gate; never means proven silence.
    pub(crate) speech: Option<bool>,
}

/// One recording's counters and bounded analysis residual, independent of transport packets.
pub(crate) struct Segmenter {
    rate: u32,
    max_samples: usize,
    open_samples: usize,
    source_samples: u64,
    segment_start: u64,
    step_samples: usize,
    step: Vec<i16>,
    detector: Option<StreamingVad>,
    speech_seen: bool,
    ambiguous_speech: bool,
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
            rate,
            max_samples,
            open_samples: 0,
            source_samples: 0,
            segment_start: 0,
            step_samples: (rate as usize / 50).max(1),
            step: Vec::new(),
            detector,
            speech_seen: false,
            ambiguous_speech: false,
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
        mut analyze: impl FnMut(&mut StreamingVad, &[i16]) -> anyhow::Result<Vec<FrameScore>>,
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
            self.source_samples += count as u64;
            if self.step.len() == self.step_samples {
                let result = analyze(self.detector.as_mut().unwrap(), &self.step);
                self.step.clear();
                match result {
                    Ok(scores) => {
                        for score in scores {
                            if score.source_end <= self.segment_start {
                                continue;
                            }
                            if score.source_start < self.segment_start {
                                self.ambiguous_speech |= score.probability >= SPEECH_THRESHOLD;
                                continue;
                            }
                            if score.probability >= SPEECH_THRESHOLD {
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
                cuts.push(self.close(offset, reason));
            }
        }
        cuts
    }

    /// Close a nonempty tail once. Its offset is zero because finish consumes no packet.
    pub(crate) fn finish(&mut self) -> Option<Cut> {
        let cut = (self.open_samples != 0).then(|| self.close(0, CutReason::Stop));
        self.step.clear();
        cut
    }

    fn close(&mut self, offset: usize, reason: CutReason) -> Cut {
        let speech = self.speech_evidence();
        self.open_samples = 0;
        self.segment_start = self.source_samples;
        self.speech_seen = false;
        self.ambiguous_speech = false;
        self.quiet_frames = 0;
        Cut {
            offset,
            reason,
            speech,
        }
    }

    /// Known speech short-circuits the existence gate; only unfinished frames need inference.
    fn speech_evidence(&self) -> Option<bool> {
        let detector = self.detector.as_ref()?;
        if self.speech_seen || self.open_samples as u64 * 1000 < u64::from(self.rate) * 32 {
            tracing::debug!(
                gate_path = "reuse",
                samples = self.open_samples,
                "Segment contains speech"
            );
            return Some(true);
        }
        let scores = match detector.preview_tail(&self.step) {
            Ok(scores) => scores,
            Err(error) => {
                tracing::warn!(%error, "Tail detection failed; using whole-segment gate");
                return None;
            }
        };
        tracing::debug!(
            gate_path = "tail",
            frames = scores.len(),
            samples = self.open_samples,
            "Checking segment tail"
        );
        let mut ambiguous = self.ambiguous_speech;
        for score in scores {
            if score.source_end <= self.segment_start || score.probability < SPEECH_THRESHOLD {
                continue;
            }
            if score.source_start >= self.segment_start {
                return Some(true);
            }
            ambiguous = true;
        }
        (!ambiguous).then_some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tail_is_analyzed_at_stop_and_hard_limit() {
        // 50 ms at 24 kHz ends before the streaming FFT returns a speech frame.
        // Dropping unfinished analysis would incorrectly cache this segment as silent.
        let wav = crate::audio::WavChunk::from_encoded_bytes(
            include_bytes!("../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (rate, pcm) = crate::audio::decode_mono(&wav).unwrap();
        let pcm = crate::audio::PcmResampler::new(rate, 24_000)
            .unwrap()
            .push(&pcm, true)
            .unwrap();
        for limit in [1200, 720_000] {
            let mut segmenter = Segmenter::new(24_000, limit, Segmentation::PauseAware);
            let mut cuts = segmenter.push(&pcm[..1200]);
            cuts.extend(segmenter.finish());
            assert_eq!(cuts.len(), 1);
            assert_eq!(cuts[0].speech, Some(true));
        }
    }

    #[test]
    fn delayed_previous_speech_cannot_open_the_next_silent_segment() {
        // FFT output can arrive after a hard cut. Returning the old segment's
        // positive frame at that point must not allow the next silent upload.
        for (start, end, expected) in [(0, 512, Some(false)), (512, 1024, None)] {
            let mut segmenter = Segmenter::new(16_000, 640, Segmentation::PauseAware);
            let mut step = 0;
            let first = segmenter.push_with(&[0; 640], |_, _| {
                step += 1;
                Ok(if step == 2 {
                    vec![FrameScore {
                        source_start: 0,
                        source_end: 512,
                        probability: 0.9,
                    }]
                } else {
                    Vec::new()
                })
            });
            assert_eq!(first[0].speech, Some(true));
            step = 0;
            let next = segmenter.push_with(&[0; 640], |_, _| {
                step += 1;
                Ok(if step == 2 {
                    vec![FrameScore {
                        source_start: start,
                        source_end: end,
                        probability: 0.9,
                    }]
                } else {
                    Vec::new()
                })
            });
            assert_eq!(next[0].speech, expected);
        }
    }

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
                    scores.push(FrameScore {
                        source_start: ordinal as u64 * 512,
                        source_end: (ordinal as u64 + 1) * 512,
                        probability: probabilities[ordinal],
                    });
                    ordinal += 1;
                }
                Ok(scores)
            }) {
                assert_eq!(cut.reason, CutReason::Pause);
                cuts.push(base + cut.offset);
            }
            base += packet.len();
        }
        assert_eq!(
            segmenter.finish().map(|cut| cut.reason),
            Some(CutReason::Stop)
        );
        assert_eq!(segmenter.finish(), None);
        cuts
    }

    #[test]
    fn breaths_do_not_cut_and_resumed_speech_restarts_the_pause() {
        for quiet_frames in [6, 12, 18, 24] {
            let mut scores = vec![0.2, 0.7];
            scores.extend(vec![0.1999; quiet_frames]);
            scores.push(0.2);
            scores.extend(vec![0.1999; 25]);
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
                    for cut in segmenter.push_with(packet, |_, _| Ok(Vec::new())) {
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
                    segmenter.finish().map(|cut| cut.reason),
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
        assert_eq!(
            segmenter.finish().map(|cut| cut.reason),
            Some(CutReason::Stop)
        );
    }
}
