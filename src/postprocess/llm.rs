use crate::core::config::ApiAuth;
use crate::dictation::{
    DictationResult, LearningWorker, Snapshot, Transcript, config::PromptComponents,
};
use crate::postprocess::{LlmConfig, PostProcessError};
use reqwest::blocking::Client;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use tracing::info;

const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Configured LLM cleanup state owned by [`crate::postprocess::PostProcessor::Llm`].
pub struct LlmPostProcessor {
    request: LlmRequest,
    preheat_enabled: bool,
    pub(super) learning: Option<LearningWorker>,
}

/// Immutable request state; each recording captures one bounded context snapshot.
#[derive(Clone)]
struct LlmRequest {
    auth: ApiAuth,
    api_url: reqwest::Url,
    model: String,
    prompt: String,
    temperature: f32,
    client: Client,
    snapshot: Snapshot,
}

impl LlmPostProcessor {
    pub(crate) fn new(config: LlmConfig) -> Result<Self, reqwest::Error> {
        Ok(Self {
            request: LlmRequest {
                auth: config.auth,
                api_url: config.endpoint,
                model: config.model,
                prompt: PromptComponents::postprocess_prompt(config.prompt.as_deref()),
                temperature: config.temperature,
                client: Client::builder().timeout(LLM_REQUEST_TIMEOUT).build()?,
                snapshot: Snapshot::default(),
            },
            preheat_enabled: config.preheat_enabled,
            learning: None,
        })
    }

    pub(super) fn process_text(&self, text: &str) -> Result<String, PostProcessError> {
        if text.is_empty() {
            return Ok(text.to_string());
        }
        info!(text_len = text.len(), "Post-processing text with LLM");
        // CLI/setup deliberately use the empty snapshot even for a desktop-configured processor.
        let result = self.request.run(text)?;
        info!(
            result_len = result.text.len(),
            "LLM post-processing complete"
        );
        Ok(result.text)
    }

    pub(super) fn process_for_delivery(&self, text: &str) -> Result<Transcript, PostProcessError> {
        if text.is_empty() {
            return Ok(String::new().into());
        }
        let snapshot = self
            .learning
            .as_ref()
            .map(LearningWorker::snapshot)
            .transpose()
            .map_err(PostProcessError::Memory)?
            .unwrap_or_default();
        let mut session = self.session(snapshot);
        session.push_stable_chunk(text);
        let result = session.finish()?.ok_or(PostProcessError::EmptyContent)?;
        match &self.learning {
            Some(learning) => learning.prepare(result).map_err(PostProcessError::Memory),
            None => Ok(result.text.into()),
        }
    }

    fn session(&self, snapshot: Snapshot) -> LlmSession {
        let request = LlmRequest {
            snapshot,
            ..self.request.clone()
        };
        if self.preheat_enabled {
            LlmSession::Preheat(PreheatLlmSession::new(request))
        } else {
            LlmSession::Conservative(ConservativeLlmSession {
                request,
                chunks: Vec::new(),
            })
        }
    }
}

enum LlmSession {
    Conservative(ConservativeLlmSession),
    Preheat(PreheatLlmSession),
}

impl LlmSession {
    fn push_stable_chunk(&mut self, text: &str) {
        match self {
            Self::Conservative(session) => session.push_stable_chunk(text),
            Self::Preheat(session) => session.push_stable_chunk(text),
        }
    }

    fn finish(&mut self) -> Result<Option<DictationResult>, PostProcessError> {
        match self {
            Self::Conservative(session) => session.finish(),
            Self::Preheat(session) => session.finish(),
        }
    }
}

impl LlmRequest {
    fn run(&self, text: &str) -> Result<DictationResult, PostProcessError> {
        let request_body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.prompt},
                {"role": "user", "content": serde_json::json!({"text":text, "contexts":self.snapshot.contexts}).to_string()}
            ],
            "temperature": self.temperature,
            "stream": false
        });

        let mut request = self
            .client
            .post(self.api_url.clone())
            .header("Content-Type", "application/json")
            .json(&request_body);
        if let ApiAuth::Bearer(secret) = &self.auth {
            request = request.bearer_auth(secret.expose());
        }
        let response = request.send()?;

        let status = response.status();
        if !status.is_success() {
            return Err(PostProcessError::Api {
                status: status.as_u16(),
            });
        }

        let body = response.text()?;
        let json: serde_json::Value = serde_json::from_str(&body)?;
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .ok_or(PostProcessError::MissingContent)?
            .trim()
            .to_string();

        if content.is_empty() {
            return Err(PostProcessError::EmptyContent);
        }

        // Schema errors may quote personal content; keep only the safe error category.
        let result = DictationResult::parse_transcript(&content, &self.snapshot.ids())
            .map_err(|_| PostProcessError::InvalidResult)?;
        if result.text.trim().is_empty() {
            return Err(PostProcessError::EmptyContent);
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Conservative session (streaming_enabled = false): accumulate, call once.
// ---------------------------------------------------------------------------

/// Accumulated text and request configuration for conservative cleanup.
struct ConservativeLlmSession {
    request: LlmRequest,
    chunks: Vec<String>,
}

impl ConservativeLlmSession {
    pub(super) fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
        }
    }

    fn finish(&mut self) -> Result<Option<DictationResult>, PostProcessError> {
        // Post-processing receives already ordered stable text fragments; joining
        // without separators preserves the STT layer's chosen spacing/punctuation.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(None);
        }
        self.request.run(&combined).map(Some)
    }
}

