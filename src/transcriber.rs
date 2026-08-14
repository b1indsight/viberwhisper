pub mod api;
#[cfg(test)]
pub use api::MockTranscriber;
pub(crate) use api::TranscriberMetadata;
pub use api::{ApiTranscriber, Transcriber, TranscriberConfig};

use std::fmt;

/// Structured failure for a single transcription request.
///
/// Producers construct the variant directly instead of encoding the failure in
/// an error string, so downstream layers (retry logic, the session orchestrator)
/// can match on it without parsing messages.
#[derive(Debug, Clone)]
pub enum TranscribeError {
    /// The API returned an HTTP error response (4xx or 5xx).
    Api { status: u16, body: String },
    /// A network, I/O, or response-parsing error.
    Network(String),
    /// The chunk did not reach a terminal state before the convergence timeout.
    /// Produced by the session orchestrator, never by a transcriber itself.
    Timeout,
}

impl fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TranscribeError::Api { status, body } => {
                write!(f, "API error {}: {}", status, body)
            }
            TranscribeError::Network(msg) => write!(f, "Network error: {}", msg),
            TranscribeError::Timeout => write!(f, "Convergence timeout"),
        }
    }
}

impl std::error::Error for TranscribeError {}
