use serde::{Deserialize, Serialize};
use std::fs;
use tracing::{info, warn};

const CONFIG_FILE: &str = "config.json";

/// Default transcription API URL (Groq Whisper endpoint).
const DEFAULT_TRANSCRIPTION_API_URL: &str =
    "https://api.groq.com/openai/v1/audio/transcriptions";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// API key for the transcription service.
    /// Not saved to config.json; load from `api_key` in JSON or `GROQ_API_KEY` env var.
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Full URL of the transcription API endpoint (OpenAI-compatible multipart format).
    pub transcription_api_url: String,
    /// Optional provider label (informational only; not used for dispatch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub temperature: f32,
    pub hold_hotkey: String,
    pub toggle_hotkey: String,
    pub mic_gain: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            transcription_api_url: DEFAULT_TRANSCRIPTION_API_URL.to_string(),
            provider: None,
            model: "whisper-large-v3-turbo".to_string(),
            language: Some("zh".to_string()),
            prompt: Some("以下是一段简体中文的普通话句子，去掉首尾的语气词".to_string()),
            temperature: 0.0,
            hold_hotkey: "F8".to_string(),
            toggle_hotkey: "F9".to_string(),
            mic_gain: 1.0,
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let mut config = AppConfig::default();

        if let Ok(content) = fs::read_to_string(CONFIG_FILE) {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    config.apply_json(&json);
                    info!(file = %CONFIG_FILE, "Config loaded successfully");
                }
                Err(e) => {
                    warn!(file = %CONFIG_FILE, error = %e, "Failed to parse config, using defaults")
                }
            }
        } else {
            info!(file = %CONFIG_FILE, "Config file not found, using defaults");
        }

        // Env var override: GROQ_API_KEY for backward compat, api_key for new configs
        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            if config.api_key.is_none() {
                config.api_key = Some(key);
            }
        }
        if let Ok(key) = std::env::var("TRANSCRIPTION_API_KEY") {
            config.api_key = Some(key);
        }

        config
    }

    /// Save config to config.json (excludes api_key — never persisted)
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(CONFIG_FILE, json)?;
        Ok(())
    }

    /// Get the string value of a config field
    pub fn get_field(&self, key: &str) -> Option<String> {
        match key {
            "api_key" | "groq_api_key" => {
                self.api_key.as_ref().map(|_| "*** (set)".to_string())
            }
            "transcription_api_url" => Some(self.transcription_api_url.clone()),
            "provider" => self.provider.clone(),
            "model" => Some(self.model.clone()),
            "hold_hotkey" => Some(self.hold_hotkey.clone()),
            "toggle_hotkey" => Some(self.toggle_hotkey.clone()),
            "temperature" => Some(self.temperature.to_string()),
            "mic_gain" => Some(self.mic_gain.to_string()),
            "language" => self.language.clone(),
            "prompt" => self.prompt.clone(),
            _ => None,
        }
    }

    /// Set a config field value (accepts string, auto-converts types)
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "api_key" | "groq_api_key" => {
                self.api_key = Some(value.to_string());
                Ok(())
            }
            "transcription_api_url" => {
                self.transcription_api_url = value.to_string();
                Ok(())
            }
            "provider" => {
                self.provider = Some(value.to_string());
                Ok(())
            }
            "model" => {
                self.model = value.to_string();
                Ok(())
            }
            "hold_hotkey" => {
                self.hold_hotkey = value.to_string();
                Ok(())
            }
            "toggle_hotkey" => {
                self.toggle_hotkey = value.to_string();
                Ok(())
            }
            "language" => {
                self.language = Some(value.to_string());
                Ok(())
            }
            "prompt" => {
                self.prompt = Some(value.to_string());
                Ok(())
            }
            "temperature" => {
                self.temperature = value
                    .parse::<f32>()
                    .map_err(|_| format!("temperature must be a float, got: {}", value))?;
                Ok(())
            }
            "mic_gain" => {
                self.mic_gain = value
                    .parse::<f32>()
                    .map_err(|_| format!("mic_gain must be a float, got: {}", value))?;
                Ok(())
            }
            _ => Err(format!(
                "Unknown config key: {}. Available: api_key, transcription_api_url, model, \
                 hold_hotkey, toggle_hotkey, language, prompt, temperature, mic_gain",
                key
            )),
        }
    }

    fn apply_json(&mut self, json: &serde_json::Value) {
        // New canonical field
        if let Some(key) = json["api_key"].as_str() {
            self.api_key = Some(key.to_string());
        }
        // Backward compat: old groq_api_key maps to api_key
        if let Some(key) = json["groq_api_key"].as_str() {
            if self.api_key.is_none() {
                self.api_key = Some(key.to_string());
            }
        }
        if let Some(url) = json["transcription_api_url"].as_str() {
            self.transcription_api_url = url.to_string();
        }
        if let Some(provider) = json["provider"].as_str() {
            self.provider = Some(provider.to_string());
        }
        if let Some(model) = json["model"].as_str() {
            self.model = model.to_string();
        }
        if let Some(lang) = json["language"].as_str() {
            self.language = Some(lang.to_string());
        }
        if let Some(temp) = json["temperature"].as_f64() {
            self.temperature = temp as f32;
        }
        // Backward compat: old hotkey field maps to hold_hotkey
        if let Some(hotkey) = json["hotkey"].as_str() {
            self.hold_hotkey = hotkey.to_string();
        }
        if let Some(hotkey) = json["hold_hotkey"].as_str() {
            self.hold_hotkey = hotkey.to_string();
        }
        if let Some(hotkey) = json["toggle_hotkey"].as_str() {
            self.toggle_hotkey = hotkey.to_string();
        }
        if let Some(gain) = json["mic_gain"].as_f64() {
            self.mic_gain = gain as f32;
        }
        if let Some(prompt) = json["prompt"].as_str() {
            self.prompt = Some(prompt.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.model, "whisper-large-v3-turbo");
        assert_eq!(config.hold_hotkey, "F8");
        assert_eq!(config.toggle_hotkey, "F9");
        assert_eq!(config.temperature, 0.0);
        assert!(config.api_key.is_none());
        assert_eq!(config.language.as_deref(), Some("zh"));
        assert_eq!(
            config.transcription_api_url,
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
    }

    #[test]
    fn test_api_key_get_set() {
        let mut config = AppConfig::default();
        assert_eq!(config.get_field("api_key"), None);
        config.set_field("api_key", "mykey").unwrap();
        assert_eq!(config.api_key.as_deref(), Some("mykey"));
        assert_eq!(config.get_field("api_key"), Some("*** (set)".to_string()));
    }

    #[test]
    fn test_groq_api_key_alias() {
        // groq_api_key is an alias for api_key in get/set
        let mut config = AppConfig::default();
        config.set_field("groq_api_key", "legacykey").unwrap();
        assert_eq!(config.api_key.as_deref(), Some("legacykey"));
        assert_eq!(
            config.get_field("groq_api_key"),
            Some("*** (set)".to_string())
        );
    }

    #[test]
    fn test_transcription_api_url_get_set() {
        let mut config = AppConfig::default();
        assert_eq!(
            config.get_field("transcription_api_url"),
            Some("https://api.groq.com/openai/v1/audio/transcriptions".to_string())
        );
        config
            .set_field(
                "transcription_api_url",
                "https://api.openai.com/v1/audio/transcriptions",
            )
            .unwrap();
        assert_eq!(
            config.transcription_api_url,
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn test_apply_json_groq_api_key_compat() {
        // Old config with groq_api_key should map to api_key
        let mut config = AppConfig::default();
        let json = serde_json::json!({"groq_api_key": "old_key"});
        config.apply_json(&json);
        assert_eq!(config.api_key.as_deref(), Some("old_key"));
    }

    #[test]
    fn test_apply_json_api_key_takes_precedence() {
        // api_key takes precedence over groq_api_key
        let mut config = AppConfig::default();
        let json = serde_json::json!({"api_key": "new_key", "groq_api_key": "old_key"});
        config.apply_json(&json);
        assert_eq!(config.api_key.as_deref(), Some("new_key"));
    }

    #[test]
    fn test_apply_json_transcription_api_url() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "transcription_api_url": "https://custom.example.com/v1/audio/transcriptions"
        });
        config.apply_json(&json);
        assert_eq!(
            config.transcription_api_url,
            "https://custom.example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn test_apply_json() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "api_key": "test_key",
            "model": "whisper-large-v3",
            "language": "zh",
            "temperature": 0.2,
            "hold_hotkey": "F10",
            "toggle_hotkey": "F11"
        });
        config.apply_json(&json);
        assert_eq!(config.api_key.as_deref(), Some("test_key"));
        assert_eq!(config.model, "whisper-large-v3");
        assert_eq!(config.language.as_deref(), Some("zh"));
        assert_eq!(config.temperature, 0.2);
        assert_eq!(config.hold_hotkey, "F10");
        assert_eq!(config.toggle_hotkey, "F11");
    }

    #[test]
    fn test_apply_json_backward_compat_hotkey() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({"hotkey": "F10"});
        config.apply_json(&json);
        assert_eq!(config.hold_hotkey, "F10");
    }

    #[test]
    fn test_get_field_known_key() {
        let config = AppConfig::default();
        assert_eq!(
            config.get_field("model"),
            Some("whisper-large-v3-turbo".to_string())
        );
        assert_eq!(config.get_field("hold_hotkey"), Some("F8".to_string()));
        assert_eq!(config.get_field("toggle_hotkey"), Some("F9".to_string()));
    }

    #[test]
    fn test_get_field_unknown_key() {
        let config = AppConfig::default();
        assert_eq!(config.get_field("nonexistent"), None);
    }

    #[test]
    fn test_set_field_string() {
        let mut config = AppConfig::default();
        config.set_field("hold_hotkey", "F10").unwrap();
        assert_eq!(config.hold_hotkey, "F10");
        config.set_field("toggle_hotkey", "F11").unwrap();
        assert_eq!(config.toggle_hotkey, "F11");
    }

    #[test]
    fn test_set_field_float() {
        let mut config = AppConfig::default();
        config.set_field("mic_gain", "2.5").unwrap();
        assert!((config.mic_gain - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_set_field_float_invalid() {
        let mut config = AppConfig::default();
        let result = config.set_field("mic_gain", "not_a_number");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_field_unknown_key() {
        let mut config = AppConfig::default();
        let result = config.set_field("nonexistent", "value");
        assert!(result.is_err());
    }
}
