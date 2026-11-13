use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Saved instruction components, composed without defaults or interpolation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PromptComponents {
    task: String,
    language: String,
    numbers: String,
    cleanup: String,
    output_format: String,
    context: String,
    memory: String,
    /// Short segments omit only this separately owned component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    examples: Option<String>,
}

impl PromptComponents {
    /// Join nonempty rules in a fixed order; JSON and template-looking text remain literal.
    pub fn compose(&self) -> String {
        self.compose_for_segment(false)
    }

    /// Short segments omit examples; every retained component stays literal.
    pub fn compose_for_segment(&self, short: bool) -> String {
        [
            self.task.as_str(),
            self.language.as_str(),
            self.numbers.as_str(),
            self.cleanup.as_str(),
            self.output_format.as_str(),
            self.context.as_str(),
            self.memory.as_str(),
            if short {
                ""
            } else {
                self.examples.as_deref().unwrap_or_default()
            },
        ]
        .into_iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// File-only settings. Authentication is resolved from an environment variable, never logged.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DictationConfig {
    pub enabled: bool,
    pub url: String,
    pub model: String,
    pub api_key_env: String,
    /// Literal instructions for isolated lab candidates; exclusive with components.
    pub prompt: Option<String>,
    /// Read only for upgrading the retired full short-prompt representation.
    #[serde(rename = "short_prompt", skip_serializing)]
    pub legacy_short_prompt: Option<String>,
    /// Saved normal instructions; short segments omit the examples component.
    pub prompt_components: Option<PromptComponents>,
    pub memory: MemoryConfig,
}

impl std::fmt::Debug for DictationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DictationConfig")
            .field("enabled", &self.enabled)
            .field("memory_enabled", &self.memory.enabled)
            .field("long_term_entries", &self.memory.long_term.len())
            .finish_non_exhaustive()
    }
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "ws://127.0.0.1:8080/v1/realtime".into(),
            model: "mlx-community/gemma-4-e4b-it-4bit".into(),
            api_key_env: "REALTIME_API_KEY".into(),
            prompt: None,
            legacy_short_prompt: None,
            prompt_components: None,
            memory: MemoryConfig::default(),
        }
    }
}

impl DictationConfig {
    /// Compose saved components, or return the validated literal lab candidate.
    pub fn prompt(&self) -> String {
        match &self.prompt_components {
            Some(components) => components.compose(),
            None => self
                .prompt
                .clone()
                .expect("validated dictation prompt source"),
        }
    }

    /// Only component-backed instructions have a shorter variant.
    pub fn short_prompt(&self) -> Option<String> {
        self.prompt_components
            .as_ref()
            .map(|components| components.compose_for_segment(true))
    }

    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.url)?;
        ensure!(
            matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some(),
            "dictation.url must be a ws:// or wss:// endpoint"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "dictation.url must not contain credentials, query or fragment"
        );
        ensure!(
            !self.model.trim().is_empty() && self.model.len() <= 256,
            "invalid dictation.model"
        );
        ensure!(
            self.legacy_short_prompt.is_none(),
            "dictation.short_prompt is retired; prepare the configuration before recognition"
        );
        ensure!(
            self.prompt.is_some() || self.prompt_components.is_some(),
            "dictation prompt components are missing; prepare the configuration before recognition"
        );
        ensure!(
            self.prompt.is_none() || self.prompt_components.is_none(),
            "dictation.prompt and dictation.prompt_components are mutually exclusive"
        );
        ensure!(
            self.prompt().chars().count() <= 4096,
            "dictation prompt exceeds 4096 characters"
        );
        ensure!(
            self.api_key_env
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "invalid dictation.api_key_env"
        );
        self.memory.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MemoryConfig {
    pub enabled: bool,
    pub long_term: Vec<LongTerm>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            long_term: Vec::new(),
        }
    }
}

