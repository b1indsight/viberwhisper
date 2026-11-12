//! Realtime audio dictation with locally owned context and memory.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const INPUT_LAYOUT: &str = "stable-prefix-audio-dynamic-v1";
pub(crate) const TRANSCRIPTION_INPUT_LAYOUT: &str = "audio-only-v1";
pub(crate) const ASR_PROMPT: &str =
    "You are a speech recognition model.\n请将这段中文语音转换为纯文本，去掉标点符号。";
pub(crate) const UNKNOWN_CONTEXT: &str = "未知语境";

/// A connection's fixed input and output protocol; never stored in configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RealtimeMode {
    Integrated,
    Transcription,
}

/// Unix-seconds memory clock. Network deadlines continue to use monotonic time.
pub(crate) type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Use wall-clock Unix seconds for ordinary production memory retention.
pub(crate) fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })
}

pub(crate) mod config;
mod correction;
mod engine;
mod feedback;
mod learning;
mod memory;
mod progress;
mod result;
mod text_stream;
mod transport;
pub(crate) use engine::{DictationWorker, RealtimeSession};
pub(crate) use feedback::{Confirmation, Transcript};
pub(crate) use learning::LearningWorker;
pub(crate) use progress::Progress;
mod evaluation;
pub(crate) use evaluation::{MemorySeed, Replay, SegmentTrace};
pub(crate) use memory::Snapshot;
pub(crate) use result::DictationResult;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;
