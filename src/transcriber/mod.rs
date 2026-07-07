pub mod api;
pub mod factory;
#[cfg(test)]
pub use api::MockTranscriber;
pub use api::Transcriber;
pub use factory::create_transcriber;

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

/// Separator inserted between merged chunk texts for a given language.
///
/// Chinese text (zh, zh-CN, zh-TW) is concatenated without a separator;
/// all other languages use a single space.
pub fn merge_separator(language: Option<&str>) -> &'static str {
    match language {
        Some(lang) if lang.starts_with("zh") => "",
        _ => " ",
    }
}

/// Merge transcription results from multiple chunks, in order.
///
/// Empty fragments are dropped so a failed or silent chunk never produces
/// double separators.
pub fn merge_texts(texts: &[String], language: Option<&str>) -> String {
    let separator = merge_separator(language);
    texts
        .iter()
        .filter(|t| !t.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::merge_texts;

    #[test]
    fn test_merge_texts_zh_variants_concatenate_without_separator() {
        let texts = vec!["你好".to_string(), "世界".to_string()];
        assert_eq!(merge_texts(&texts, Some("zh")), "你好世界");
        assert_eq!(merge_texts(&texts, Some("zh-CN")), "你好世界");
        assert_eq!(merge_texts(&texts, Some("zh-TW")), "你好世界");
    }

    #[test]
    fn test_merge_texts_other_languages_use_space() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        assert_eq!(merge_texts(&texts, Some("en")), "hello world");
        assert_eq!(merge_texts(&texts, None), "hello world");
    }

    #[test]
    fn test_merge_texts_filters_empty_fragments() {
        let texts = vec!["a".to_string(), String::new(), "b".to_string()];
        assert_eq!(merge_texts(&texts, Some("en")), "a b");
        assert_eq!(merge_texts(&[String::new(), String::new()], Some("en")), "");
    }
}
