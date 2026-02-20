use serde_json::Value;
use std::path::Path;

const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub groq_api_key: Option<String>,
    pub model: String,
    pub language: Option<String>,
    pub temperature: f32,
    pub hotkey: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            groq_api_key: None,
            model: "whisper-large-v3-turbo".to_string(),
            language: None,
            temperature: 0.0,
            hotkey: "F8".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let mut config = AppConfig::default();

        if Path::new(CONFIG_FILE).exists() {
            match std::fs::read_to_string(CONFIG_FILE) {
                Ok(content) => match serde_json::from_str::<Value>(&content) {
                    Ok(json) => {
                        config.apply_json(&json);
                        println!("[Config] 已从 {} 加载配置", CONFIG_FILE);
                    }
                    Err(e) => eprintln!("[Config] 解析 {} 失败: {}", CONFIG_FILE, e),
                },
                Err(e) => eprintln!("[Config] 读取 {} 失败: {}", CONFIG_FILE, e),
            }
        } else {
            println!("[Config] 未找到 {}，使用默认配置", CONFIG_FILE);
        }

        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            config.groq_api_key = Some(key);
        }

        config
    }

    fn apply_json(&mut self, json: &Value) {
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
        assert!(config.language.is_none());
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
}
