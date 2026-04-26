use serde::{Deserialize, Serialize};
use std::fs;
use tracing::{info, warn};

const CONFIG_FILE: &str = "config.json";

/// Default transcription API URL (Groq Whisper endpoint).
const DEFAULT_TRANSCRIPTION_API_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

fn default_chunk_duration() -> u32 {
    30
}

fn default_chunk_size() -> u64 {
    23 * 1024 * 1024
}

fn default_retries() -> u32 {
    3
}

fn default_convergence_timeout() -> u64 {
    30
}

fn default_post_process_streaming_enabled() -> bool {
    true
}

fn default_local_server_port() -> u16 {
    17265
}

fn default_local_quantization() -> String {
    "int8".to_string()
}

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
    /// Maximum duration (in seconds) per audio chunk when splitting long recordings.
    /// 0 means no duration limit (size limit still applies). Default: 30.
    #[serde(default = "default_chunk_duration")]
    pub max_chunk_duration_secs: u32,
    /// Maximum byte size per audio chunk (including WAV header). Default: 23 MiB.
    /// 0 means no size limit (duration limit still applies).
    #[serde(default = "default_chunk_size")]
    pub max_chunk_size_bytes: u64,
    /// Maximum number of retry attempts per chunk upload on transient errors. Default: 3.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// How long (in seconds) `stop_session` waits for background chunk uploads to
    /// complete after recording stops. Chunks still pending at the deadline are
    /// marked `Failed(Timeout)` and the partial result is returned. Default: 30.
    #[serde(default = "default_convergence_timeout")]
    pub convergence_timeout_secs: u64,

    // --- LLM text post-processing ---
    /// Enable LLM-based text post-processing after STT. Default: false.
    #[serde(default)]
    pub post_process_enabled: bool,
    /// If true, the `run_listener` path feeds stable STT chunks to the post-processor
    /// incrementally instead of waiting for the full session to complete. Default: true.
    #[serde(default = "default_post_process_streaming_enabled")]
    pub post_process_streaming_enabled: bool,
    /// Full URL of the LLM chat-completions endpoint (OpenAI-compatible).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_process_api_url: Option<String>,
    /// API key for the post-processing LLM service.
    /// Not saved to config.json; load from `post_process_api_key` in JSON or
    /// `POST_PROCESS_API_KEY` env var.
    #[serde(skip)]
    pub post_process_api_key: Option<String>,
    /// LLM model name for post-processing (e.g., "gpt-4o-mini").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_process_model: Option<String>,
    /// System prompt for the post-processing LLM. Falls back to a built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_process_prompt: Option<String>,
    /// Temperature for the post-processing LLM. Default: 0.0.
    #[serde(default)]
    pub post_process_temperature: f32,
    /// If true, use the local Gemma service instead of cloud APIs.
    #[serde(default)]
    pub local_mode: bool,
    /// Directory for the local model weights and Python virtualenv.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_data_dir: Option<String>,
    /// Port for the local inference server. Default: 17265.
    #[serde(default = "default_local_server_port")]
    pub local_server_port: u16,
    /// Quantization mode for the local service. Default: "int8".
    #[serde(default = "default_local_quantization")]
    pub local_quantization: String,
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
            max_chunk_duration_secs: default_chunk_duration(),
            max_chunk_size_bytes: default_chunk_size(),
            max_retries: default_retries(),
            convergence_timeout_secs: default_convergence_timeout(),
            post_process_enabled: false,
            post_process_streaming_enabled: default_post_process_streaming_enabled(),
            post_process_api_url: None,
            post_process_api_key: None,
            post_process_model: None,
            post_process_prompt: None,
            post_process_temperature: 0.0,
            local_mode: false,
            local_data_dir: None,
            local_server_port: default_local_server_port(),
            local_quantization: default_local_quantization(),
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
        if let Ok(key) = std::env::var("GROQ_API_KEY")
            && config.api_key.is_none()
        {
            config.api_key = Some(key);
        }
        if let Ok(key) = std::env::var("TRANSCRIPTION_API_KEY") {
            config.api_key = Some(key);
        }
        if let Ok(key) = std::env::var("POST_PROCESS_API_KEY") {
            config.post_process_api_key = Some(key);
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
            "api_key" | "groq_api_key" => self.api_key.as_ref().map(|_| "*** (set)".to_string()),
            "transcription_api_url" => Some(self.transcription_api_url.clone()),
            "provider" => self.provider.clone(),
            "model" => Some(self.model.clone()),
            "hold_hotkey" => Some(self.hold_hotkey.clone()),
            "toggle_hotkey" => Some(self.toggle_hotkey.clone()),
            "temperature" => Some(self.temperature.to_string()),
            "mic_gain" => Some(self.mic_gain.to_string()),
            "language" => self.language.clone(),
            "prompt" => self.prompt.clone(),
            "max_chunk_duration_secs" => Some(self.max_chunk_duration_secs.to_string()),
            "max_chunk_size_bytes" => Some(self.max_chunk_size_bytes.to_string()),
            "max_retries" => Some(self.max_retries.to_string()),
            "convergence_timeout_secs" => Some(self.convergence_timeout_secs.to_string()),
            "post_process_enabled" => Some(self.post_process_enabled.to_string()),
            "post_process_streaming_enabled" => {
                Some(self.post_process_streaming_enabled.to_string())
            }
            "post_process_api_url" => self.post_process_api_url.clone(),
            "post_process_api_key" => self
                .post_process_api_key
                .as_ref()
                .map(|_| "*** (set)".to_string()),
            "post_process_model" => self.post_process_model.clone(),
            "post_process_prompt" => self.post_process_prompt.clone(),
            "post_process_temperature" => Some(self.post_process_temperature.to_string()),
            "local_mode" => Some(self.local_mode.to_string()),
            "local_data_dir" => self.local_data_dir.clone(),
            "local_server_port" => Some(self.local_server_port.to_string()),
            "local_quantization" => Some(self.local_quantization.clone()),
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
            "max_chunk_duration_secs" => {
                self.max_chunk_duration_secs = value.parse::<u32>().map_err(|_| {
                    format!("max_chunk_duration_secs must be a u32, got: {}", value)
                })?;
                Ok(())
            }
            "max_chunk_size_bytes" => {
                self.max_chunk_size_bytes = value
                    .parse::<u64>()
                    .map_err(|_| format!("max_chunk_size_bytes must be a u64, got: {}", value))?;
                Ok(())
            }
            "max_retries" => {
                self.max_retries = value
                    .parse::<u32>()
                    .map_err(|_| format!("max_retries must be a u32, got: {}", value))?;
                Ok(())
            }
            "convergence_timeout_secs" => {
                self.convergence_timeout_secs = value.parse::<u64>().map_err(|_| {
                    format!("convergence_timeout_secs must be a u64, got: {}", value)
                })?;
                Ok(())
            }
            "post_process_enabled" => {
                self.post_process_enabled = value.parse::<bool>().map_err(|_| {
                    format!("post_process_enabled must be true/false, got: {}", value)
                })?;
                Ok(())
            }
            "post_process_streaming_enabled" => {
                self.post_process_streaming_enabled = value.parse::<bool>().map_err(|_| {
                    format!(
                        "post_process_streaming_enabled must be true/false, got: {}",
                        value
                    )
                })?;
                Ok(())
            }
            "post_process_api_url" => {
                self.post_process_api_url = Some(value.to_string());
                Ok(())
            }
            "post_process_api_key" => {
                self.post_process_api_key = Some(value.to_string());
                Ok(())
            }
            "post_process_model" => {
                self.post_process_model = Some(value.to_string());
                Ok(())
            }
            "post_process_prompt" => {
                self.post_process_prompt = Some(value.to_string());
                Ok(())
            }
            "post_process_temperature" => {
                self.post_process_temperature = value.parse::<f32>().map_err(|_| {
                    format!("post_process_temperature must be a float, got: {}", value)
                })?;
                Ok(())
            }
            "local_mode" => {
                self.local_mode = value
                    .parse::<bool>()
                    .map_err(|_| format!("local_mode must be true/false, got: {}", value))?;
                Ok(())
            }
            "local_data_dir" => {
                self.local_data_dir = Some(value.to_string());
                Ok(())
            }
            "local_server_port" => {
                self.local_server_port = value
                    .parse::<u16>()
                    .map_err(|_| format!("local_server_port must be a u16, got: {}", value))?;
                Ok(())
            }
            "local_quantization" => {
                self.local_quantization = value.to_string();
                Ok(())
            }
            _ => Err(format!(
                "Unknown config key: {}. Available: api_key, transcription_api_url, model, \
                 hold_hotkey, toggle_hotkey, language, prompt, temperature, mic_gain, \
                 max_chunk_duration_secs, max_chunk_size_bytes, max_retries, \
                 convergence_timeout_secs, post_process_enabled, post_process_streaming_enabled, \
                 post_process_api_url, post_process_api_key, post_process_model, \
                 post_process_prompt, post_process_temperature, \
                 local_mode, local_data_dir, local_server_port, local_quantization",
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
        if let Some(key) = json["groq_api_key"].as_str()
            && self.api_key.is_none()
        {
            self.api_key = Some(key.to_string());
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
        if let Some(v) = json["max_chunk_duration_secs"].as_u64() {
            self.max_chunk_duration_secs = v as u32;
        }
        if let Some(v) = json["max_chunk_size_bytes"].as_u64() {
            self.max_chunk_size_bytes = v;
        }
        if let Some(v) = json["max_retries"].as_u64() {
            self.max_retries = v as u32;
        }
        if let Some(v) = json["convergence_timeout_secs"].as_u64() {
            self.convergence_timeout_secs = v;
        }
        if let Some(v) = json["post_process_enabled"].as_bool() {
            self.post_process_enabled = v;
        }
        if let Some(v) = json["post_process_streaming_enabled"].as_bool() {
            self.post_process_streaming_enabled = v;
        }
        if let Some(v) = json["post_process_api_url"].as_str() {
            self.post_process_api_url = Some(v.to_string());
        }
        if let Some(v) = json["post_process_api_key"].as_str() {
            self.post_process_api_key = Some(v.to_string());
        }
        if let Some(v) = json["post_process_model"].as_str() {
            self.post_process_model = Some(v.to_string());
        }
        if let Some(v) = json["post_process_prompt"].as_str() {
            self.post_process_prompt = Some(v.to_string());
        }
        if let Some(v) = json["post_process_temperature"].as_f64() {
            self.post_process_temperature = v as f32;
        }
        if let Some(v) = json["local_mode"].as_bool() {
            self.local_mode = v;
        }
        if let Some(v) = json["local_data_dir"].as_str() {
            self.local_data_dir = Some(v.to_string());
        }
        if let Some(v) = json["local_server_port"].as_u64() {
            self.local_server_port = v as u16;
        }
        if let Some(v) = json["local_quantization"].as_str() {
            self.local_quantization = v.to_string();
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
        assert!(!config.local_mode);
        assert!(config.local_data_dir.is_none());
        assert_eq!(config.local_server_port, 17265);
        assert_eq!(config.local_quantization, "int8");
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

    #[test]
    fn test_default_chunk_config() {
        let config = AppConfig::default();
        assert_eq!(config.max_chunk_duration_secs, 30);
        assert_eq!(config.max_chunk_size_bytes, 23 * 1024 * 1024);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_apply_json_chunk_config() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "max_chunk_duration_secs": 60,
            "max_chunk_size_bytes": 10485760u64,
            "max_retries": 5
        });
        config.apply_json(&json);
        assert_eq!(config.max_chunk_duration_secs, 60);
        assert_eq!(config.max_chunk_size_bytes, 10485760);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn test_backward_compat_missing_chunk_fields() {
        // Old config without chunk fields should use defaults after apply_json
        let mut config = AppConfig::default();
        let json = serde_json::json!({ "model": "whisper-large-v3" });
        config.apply_json(&json);
        assert_eq!(config.max_chunk_duration_secs, 30);
        assert_eq!(config.max_chunk_size_bytes, 23 * 1024 * 1024);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_default_convergence_timeout() {
        let config = AppConfig::default();
        assert_eq!(config.convergence_timeout_secs, 30);
    }

    #[test]
    fn test_apply_json_convergence_timeout() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({ "convergence_timeout_secs": 60u64 });
        config.apply_json(&json);
        assert_eq!(config.convergence_timeout_secs, 60);
    }

    #[test]
    fn test_backward_compat_missing_convergence_timeout() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({ "model": "whisper-large-v3" });
        config.apply_json(&json);
        // Missing field → default applied.
        assert_eq!(config.convergence_timeout_secs, 30);
    }

    #[test]
    fn test_get_set_convergence_timeout() {
        let mut config = AppConfig::default();
        assert_eq!(
            config.get_field("convergence_timeout_secs"),
            Some("30".to_string())
        );
        config.set_field("convergence_timeout_secs", "120").unwrap();
        assert_eq!(config.convergence_timeout_secs, 120);
        assert_eq!(
            config.get_field("convergence_timeout_secs"),
            Some("120".to_string())
        );
    }

    #[test]
    fn test_get_set_chunk_fields() {
        let mut config = AppConfig::default();
        assert_eq!(
            config.get_field("max_chunk_duration_secs"),
            Some("30".to_string())
        );
        assert_eq!(config.get_field("max_retries"), Some("3".to_string()));
        config.set_field("max_chunk_duration_secs", "45").unwrap();
        assert_eq!(config.max_chunk_duration_secs, 45);
        config
            .set_field("max_chunk_size_bytes", "10485760")
            .unwrap();
        assert_eq!(config.max_chunk_size_bytes, 10485760);
        config.set_field("max_retries", "5").unwrap();
        assert_eq!(config.max_retries, 5);
    }

    // --- post-process config tests ---

    #[test]
    fn test_default_post_process_disabled() {
        let config = AppConfig::default();
        assert!(!config.post_process_enabled);
    }

    #[test]
    fn test_default_post_process_streaming_enabled() {
        let config = AppConfig::default();
        assert!(config.post_process_streaming_enabled);
    }

    #[test]
    fn test_get_set_post_process_enabled() {
        let mut config = AppConfig::default();
        assert_eq!(
            config.get_field("post_process_enabled"),
            Some("false".to_string())
        );
        config.set_field("post_process_enabled", "true").unwrap();
        assert!(config.post_process_enabled);
        assert_eq!(
            config.get_field("post_process_enabled"),
            Some("true".to_string())
        );
    }

    #[test]
    fn test_get_set_post_process_streaming_enabled() {
        let mut config = AppConfig::default();
        config
            .set_field("post_process_streaming_enabled", "false")
            .unwrap();
        assert!(!config.post_process_streaming_enabled);
    }

    #[test]
    fn test_get_set_post_process_model() {
        let mut config = AppConfig::default();
        assert_eq!(config.get_field("post_process_model"), None);
        config
            .set_field("post_process_model", "gpt-4o-mini")
            .unwrap();
        assert_eq!(config.post_process_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(
            config.get_field("post_process_model"),
            Some("gpt-4o-mini".to_string())
        );
    }

    #[test]
    fn test_get_set_post_process_prompt() {
        let mut config = AppConfig::default();
        config
            .set_field("post_process_prompt", "custom prompt")
            .unwrap();
        assert_eq!(
            config.get_field("post_process_prompt"),
            Some("custom prompt".to_string())
        );
    }

    #[test]
    fn test_get_set_post_process_api_key_masked() {
        let mut config = AppConfig::default();
        assert_eq!(config.get_field("post_process_api_key"), None);
        config.set_field("post_process_api_key", "secret").unwrap();
        assert_eq!(config.post_process_api_key.as_deref(), Some("secret"));
        assert_eq!(
            config.get_field("post_process_api_key"),
            Some("*** (set)".to_string())
        );
    }

    #[test]
    fn test_apply_json_post_process_fields() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "post_process_enabled": true,
            "post_process_streaming_enabled": false,
            "post_process_api_url": "https://api.example.com/v1/chat/completions",
            "post_process_model": "gpt-4o-mini",
            "post_process_prompt": "clean up",
            "post_process_temperature": 0.1
        });
        config.apply_json(&json);
        assert!(config.post_process_enabled);
        assert!(!config.post_process_streaming_enabled);
        assert_eq!(
            config.post_process_api_url.as_deref(),
            Some("https://api.example.com/v1/chat/completions")
        );
        assert_eq!(config.post_process_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(config.post_process_prompt.as_deref(), Some("clean up"));
        assert!((config.post_process_temperature - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_backward_compat_missing_post_process_fields() {
        // Old config without post-process fields should use defaults.
        let mut config = AppConfig::default();
        let json = serde_json::json!({ "model": "whisper-large-v3" });
        config.apply_json(&json);
        assert!(!config.post_process_enabled);
        assert!(config.post_process_streaming_enabled);
        assert!(config.post_process_api_key.is_none());
        assert!(config.post_process_model.is_none());
    }

    #[test]
    fn test_local_config_get_set() {
        let mut config = AppConfig::default();
        config.set_field("local_mode", "true").unwrap();
        config
            .set_field("local_data_dir", "/tmp/viberwhisper")
            .unwrap();
        config.set_field("local_server_port", "9000").unwrap();
        config.set_field("local_quantization", "bf16").unwrap();

        assert_eq!(config.get_field("local_mode").as_deref(), Some("true"));
        assert_eq!(
            config.get_field("local_data_dir").as_deref(),
            Some("/tmp/viberwhisper")
        );
        assert_eq!(
            config.get_field("local_server_port").as_deref(),
            Some("9000")
        );
        assert_eq!(
            config.get_field("local_quantization").as_deref(),
            Some("bf16")
        );
    }

    #[test]
    fn test_apply_json_local_fields() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "local_mode": true,
            "local_data_dir": "/tmp/local-data",
            "local_server_port": 9001,
            "local_quantization": "bf16"
        });

        config.apply_json(&json);

        assert!(config.local_mode);
        assert_eq!(config.local_data_dir.as_deref(), Some("/tmp/local-data"));
        assert_eq!(config.local_server_port, 9001);
        assert_eq!(config.local_quantization, "bf16");
    }
}
