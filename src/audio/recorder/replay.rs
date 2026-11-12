use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::{ActiveRecording, ChunkError, ChunkMode, RecorderError, WavChunk};
use crate::audio::{WavChunkReader, decode_mono};
use crate::session::SessionId;

/// Replays saved PCM at 1x through one production recording, without capture gain or device access.
/// `streaming` selects Realtime frames; otherwise the live HTTP pause-aware policy is used.
/// The callback must enqueue without waiting for ASR, just like live chunk delivery.
pub(crate) fn replay_wav(
    path: &Path,
    session_id: SessionId,
    streaming: bool,
    cancelled: &CancellationToken,
    on_chunk: impl FnMut(WavChunk),
) -> anyhow::Result<()> {
    anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
    let reader = WavChunkReader::open(path)?;
    let started = Instant::now();
    replay_chunks(
        reader,
        session_id,
        streaming,
        cancelled,
        |offset, token| wait_for_replay(started + offset, token),
        on_chunk,
    )
}

/// I/O packets only bound decode memory. Cadence and recognition cuts belong to the recording.
fn replay_chunks(
    chunks: impl IntoIterator<Item = Result<WavChunk, ChunkError>>,
    session_id: SessionId,
    streaming: bool,
    cancelled: &CancellationToken,
    mut wait_until: impl FnMut(Duration, &CancellationToken) -> anyhow::Result<()>,
    mut on_chunk: impl FnMut(WavChunk),
) -> anyhow::Result<()> {
    anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
    let mut chunks = chunks.into_iter();
    let first = chunks.next().ok_or(RecorderError::NoAudioData)??;
    let (rate, mut pcm) = decode_mono(&first)?;
    drop(first);
    let mode = if streaming {
        ChunkMode::Frames
    } else {
        ChunkMode::Pauses
    };
    let mut active = ActiveRecording::new(session_id, rate, mode)?;
    let cadence = active.shared.wake_samples;
    let mut frame = Vec::new();
    let mut total = 0_u64;
    let mut feed = |frame: &[i16]| -> anyhow::Result<()> {
        total += frame.len() as u64;
        let rate = u64::from(rate);
        let offset = Duration::from_secs(total / rate)
            + Duration::from_nanos(total % rate * 1_000_000_000 / rate);
        wait_until(offset, cancelled)?;
        anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
        // Saved PCM has already passed capture gain; only the live callback applies it.
        active.shared.push_mono(frame);
        if active.shared.overflowed.load(Ordering::Acquire) {
            return Err(RecorderError::StreamingOverflow.into());
        }
        while let Some(ready) = active.take_ready_chunk() {
            on_chunk(ready.chunk);
            anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
        }
        Ok(())
    };

    loop {
        let mut remaining = pcm.as_slice();
        while !remaining.is_empty() {
            let count = (cadence - frame.len()).min(remaining.len());
            frame.extend_from_slice(&remaining[..count]);
            remaining = &remaining[count..];
            if frame.len() == cadence {
                feed(&frame)?;
                frame.clear();
            }
        }
        anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
        let Some(next) = chunks.next() else { break };
        let (next_rate, next_pcm) = decode_mono(&next?)?;
        anyhow::ensure!(next_rate == rate, "WAV sample rate changed during replay");
        pcm = next_pcm;
    }
    if !frame.is_empty() {
        feed(&frame)?;
    }
    anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
    for chunk in active.stop()? {
        on_chunk(chunk);
        anyhow::ensure!(!cancelled.is_cancelled(), "WAV replay cancelled");
    }
    Ok(())
}

struct ReplayWake(thread::Thread);