impl MemoryConfig {
    /// Validate vocabulary independently of the selected recognition transport.
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.long_term.len() <= 512,
            "at most 512 long-term memory entries"
        );
        for term in &self.long_term {
            ensure!(
                term.context != super::UNKNOWN_CONTEXT,
                "long-term context \"{}\" is reserved for unknown context; rename it to a real topic",
                super::UNKNOWN_CONTEXT
            );
            ensure!(
                valid_label(&term.context) && valid_term(&term.text),
                "invalid long-term memory entry"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TermKind {
    ProperNoun,
    Phrase,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LongTerm {
    pub context: String,
    pub kind: TermKind,
    pub text: String,
}

pub(crate) fn valid_label(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= 64 && !text.chars().any(char::is_control)
}

pub(crate) fn valid_term(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= 80 && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use serde_json::json;

    fn components() -> PromptComponents {
        serde_json::from_value(json!({
            "memory": "  记忆  ",
            "context": "语境",
            "output_format": "{\"text\":\"原句\"}",
            "cleanup": "清理",
            "numbers": "\n",
            "language": "语言",
            "task": "  任务 {audio} \\path  "
        }))
        .unwrap()
    }

    #[test]
    fn prompt_components_preserve_literals_and_use_fixed_order() {
        // Users edit JSON rules independently; input key order and template-looking text must not alter composition.
        assert_eq!(
            components().compose(),
            "任务 {audio} \\path\n语言\n清理\n{\"text\":\"原句\"}\n语境\n记忆"
        );
    }

    #[test]
    fn short_segments_omit_only_examples_and_keep_other_components_literal() {
        // A short recording must omit examples without injecting another instruction
        // or deleting example-looking text from a saved rule.
        let mut legacy = components();
        legacy.memory = "记忆\n口述：自定义旧示例\n回复：保持原文".into();
        assert_eq!(legacy.compose_for_segment(true), legacy.compose());
        for examples in ["  自定义独立示例  ", ""] {
            let mut rules = legacy.clone();
            rules.examples = Some(examples.into());
            assert_eq!(rules.compose_for_segment(true), legacy.compose());
            assert_eq!(rules.compose_for_segment(false), rules.compose());
            let normal = if examples.is_empty() {
                legacy.compose()
            } else {
                format!("{}\n{}", legacy.compose(), examples.trim())
            };
            assert_eq!(rules.compose(), normal);
        }
    }

    #[test]
    fn literal_candidates_remain_exact_and_legacy_short_prompts_require_preparation() {
        for text in ["  自定义全文\n", ""] {
            let mut config: DictationConfig =
                serde_json::from_value(json!({"prompt": text})).unwrap();
            assert!(config.prompt_components.is_none());
            config.validate().unwrap();
            assert_eq!(config.prompt(), text);
            assert!(config.short_prompt().is_none());
            config.legacy_short_prompt = Some(text.into());
            assert!(
                config
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains("prepare")
            );
        }
        let config: DictationConfig = serde_json::from_str("{}").unwrap();
        assert!(config.prompt.is_none());
        assert!(config.short_prompt().is_none());
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("prepare")
        );
    }

    #[test]
    fn saved_components_supply_instructions_and_conflicting_sources_are_rejected() {
        let mut config = DictationConfig {
            prompt_components: Some(components()),
            ..Default::default()
        };
        config.validate().unwrap();
        assert_eq!(
            config.prompt(),
            "任务 {audio} \\path\n语言\n清理\n{\"text\":\"原句\"}\n语境\n记忆"
        );
        config.prompt = Some(String::new());
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("mutually exclusive")
        );
    }

    #[test]
    fn component_schema_rejects_typos_missing_rules_and_non_strings() {
        let valid = serde_json::to_value(components()).unwrap();
        let mut typo = valid.clone();
        typo["numbrs"] = json!("规则");
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("memory");
        let mut wrong_type = valid;
        wrong_type["numbers"] = json!(42);
        for invalid in [typo, missing, wrong_type] {
            assert!(serde_json::from_value::<PromptComponents>(invalid).is_err());
        }
    }

    #[test]
    fn saved_prompt_limits_count_unicode_characters() {
        for source in ["literal", "components"] {
            for length in [4096, 4097] {
                let text = "字".repeat(length);
                let value = if source == "literal" {
                    json!({"prompt": text})
                } else {
                    json!({"prompt_components": {
                        "task": text, "language": "", "numbers": "", "cleanup": "",
                        "output_format": "", "context": "", "memory": ""
                    }})
                };
                let config: DictationConfig = serde_json::from_value(value).unwrap();
                assert_eq!(
                    config.validate().is_ok(),
                    length == 4096,
                    "{source}: {length}"
                );
            }
        }
    }

    #[test]
    fn offline_metadata_records_the_saved_component_composition() {
        let config = DictationConfig {
            prompt_components: Some(components()),
            ..Default::default()
        };
        let transcriber = crate::dictation::OfflineDictation::new(config, None, None).unwrap();
        assert_eq!(
            transcriber.metadata().prompt.as_deref(),
            Some("任务 {audio} \\path\n语言\n清理\n{\"text\":\"原句\"}\n语境\n记忆")
        );
    }
}