// ---------------------------------------------------------------------------
// Preheat session (streaming_enabled = true): fire LLM on every chunk arrival.
// ---------------------------------------------------------------------------

/// Shared state between the session and its background LLM threads.
struct PreheatState {
    /// The generation counter for the latest request.
    latest_generation: u64,
    /// Result from the latest completed request whose generation matches `latest_generation`.
    latest_result: Option<Result<DictationResult, PostProcessError>>,
}

/// Accumulated text and background request state for preheated cleanup.
struct PreheatLlmSession {
    request: LlmRequest,
    chunks: Vec<String>,
    generation: u64,
    state: Arc<(Mutex<PreheatState>, Condvar)>,
}

impl PreheatLlmSession {
    fn new(request: LlmRequest) -> Self {
        Self {
            request,
            chunks: Vec::new(),
            generation: 0,
            state: Arc::new((
                Mutex::new(PreheatState {
                    latest_generation: 0,
                    latest_result: None,
                }),
                Condvar::new(),
            )),
        }
    }

    fn fire_request(&mut self) {
        // Post-processing accumulates stable text exactly as received; it does
        // not add separators because upstream text already owns word spacing.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return;
        }

        self.generation += 1;
        let request_gen = self.generation;

        // Update latest_generation so older threads know they're stale.
        {
            let mut st = self.state.0.lock().unwrap();
            st.latest_generation = request_gen;
            st.latest_result = None; // clear stale result
        }

        let request = self.request.clone();
        let state = Arc::clone(&self.state);

