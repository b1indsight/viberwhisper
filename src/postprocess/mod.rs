pub mod factory;
pub mod llm;

pub use factory::create_post_processor;
pub use llm::LlmPostProcessor;

/// Post-processes STT text (e.g., via LLM cleanup).
///
/// - `process` handles a complete text string (used by the `convert` path).
/// - `start_session` creates an incremental session (used by the `run_listener` path).
pub trait TextPostProcessor: Send + Sync {
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>>;
    fn start_session(&self) -> Box<dyn TextPostProcessorSession>;
}

/// Incremental post-processing session.
///
/// Stable STT text chunks are pushed via `push_stable_chunk`; `finish` returns the
/// final processed text once the recording session is complete.
pub trait TextPostProcessorSession: Send {
    fn push_stable_chunk(&mut self, text: &str);
    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>>;
}

/// No-op post-processor: passes text through unchanged.
pub struct NoopPostProcessor;

impl TextPostProcessor for NoopPostProcessor {
    fn process(&self, text: &str) -> Result<String, Box<dyn std::error::Error>> {
        Ok(text.to_string())
    }

    fn start_session(&self) -> Box<dyn TextPostProcessorSession> {
        Box::new(NoopSession { chunks: Vec::new() })
    }
}

struct NoopSession {
    chunks: Vec<String>,
}

impl TextPostProcessorSession for NoopSession {
    fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(self.chunks.join(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_process() {
        let p = NoopPostProcessor;
        assert_eq!(p.process("hello").unwrap(), "hello");
        assert_eq!(p.process("").unwrap(), "");
    }

    #[test]
    fn test_noop_session() {
        let p = NoopPostProcessor;
        let mut session = p.start_session();
        session.push_stable_chunk("hello");
        session.push_stable_chunk("world");
        assert_eq!(session.finish().unwrap(), "helloworld");
    }

    #[test]
    fn test_noop_session_empty_chunks_filtered() {
        let p = NoopPostProcessor;
        let mut session = p.start_session();
        session.push_stable_chunk("");
        session.push_stable_chunk("hello");
        assert_eq!(session.finish().unwrap(), "hello");
    }

    #[test]
    fn test_noop_session_no_chunks() {
        let p = NoopPostProcessor;
        let mut session = p.start_session();
        assert_eq!(session.finish().unwrap(), "");
    }
}
