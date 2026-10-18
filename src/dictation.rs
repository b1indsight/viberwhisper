//! Realtime audio dictation with locally owned context and memory.

pub(crate) const INPUT_LAYOUT: &str = "stable-prefix-audio-dynamic-v1";

pub(crate) mod config;
mod correction;
mod engine;
mod feedback;
mod memory;
mod result;
mod transport;
pub(crate) use engine::{DictationWorker, RealtimeSession};
pub(crate) use feedback::{Confirmation, Transcript};
mod offline;
pub(crate) use offline::OfflineDictation;
mod evaluation;
pub(crate) use evaluation::{MemorySeed, RealtimeEvaluator, Replay, SegmentTrace};
pub(crate) use memory::Snapshot;
pub(crate) use result::{ContextChoice, DictationResult};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;