        thread::spawn(move || {
            let result = request.run(&combined);

            let (lock, cvar) = &*state;
            let mut st = lock.lock().unwrap();
            // Only store result if this thread's generation is still the latest.
            if st.latest_generation == request_gen {
                st.latest_result = Some(result);
                cvar.notify_all();
            }
            // Otherwise this result is stale — silently drop it.
        });
    }

    pub(super) fn push_stable_chunk(&mut self, text: &str) {
        if !text.is_empty() {
            self.chunks.push(text.to_string());
            info!(
                generation = self.generation + 1,
                text_len = self.chunks.iter().map(|c| c.len()).sum::<usize>(),
                "Preheat: firing LLM request for accumulated text"
            );
            self.fire_request();
        }
    }

    fn finish(&mut self) -> Result<Option<DictationResult>, PostProcessError> {
        // Keep post-processing concatenation separator-free; the STT result is
        // already a display-ready text stream before LLM cleanup.
        let combined = self.chunks.join("");
        if combined.is_empty() {
            return Ok(None);
        }

        // If no request was ever fired (shouldn't happen if push_stable_chunk was called
        // with non-empty text, but be safe), fire one now.
        if self.generation == 0 {
            return self.request.run(&combined).map(Some);
        }

        // Wait for the latest generation's result.
        let (lock, cvar) = &*self.state;
        let mut st = lock.lock().unwrap();
        while st.latest_result.is_none() {
            st = cvar.wait(st).unwrap();
        }

        let result = st.latest_result.take().unwrap();
        match result {
            Ok(result) => {
                info!(
                    result_len = result.text.len(),
                    "Preheat: LLM post-processing complete"
                );
                Ok(Some(result))
            }
            Err(e) => {
                // Preheat request failed — retry once with full text as fallback.
                info!(error = %e, "Preheat: last request failed, retrying with full text");
                drop(st);
                self.request.run(&combined).map(Some)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{ApiAuth, SecretValue};
    use crate::postprocess::{PostProcessConfig, PostProcessor, test_support::stub};
    use serde_json::json;

    fn config_with_postprocess(preheat_enabled: bool, prompt: Option<&str>) -> LlmConfig {
        LlmConfig {
            endpoint: reqwest::Url::parse("https://api.example.com/v1/chat/completions").unwrap(),
            auth: ApiAuth::Bearer(SecretValue::new("test_key")),
            model: "gpt-4o-mini".to_string(),
            preheat_enabled,
            prompt: prompt.map(str::to_string),
            temperature: 0.0,
        }
    }

    #[test]
    fn authorization_header_matches_typed_auth_mode() {
        for (auth, expected_header) in [
            (ApiAuth::None, None),
            (
                ApiAuth::Bearer(SecretValue::new("header-token")),
                Some("authorization: bearer header-token"),
            ),
        ] {
            let (endpoint, requests) = stub(vec![json!({"text":"clean"}).to_string()]);
            let mut config = config_with_postprocess(false, None);
            config.endpoint = endpoint;
            config.auth = auth;
            let processor = PostProcessor::new(PostProcessConfig::Llm(config));
            assert_eq!(processor.process_text("raw").unwrap(), "clean");
            let headers = requests.recv().unwrap().headers.to_ascii_lowercase();
            match expected_header {
                Some(expected) => assert!(headers.contains(expected)),
                None => assert!(!headers.contains("authorization:")),
            }
        }
    }

    #[test]
    fn configures_prompt_and_session_mode() {
        let p = LlmPostProcessor::new(config_with_postprocess(true, None)).unwrap();
        assert_eq!(p.request.model, "gpt-4o-mini");
        assert_eq!(
            p.request.api_url.as_str(),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(p.request.prompt, PromptComponents::postprocess_prompt(None));
        assert!(p.preheat_enabled);

        let custom =
            LlmPostProcessor::new(config_with_postprocess(false, Some("custom prompt"))).unwrap();
        assert_eq!(
            custom.request.prompt,
            PromptComponents::postprocess_prompt(Some("custom prompt"))
        );
        assert!(!custom.preheat_enabled);
    }

    #[test]
    fn configured_processor_cleans_complete_text() {
        // Convert and setup verification use the configured processor directly;
        // an accidental pass-through here would silently skip enabled cleanup.
        let (endpoint, _requests) = stub(vec![json!({"text":"clean"}).to_string()]);
        let mut config = config_with_postprocess(true, None);
        config.endpoint = endpoint;
        let processor = PostProcessor::new(PostProcessConfig::Llm(config));

        assert_eq!(processor.process_text("raw").unwrap(), "clean");
    }

    #[test]
    fn aggregate_cleanup_preserves_long_text_for_cli_and_desktop() {
        use crate::dictation::config::MemoryConfig;

        // HTTP cleanup receives a whole recording, not a Realtime segment.
        // Cover both the 16 KiB text and 64 KiB JSON limits, with memory on and off.
        for characters in [6_000, 24_000] {
            let prefix = "长".repeat(characters);
            let raw = format!("{prefix}未整理");
            let cleaned = format!("{prefix}整理完成。");
            for preheat in [false, true] {
                for enabled in [false, true] {
                    let response = json!({"text":cleaned}).to_string();
                    let (endpoint, _requests) = stub(vec![response.clone(), response]);
                    let mut config = config_with_postprocess(preheat, None);
                    config.endpoint = endpoint;
                    let processor = PostProcessor::new_desktop(
                        PostProcessConfig::Llm(config),
                        MemoryConfig {
                            enabled,
                            ..MemoryConfig::default()
                        },
                        None,
                        Some("zh".into()),
                    )
                    .unwrap();

                    assert_eq!(processor.process_text(&raw).unwrap(), cleaned);
                    let transcript = processor.process_for_delivery(&raw).unwrap();
                    assert_eq!(transcript.text, cleaned);
                    assert_eq!(transcript.confirmation.is_some(), enabled);
                    drop(transcript);
                    processor.shutdown();
                }
            }
        }
    }

    #[test]
    fn test_process_text_empty_input_bypasses_llm() {
        let config = config_with_postprocess(true, None);
        let p = LlmPostProcessor::new(config).unwrap();
        let result = p.process_text("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn session_processes_text_in_both_modes() {
        // Desktop finalization must reach the LLM through either enum variant;
        // empty-input tests bypass the request and cannot verify this path.
        for preheat_enabled in [false, true] {
            let (endpoint, _requests) = stub(vec![json!({"text":"clean"}).to_string()]);
            let mut config = config_with_postprocess(preheat_enabled, None);
            config.endpoint = endpoint;
            let processor = PostProcessor::new(PostProcessConfig::Llm(config));
            assert_eq!(
                processor.process_for_delivery("raw").unwrap().text,
                "clean",
                "preheat_enabled={preheat_enabled}"
            );
        }
    }

    #[test]
    fn empty_session_finishes_empty_in_both_modes() {
        for preheat_enabled in [false, true] {
            let config = config_with_postprocess(preheat_enabled, None);
            let p = LlmPostProcessor::new(config).unwrap();
            assert_eq!(
                p.process_for_delivery("").unwrap().text,
                "",
                "preheat_enabled={preheat_enabled}"
            );
        }
    }

    fn structured(text: &str, context: serde_json::Value) -> String {
        serde_json::json!({
            "text": text, "context": context,
            "memory_candidates": [{"kind":"proper_noun", "text":"维伯耳语", "quote":text}]
        })
        .to_string()
    }

    #[test]
    fn desktop_confirmation_repairs_next_result_after_restart_in_both_modes() {
        use crate::dictation::config::MemoryConfig;
        use serde_json::json;
        // A repeated model misspelling must be repaired from confirmed feedback,
        // including after restart; merely prompting the model is insufficient.
        for preheat in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dictation-memory.json");
            let wrong = "请打开维伯耳语。";
            let (endpoint, requests) = stub(vec![
                structured(wrong, json!({"kind":"new", "label":"项目"})),
                structured(wrong, json!({"kind":"existing", "id":"项目"})),
                structured(wrong, json!({"kind":"existing", "id":"项目"})),
            ]);
            let build = || {
                let mut config = config_with_postprocess(preheat, None);
                config.endpoint = endpoint.clone();
                PostProcessor::new_desktop(
                    PostProcessConfig::Llm(config),
                    MemoryConfig::default(),
                    Some(path.clone()),
                    Some("zh".into()),
                )
                .unwrap()
            };
            let processor = build();
            let first = processor.process_for_delivery(wrong).unwrap();
            assert_eq!(first.text, wrong);
            assert!(!path.exists(), "unconfirmed output must not be saved");
            first
                .confirmation
                .unwrap()
                .confirm("请打开ViberWhisper。".into())
                .unwrap();
            processor.shutdown(); // Accepted feedback must be drained on exit.
            let stored: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(stored["schema_version"], 2);
            assert_eq!(
                stored["corrections"][0]["rule"],
                json!({"from":"维伯耳语","to":"ViberWhisper"})
            );

            let processor = build();
            let second = processor.process_for_delivery(wrong).unwrap();
            assert_eq!(second.text, "请打开ViberWhisper。");
            second.confirmation.unwrap().confirm(wrong.into()).unwrap();
            let third = processor.process_for_delivery(wrong).unwrap();
            assert_eq!(
                third.text, wrong,
                "undo must revoke the automatic correction before the next request"
            );
            drop(third); // Cancellation must release the worker without learning.
            processor.shutdown();

            let sent: Vec<_> = (0..3)
                .map(|_| requests.recv_timeout(Duration::from_secs(5)).unwrap().body)
                .collect();
            let input = |at: usize| {
                serde_json::from_str::<serde_json::Value>(
                    sent[at]["messages"][1]["content"].as_str().unwrap(),
                )
                .unwrap()
            };
            assert_eq!(input(0)["contexts"], json!([]));
            assert_eq!(
                input(1)["contexts"][0]["corrections"][0],
                json!({"from":"维伯耳语","to":"ViberWhisper"})
            );
            assert!(input(2)["contexts"][0].get("corrections").is_none());
            assert_eq!(input(1)["text"], wrong);
            assert_eq!(sent[0]["stream"], false);
            assert!(
                sent[0]["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("明确改口时仅用最终值替换旧值")
            );
        }
    }

    #[test]
    fn desktop_disabled_memory_and_invalid_results_cannot_learn() {
        use crate::dictation::config::MemoryConfig;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.json");
        std::fs::write(&path, "do not read or overwrite").unwrap();
        // A disabled memory setting must not touch even an invalid existing file.
        let (endpoint, requests) = stub(vec![
            structured("维伯耳语", json!({"kind":"new","label":"项目"})),
            "not JSON".into(),
            json!("private transcript must not be logged").to_string(),
        ]);
        let mut config = config_with_postprocess(false, Some("保留换行"));
        config.endpoint = endpoint;
        let processor = PostProcessor::new_desktop(
            PostProcessConfig::Llm(config),
            MemoryConfig {
                enabled: false,
                long_term: vec![],
            },
            Some(path.clone()),
            None,
        )
        .unwrap();
        let result = processor.process_for_delivery("维伯耳语").unwrap();
        assert_eq!(result.text, "维伯耳语");
        assert!(result.confirmation.is_none());
        for _ in 0..2 {
            let error = processor.process_for_delivery("raw").unwrap_err();
            assert!(matches!(error, PostProcessError::InvalidResult));
            assert!(!format!("{error}; {error:?}").contains("private transcript"));
        }
        processor.shutdown();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "do not read or overwrite"
        );
        for _ in 0..3 {
            let request = requests.recv_timeout(Duration::from_secs(5)).unwrap().body;
            let input: serde_json::Value =
                serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(input["contexts"], json!([]));
            let prompt = request["messages"][0]["content"].as_str().unwrap();
            assert!(prompt.contains("保留换行"));
            assert!(prompt.contains("memory_candidates"));
        }
    }
}
