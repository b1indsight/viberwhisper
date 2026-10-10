use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// File-only prompt rules, composed in task order without interpolation.
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
}

impl Default for PromptComponents {
    fn default() -> Self {
        serde_json::from_str(include_str!("../../assets/dictation-prompt.default.json"))
            .expect("bundled dictation prompt components must be valid")
    }
}

impl PromptComponents {
    /// Join nonempty rules in a fixed order; JSON and template-looking text remain literal.
    pub fn compose(&self) -> String {
        [
            &self.task,
            &self.language,
            &self.numbers,
            &self.cleanup,
            &self.output_format,
            &self.context,
            &self.memory,
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
    /// Full-string override: None selects components; an empty string disables instructions.
    pub prompt: Option<String>,
    /// Complete component object; mutually exclusive with the full-string override.
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
            url: "ws://192.168.50.156:8080/v1/realtime".into(),
            model: "mlx-community/gemma-4-e4b-it-4bit".into(),
            api_key_env: "REALTIME_API_KEY".into(),
            prompt: None,
            prompt_components: None,
            memory: MemoryConfig::default(),
        }
    }
}

impl DictationConfig {
    pub fn prompt(&self) -> String {
        if let Some(prompt) = &self.prompt {
            return prompt.clone();
        }
        self.prompt_components
            .as_ref()
            .map(PromptComponents::compose)
            .unwrap_or_else(|| PromptComponents::default().compose())
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
            self.prompt.is_none() || self.prompt_components.is_none(),
            "cannot set both dictation.prompt and dictation.prompt_components; set prompt to null to use components"
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
        ensure!(
            self.memory.long_term.len() <= 512,
            "at most 512 long-term memory entries"
        );
        for term in &self.memory.long_term {
            ensure!(
                valid_label(&term.context) && valid_term(&term.text),
                "invalid long-term memory entry"
            );
        }
        Ok(())
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

pub(super) fn valid_label(text: &str) -> bool {
    !text.trim().is_empty() && text.chars().count() <= 64 && !text.chars().any(char::is_control)
}

pub(super) fn valid_term(text: &str) -> bool {
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
    fn legacy_full_prompt_and_empty_override_remain_exact() {
        for text in ["  自定义全文\n", ""] {
            let config: DictationConfig = serde_json::from_value(json!({"prompt": text})).unwrap();
            assert!(config.prompt_components.is_none());
            config.validate().unwrap();
            assert_eq!(config.prompt(), text);
        }
        let config: DictationConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.prompt(), PromptComponents::default().compose());
        config.validate().unwrap();
    }

    #[test]
    fn competing_prompt_sources_fail_instead_of_hiding_component_edits() {
        let config = DictationConfig {
            prompt: Some(String::new()),
            prompt_components: Some(components()),
            ..Default::default()
        };
        assert!(config.validate().unwrap_err().to_string().contains("both"));
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
    fn composed_prompt_limit_counts_unicode_characters_and_separators() {
        let mut rules: PromptComponents = serde_json::from_value(json!({
            "task":"", "language":"", "numbers":"", "cleanup":"",
            "output_format":"", "context":"", "memory":""
        }))
        .unwrap();
        rules.task = "字".repeat(4096);
        let mut config = DictationConfig {
            prompt_components: Some(rules),
            ..Default::default()
        };
        config.validate().unwrap();
        // A second component adds both its contents and a separator to the request limit.
        config.prompt_components.as_mut().unwrap().language = "中".into();
        assert!(config.validate().unwrap_err().to_string().contains("4096"));
    }

    #[test]
    fn offline_metadata_uses_the_same_composed_instructions() {
        let config = DictationConfig {
            prompt_components: Some(components()),
            ..Default::default()
        };
        let expected = config.prompt();
        let transcriber = crate::dictation::OfflineDictation::new(config, None, None).unwrap();
        assert_eq!(transcriber.metadata().prompt, Some(expected));
    }
}
