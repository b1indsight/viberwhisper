use serde::{Deserialize, Serialize};
use std::fs;
use tracing::{info, warn};

const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
    pub provider: String,
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
            groq_api_key: None,
            provider: "groq".to_string(),
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
                Err(e) => warn!(file = %CONFIG_FILE, error = %e, "Failed to parse config, using defaults"),
            }
        } else {
            info!(file = %CONFIG_FILE, "Config file not found, using defaults");
        }

        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            config.groq_api_key = Some(key);
        }

        config
    }

    /// Save config to config.json (excludes groq_api_key)
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(self)?;
        if let Some(obj) = value.as_object_mut() {
            obj.remove("groq_api_key");
        }
        let json = serde_json::to_string_pretty(&value)?;
        fs::write(CONFIG_FILE, json)?;
        Ok(())
    }

    /// Get the string value of a config field
    pub fn get_field(&self, key: &str) -> Option<String> {
        match key {
            "provider" => Some(self.provider.clone()),
            "model" => Some(self.model.clone()),
            "hold_hotkey" => Some(self.hold_hotkey.clone()),
            "toggle_hotkey" => Some(self.toggle_hotkey.clone()),
            "temperature" => Some(self.temperature.to_string()),
            "mic_gain" => Some(self.mic_gain.to_string()),
            "language" => self.language.clone(),
            "prompt" => self.prompt.clone(),
            "groq_api_key" => self
                .groq_api_key
                .as_ref()
                .map(|_| "*** (set)".to_string()),
            _ => None,
        }
    }

    /// Set a config field value (accepts string, auto-converts types)
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "provider" => {
                self.provider = value.to_string();
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
            "groq_api_key" => {
                self.groq_api_key = Some(value.to_string());
                Ok(())
            }
            _ => Err(format!(
                "Unknown config key: {}. Available: provider, model, hold_hotkey, toggle_hotkey, language, prompt, temperature, mic_gain, groq_api_key",
                key
            )),
        }
    }

    fn apply_json(&mut self, json: &serde_json::Value) {
        if let Some(key) = json["groq_api_key"].as_str() {
            self.groq_api_key = Some(key.to_string());
        }
        if let Some(provider) = json["provider"].as_str() {
            self.provider = provider.to_string();
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
        assert_eq!(config.provider, "groq");
        assert_eq!(config.model, "whisper-large-v3-turbo");
        assert_eq!(config.hold_hotkey, "F8");
        assert_eq!(config.toggle_hotkey, "F9");
        assert_eq!(config.temperature, 0.0);
        assert!(config.groq_api_key.is_none());
        assert_eq!(config.language.as_deref(), Some("zh"));
    }

    #[test]
    fn test_provider_get_set() {
        let mut config = AppConfig::default();
        assert_eq!(config.get_field("provider"), Some("groq".to_string()));
        config.set_field("provider", "custom").unwrap();
        assert_eq!(config.provider, "custom");
        assert_eq!(config.get_field("provider"), Some("custom".to_string()));
    }

    #[test]
    fn test_apply_json_provider() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({"provider": "groq"});
        config.apply_json(&json);
        assert_eq!(config.provider, "groq");
    }

    #[test]
    fn test_apply_json_no_provider_keeps_default() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({"model": "whisper-large-v3"});
        config.apply_json(&json);
        assert_eq!(config.provider, "groq");
    }

    #[test]
    fn test_apply_json() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "groq_api_key": "test_key",
            "model": "whisper-large-v3",
            "language": "zh",
            "temperature": 0.2,
            "hold_hotkey": "F10",
            "toggle_hotkey": "F11"
        });
        config.apply_json(&json);
        assert_eq!(config.groq_api_key.unwrap(), "test_key");
        assert_eq!(config.model, "whisper-large-v3");
        assert_eq!(config.language.unwrap(), "zh");
        assert_eq!(config.temperature, 0.2);
        assert_eq!(config.hold_hotkey, "F10");
        assert_eq!(config.toggle_hotkey, "F11");
    }

    #[test]
    fn test_apply_json_backward_compat() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "hotkey": "F10"
        });
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
