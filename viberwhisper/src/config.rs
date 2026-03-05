use serde::{Deserialize, Serialize};
use std::fs;

const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub temperature: f32,
    pub hotkey: String,
    pub mic_gain: f32,
    /// 自定义 STT 服务端点（OpenAI 兼容格式）。未设置时使用 Groq 默认端点。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt_endpoint: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            groq_api_key: None,
            model: "whisper-large-v3-turbo".to_string(),
            language: Some("zh".to_string()),
            prompt: Some("以下是一段简体中文的普通话句子，去掉首尾的语气词".to_string()),
            temperature: 0.0,
            hotkey: "F8".to_string(),
            mic_gain: 1.0,
            stt_endpoint: None,
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
                    println!("[Config] 已从 {} 加载配置", CONFIG_FILE);
                }
                Err(e) => eprintln!("[Config] 解析 {} 失败: {}", CONFIG_FILE, e),
            }
        } else {
            println!("[Config] 未找到 {}，使用默认配置", CONFIG_FILE);
        }

        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            config.groq_api_key = Some(key);
        }

        config
    }

    /// 将配置保存到 config.json（不包含 groq_api_key）
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(self)?;
        if let Some(obj) = value.as_object_mut() {
            obj.remove("groq_api_key");
        }
        let json = serde_json::to_string_pretty(&value)?;
        fs::write(CONFIG_FILE, json)?;
        Ok(())
    }

    /// 获取指定字段的字符串值
    pub fn get_field(&self, key: &str) -> Option<String> {
        match key {
            "model" => Some(self.model.clone()),
            "hotkey" => Some(self.hotkey.clone()),
            "temperature" => Some(self.temperature.to_string()),
            "mic_gain" => Some(self.mic_gain.to_string()),
            "language" => self.language.clone(),
            "prompt" => self.prompt.clone(),
            "groq_api_key" => self
                .groq_api_key
                .as_ref()
                .map(|_| "***（已设置）".to_string()),
            "stt_endpoint" => self.stt_endpoint.clone(),
            _ => None,
        }
    }

    /// 设置指定字段的值（接受字符串，自动转换类型）
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "model" => {
                self.model = value.to_string();
                Ok(())
            }
            "hotkey" => {
                self.hotkey = value.to_string();
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
                    .map_err(|_| format!("temperature 必须是浮点数，收到: {}", value))?;
                Ok(())
            }
            "mic_gain" => {
                self.mic_gain = value
                    .parse::<f32>()
                    .map_err(|_| format!("mic_gain 必须是浮点数，收到: {}", value))?;
                Ok(())
            }
            "groq_api_key" => {
                self.groq_api_key = Some(value.to_string());
                Ok(())
            }
            "stt_endpoint" => {
                self.stt_endpoint = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            _ => Err(format!(
                "未知配置项: {}。可用项: model, hotkey, language, prompt, temperature, mic_gain, groq_api_key, stt_endpoint",
                key
            )),
        }
    }

    fn apply_json(&mut self, json: &serde_json::Value) {
        if let Some(key) = json["groq_api_key"].as_str() {
            self.groq_api_key = Some(key.to_string());
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
        if let Some(hotkey) = json["hotkey"].as_str() {
            self.hotkey = hotkey.to_string();
        }
        if let Some(gain) = json["mic_gain"].as_f64() {
            self.mic_gain = gain as f32;
        }
        if let Some(prompt) = json["prompt"].as_str() {
            self.prompt = Some(prompt.to_string());
        }
        if let Some(endpoint) = json["stt_endpoint"].as_str() {
            self.stt_endpoint = Some(endpoint.to_string());
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
        assert_eq!(config.hotkey, "F8");
        assert_eq!(config.temperature, 0.0);
        assert!(config.groq_api_key.is_none());
        assert_eq!(config.language.as_deref(), Some("zh"));
    }

    #[test]
    fn test_apply_json() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "groq_api_key": "test_key",
            "model": "whisper-large-v3",
            "language": "zh",
            "temperature": 0.2,
            "hotkey": "F9"
        });
        config.apply_json(&json);
        assert_eq!(config.groq_api_key.unwrap(), "test_key");
        assert_eq!(config.model, "whisper-large-v3");
        assert_eq!(config.language.unwrap(), "zh");
        assert_eq!(config.temperature, 0.2);
        assert_eq!(config.hotkey, "F9");
    }

    #[test]
    fn test_get_field_known_key() {
        let config = AppConfig::default();
        assert_eq!(
            config.get_field("model"),
            Some("whisper-large-v3-turbo".to_string())
        );
        assert_eq!(config.get_field("hotkey"), Some("F8".to_string()));
    }

    #[test]
    fn test_get_field_unknown_key() {
        let config = AppConfig::default();
        assert_eq!(config.get_field("nonexistent"), None);
    }

    #[test]
    fn test_set_field_string() {
        let mut config = AppConfig::default();
        config.set_field("hotkey", "F9").unwrap();
        assert_eq!(config.hotkey, "F9");
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

    #[test]
    fn test_default_stt_endpoint_is_none() {
        let config = AppConfig::default();
        assert!(config.stt_endpoint.is_none());
    }

    #[test]
    fn test_get_field_stt_endpoint_unset() {
        let config = AppConfig::default();
        assert_eq!(config.get_field("stt_endpoint"), None);
    }

    #[test]
    fn test_set_field_stt_endpoint() {
        let mut config = AppConfig::default();
        config
            .set_field("stt_endpoint", "http://localhost:8080/v1/audio/transcriptions")
            .unwrap();
        assert_eq!(
            config.stt_endpoint,
            Some("http://localhost:8080/v1/audio/transcriptions".to_string())
        );
    }

    #[test]
    fn test_set_field_stt_endpoint_empty_clears() {
        let mut config = AppConfig::default();
        config.stt_endpoint = Some("http://example.com".to_string());
        config.set_field("stt_endpoint", "").unwrap();
        assert!(config.stt_endpoint.is_none());
    }

    #[test]
    fn test_apply_json_stt_endpoint() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "stt_endpoint": "http://localhost:8080/v1/audio/transcriptions"
        });
        config.apply_json(&json);
        assert_eq!(
            config.stt_endpoint,
            Some("http://localhost:8080/v1/audio/transcriptions".to_string())
        );
    }
}
