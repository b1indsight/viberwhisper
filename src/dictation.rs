//! Realtime audio dictation with locally owned context and memory.

pub(crate) mod config;
mod engine;
mod memory;
mod result;
mod transport;
pub(crate) use engine::{DictationWorker, RealtimeSession};
mod offline;
pub(crate) use offline::OfflineDictation;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;
