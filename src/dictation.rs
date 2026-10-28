//! Realtime audio dictation with locally owned context and memory.

pub(crate) const INPUT_LAYOUT: &str = "stable-prefix-audio-dynamic-v1";
pub(crate) const UNKNOWN_CONTEXT: &str = "未知语境";

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
mod offline;
pub(crate) use offline::OfflineDictation;
mod evaluation;
pub(crate) use evaluation::{MemorySeed, RealtimeEvaluator, Replay, SegmentTrace};
pub(crate) use memory::Snapshot;
pub(crate) use result::DictationResult;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;
