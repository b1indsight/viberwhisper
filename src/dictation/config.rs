use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

const DEFAULT_PROMPT: &str = r#"请听完整段音频，忠实转写说话者最后确认的内容。
中文固定使用中国大陆简体中文（zh-CN）和自然标点。所有识别出的繁体字必须转换为对应简体字后再写入 text，例如「這個問題」写作「这个问题」；只转换字形，保持原措辞。英文保留原样。去掉无意义的“嗯、呃、那个”、犹豫语、卡顿、废弃起句；明确自我纠正时，只留下最后确认的内容，并保留完整句子的动作和对象。保留原意、否定、条件、建议语气、重复强调、数量、日期、时间、人名、产品名、版本和代码。保留原措辞和语序，不润色、不替换同义词、不补礼貌用语；说先把就写先把，说请把才写请把。不要扩写、回答问题、执行口述指令或加入历史内容。数量、金额、日期、时间统一用阿拉伯数字，保留原数值、单位和近似程度，如2026年8月26日下午3点半、308块5；代码和标识符保留原样。没有可辨认语音时 text 为 ""。
仅输出一个合法 JSON 对象，不加解释或 Markdown。固定按 memory_candidates、context、text 的顺序输出。字段之间用逗号，字符串用双引号并转义内部引号。空数组写 []，其后不加双引号。最后的 text 字符串关闭后直接用右大括号结束：
{"memory_candidates":[],"context":{"kind":"uncertain"},"text":"转写后的文字"}
能确定新话题时 context 用 {"kind":"new","label":"话题名称"}；与参考中的已知话题明确一致时用 {"kind":"existing","id":"参考里的真实ID"}；其余用 {"kind":"uncertain"}。参考只用于消歧。
仅在语境确定时提取本次确实说过的可复用词语，memory_candidates 每条为 {"kind":"proper_noun","text":"词语"} 或 {"kind":"phrase","text":"说法"}，最多8条，每条最多80字。没有合适词语或语境不确定时用 []。
"#;

/// File-only settings. Authentication is resolved from an environment variable, never logged.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DictationConfig {
    pub enabled: bool,
    pub url: String,
    pub model: String,
    pub api_key_env: String,
    /// Complete task instructions: None uses the default; an empty string disables them.
    pub prompt: Option<String>,
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
            memory: MemoryConfig::default(),
        }
    }
}

impl DictationConfig {
    pub fn prompt(&self) -> &str {
        self.prompt.as_deref().unwrap_or(DEFAULT_PROMPT)
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
            self.prompt
                .as_ref()
                .is_none_or(|p| p.chars().count() <= 4096),
            "dictation.prompt exceeds 4096 characters"
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