impl Wake for ReplayWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn wait_for_replay(deadline: Instant, cancelled: &CancellationToken) -> anyhow::Result<()> {
    // Register cancellation before parking, so cancellation racing the wait cannot be missed.
    // This synchronous input adapter does not require (or nest) a Tokio runtime.
    let waker = Waker::from(Arc::new(ReplayWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut cancellation = std::pin::pin!(cancelled.cancelled());
    loop {
        anyhow::ensure!(
            cancellation.as_mut().poll(&mut context).is_pending(),
            "WAV replay cancelled"
        );
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(());
        };
        thread::park_timeout(remaining);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunk::encode_i16_wav;
    use hound::WavSpec;

    fn decode_chunks(chunks: impl IntoIterator<Item = WavChunk>) -> Vec<i16> {
        chunks
            .into_iter()
            .flat_map(|chunk| {
                hound::WavReader::new(std::io::Cursor::new(chunk.bytes()))
                    .unwrap()
                    .samples::<i16>()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            })
            .collect()
    }

    fn replay_offsets(sample_count: usize, rate: u32) -> Vec<Duration> {
        let frame = (rate as usize / 5).max(1);
        (1..=sample_count.div_ceil(frame))
            .map(|index| {
                let samples = (index * frame).min(sample_count) as u64;
                Duration::from_secs(samples / u64::from(rate))
                    + Duration::from_nanos(
                        samples % u64::from(rate) * 1_000_000_000 / u64::from(rate),
                    )
            })
            .collect()
    }

    fn replay_packets(
        pcm: &[i16],
        rate: u32,
        packet_size: usize,
        streaming: bool,
    ) -> Vec<WavChunk> {
        let packets = pcm
            .chunks(packet_size)
            .map(|part| encode_i16_wav(part, rate));
        let mut chunks = Vec::new();
        let mut offsets = Vec::new();
        replay_chunks(
            packets,
            SessionId(41),
            streaming,
            &CancellationToken::new(),
            |offset, _| {
                offsets.push(offset);
                Ok(())
            },
            |chunk| chunks.push(chunk),
        )
        .unwrap();
        // Deadlines are absolute audio offsets, not reset by I/O packets or callback work.
        assert_eq!(offsets, replay_offsets(pcm.len(), rate));
        chunks
    }

    #[test]
    fn wav_replay_preserves_saved_pcm_without_gain_across_read_boundaries() {
        // Nonperiodic signed PCM exposes duplicate gain, reordered reads and a lost short tail.
        // The odd sample rate also makes 30-second I/O blocks end between 200 ms frames.
        for rate in [16_000, 44_101] {
            let mut pcm: Vec<i16> = (0..rate as usize * 31 + 317)
                .map(|n| ((n * 7919 % 60_001) as i32 - 30_000) as i16)
                .collect();
            pcm[..4].copy_from_slice(&[i16::MIN, 29_317, -127, i16::MAX]);
            let expected_lengths: Vec<_> =
                pcm.chunks(rate as usize / 5).map(<[i16]>::len).collect();
            for packet_size in [1733, rate as usize * 30] {
                let chunks = replay_packets(&pcm, rate, packet_size, true);
                let lengths: Vec<_> = chunks
                    .iter()
                    .map(|chunk| {
                        let reader =
                            hound::WavReader::new(std::io::Cursor::new(chunk.bytes())).unwrap();
                        assert_eq!(reader.spec().sample_rate, rate);
                        reader.len() as usize
                    })
                    .collect();
                assert_eq!(lengths, expected_lengths);
                assert_eq!(decode_chunks(chunks), pcm);
            }
        }
    }

    #[test]
    fn wav_replay_uses_shared_stereo_float_conversion_and_validation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let spec = WavSpec {
            channels: 2,
            sample_rate: 24_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
        let stereo = [
            [0.75_f32, -0.25],
            [-1.0, 0.25],
            [0.125, 0.375],
            [-0.4375, 0.125],
        ];
        let mut expected = Vec::new();
        for index in 0..4817 {
            for sample in stereo[index % stereo.len()] {
                writer.write_sample(sample).unwrap();
            }
            expected.push([8192, -12288, 8192, -5120][index % stereo.len()]);
        }
        writer.finalize().unwrap();
        let mut actual = Vec::new();
        replay_chunks(
            WavChunkReader::open(file.path()).unwrap(),
            SessionId(41),
            true,
            &CancellationToken::new(),
            |_, _| Ok(()),
            |chunk| actual.push(chunk),
        )
        .unwrap();
        assert_eq!(actual.len(), 2);
        assert_eq!(decode_chunks(actual), expected);

        // An invalid float after an emitted frame must fail validation, not become clamped PCM.
        let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
        writer.write_sample(f32::NAN).unwrap();
        writer.write_sample(0.25_f32).unwrap();
        writer.finalize().unwrap();
        let bad = WavChunkReader::open(file.path()).unwrap().next().unwrap();
        let good = encode_i16_wav(&vec![-173; 4800], 24_000).unwrap();
        let mut actual = Vec::new();
        let error = replay_chunks(
            [Ok(good.clone()), bad],
            SessionId(41),
            true,
            &CancellationToken::new(),
            |_, _| Ok(()),
            |chunk| actual.push(chunk),
        )
        .unwrap_err();
        assert!(error.to_string().contains("non-finite"));
        assert_eq!(actual, [good]);
    }

    #[test]
    fn wav_replay_matches_live_pause_and_frame_chunking() {
        let wav = WavChunk::from_encoded_bytes(
            include_bytes!("../../../tests/fixtures/audio/speech.wav").to_vec(),
        );
        let (_, speech) = crate::audio::decode_mono(&wav).unwrap();
        for rate in [16_000, 44_100, 48_000] {
            let voice = crate::audio::PcmResampler::new(16_000, rate)
                .unwrap()
                .push(&speech, true)
                .unwrap();
            let mut pcm = voice.clone();
            pcm.extend(vec![0; rate as usize * 2]);
            pcm.extend_from_slice(&voice[..voice.len() / 2]);
            pcm.extend_from_slice(&[-31, 2903, -113]);
            for streaming in [false, true] {
                let mode = if streaming {
                    ChunkMode::Frames
                } else {
                    ChunkMode::Pauses
                };
                let mut live = ActiveRecording::new(SessionId(41), rate, mode).unwrap();
                let mut expected = Vec::new();
                for packet in pcm.chunks(317) {
                    live.shared.push_mono(packet);
                    while let Some(ready) = live.take_ready_chunk() {
                        assert_eq!(ready.session_id, SessionId(41));
                        expected.push(ready.chunk);
                    }
                }
                expected.extend(live.stop().unwrap());
                if !streaming {
                    assert_eq!(
                        expected.len(),
                        2,
                        "speech/pause/short speech must not be Fixed"
                    );
                }
                for packet_size in [997, rate as usize * 30] {
                    let actual = replay_packets(&pcm, rate, packet_size, streaming);
                    assert_eq!(actual, expected);
                    assert_eq!(decode_chunks(actual), pcm);
                }
            }
        }
    }

    #[test]
    fn wav_replay_keeps_one_session_across_io_blocks_and_stops_tail_once() {
        let rate = 16_000;
        let limit = rate as usize * 30;
        for length in [limit - 1, limit, limit + 1, limit * 2 + 317] {
            // Quiet asymmetric PCM must reach the hard boundary, including a one-sample tail.
            let pcm: Vec<i16> = (0..length).map(|n| (n % 17) as i16 - 8).collect();
            let file = tempfile::NamedTempFile::new().unwrap();
            let wav = encode_i16_wav(&pcm, rate).unwrap();
            std::fs::write(file.path(), wav.bytes()).unwrap();
            for streaming in [false, true] {
                let mut chunks = Vec::new();
                let mut offsets = Vec::new();
                replay_chunks(
                    WavChunkReader::open(file.path()).unwrap(),
                    SessionId(41),
                    streaming,
                    &CancellationToken::new(),
                    |offset, _| {
                        offsets.push(offset);
                        Ok(())
                    },
                    |chunk| chunks.push(chunk),
                )
                .unwrap();
                assert_eq!(offsets, replay_offsets(length, rate));
                let chunk_size = if streaming { rate as usize / 5 } else { limit };
                assert_eq!(chunks.len(), length.div_ceil(chunk_size));
                assert_eq!(decode_chunks(chunks), pcm);
            }
        }
    }

    #[test]
    fn wav_replay_propagates_decode_and_io_errors_after_emitted_audio() {
        let good = encode_i16_wav(&vec![-713; 3200], 16_000).unwrap();
        for bad in [
            Err(ChunkError::UnexpectedEndOfFile),
            Ok(WavChunk::from_encoded_bytes(b"not a WAV".to_vec())),
        ] {
            let mut chunks = Vec::new();
            let result = replay_chunks(
                [Ok(good.clone()), bad],
                SessionId(41),
                true,
                &CancellationToken::new(),
                |_, _| Ok(()),
                |chunk| chunks.push(chunk),
            );
            assert!(result.is_err());
            assert_eq!(chunks, std::slice::from_ref(&good));
        }

        // A physical truncated read in the second I/O block must not erase the first 30 seconds.
        let file = tempfile::NamedTempFile::new().unwrap();
        let pcm = vec![127; 16_000 * 31];
        let wav = encode_i16_wav(&pcm, 16_000).unwrap();
        std::fs::write(file.path(), &wav.bytes()[..wav.len() - 1]).unwrap();
        let mut chunks = Vec::new();
        assert!(
            replay_chunks(
                WavChunkReader::open(file.path()).unwrap(),
                SessionId(41),
                true,
                &CancellationToken::new(),
                |_, _| Ok(()),
                |chunk| chunks.push(chunk),
            )
            .is_err()
        );
        assert_eq!(decode_chunks(chunks), pcm[..16_000 * 30]);
    }

    #[test]
    fn wav_replay_cancellation_interrupts_the_cadence_without_flushing_tail() {
        let pcm = vec![291; 3200 * 2 + 317];
        let wav = encode_i16_wav(&pcm, 16_000).unwrap();
        let cancelled = CancellationToken::new();
        let mut waits = 0;
        let mut chunks = Vec::new();
        let result = replay_chunks(
            [Ok(wav)],
            SessionId(41),
            true,
            &cancelled,
            |_, token| {
                waits += 1;
                if waits == 2 {
                    token.cancel();
                }
                Ok(())
            },
            |chunk| chunks.push(chunk),
        );
        assert!(result.unwrap_err().to_string().contains("cancel"));
        assert_eq!(waits, 2);
        assert_eq!(decode_chunks(chunks), pcm[..3200]);

        // A caller can cancel while handling a frame; no additional frame or stop tail is sent.
        let cancelled = CancellationToken::new();
        let mut emitted = 0;
        assert!(
            replay_chunks(
                [encode_i16_wav(&pcm, 16_000)],
                SessionId(41),
                true,
                &cancelled,
                |_, _| Ok(()),
                |_| {
                    emitted += 1;
                    cancelled.cancel();
                },
            )
            .is_err()
        );
        assert_eq!(emitted, 1);
        assert!(
            replay_chunks(
                std::iter::from_fn(|| panic!("cancelled replay must not read input")),
                SessionId(41),
                true,
                &cancelled,
                |_, _| panic!("cancelled replay must not wait"),
                |_| panic!("cancelled replay must not emit audio"),
            )
            .is_err()
        );
    }
}
