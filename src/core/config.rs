use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

const CONFIG_FILE: &str = "config.json";

/// Resolve where the config file lives.
///
/// A `config.json` in the current working directory takes precedence so
/// existing setups and `cargo run` keep working. Otherwise fall back to the
/// per-user config directory, which is what a bundled app launched from
/// Finder/Explorer needs (its cwd is not the project directory).
fn config_file_path() -> PathBuf {
    resolve_config_path(PathBuf::from(CONFIG_FILE), dirs::config_dir())
}

fn resolve_config_path(cwd_candidate: PathBuf, config_dir: Option<PathBuf>) -> PathBuf {
    if cwd_candidate.exists() {
        return cwd_candidate;
    }
    config_dir
        .map(|dir| dir.join("viberwhisper").join(CONFIG_FILE))
        .unwrap_or(cwd_candidate)
}
const MAX_RETRIES: u32 = 16;
const MAX_CONVERGENCE_TIMEOUT_SECS: u64 = 60 * 60;

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

fn parse_finite_f32(key: &str, value: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{key} must be a float, got: {value}"))?;
    if !parsed.is_finite() {
        return Err(format!("{key} must be finite, got: {value}"));
    }
    Ok(parsed)
}

fn finite_f32_from_json(key: &str, value: f64) -> Option<f32> {
    let narrowed = value as f32;
    if narrowed.is_finite() {
        Some(narrowed)
    } else {
        warn!(key, value, "Ignoring non-finite or out-of-range float");
        None
    }
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

        let path = config_file_path();
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    config.apply_json(&json);
                    info!(file = %path.display(), "Config loaded successfully");
                }
                Err(e) => {
                    warn!(file = %path.display(), error = %e, "Failed to parse config, using defaults")
                }
            }
        } else {
            info!(file = %path.display(), "Config file not found, using defaults");
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
        let path = config_file_path();
        let existing = fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
        let value = self.json_for_save(existing.as_ref())?;
        let json = serde_json::to_string_pretty(&value)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, json)?;
        Ok(())
    }

    /// Build the serialized config while retaining secrets that were already
    /// present on disk. Runtime/env secrets are deliberately never introduced.
    fn json_for_save(
        &self,
        existing: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let (Some(output), Some(existing)) = (value.as_object_mut(), existing) {
            for key in ["api_key", "groq_api_key", "post_process_api_key"] {
                if let Some(secret) = existing.get(key).and_then(serde_json::Value::as_str) {
                    output.insert(
                        key.to_string(),
                        serde_json::Value::String(secret.to_string()),
                    );
                }
            }
        }
        Ok(value)
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
            "api_key" | "groq_api_key" => Err(
                "api_key cannot be saved by the config command; use the TRANSCRIPTION_API_KEY environment variable or edit config.json manually"
                    .to_string(),
            ),
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
                self.temperature = parse_finite_f32("temperature", value)?;
                Ok(())
            }
            "mic_gain" => {
                self.mic_gain = parse_finite_f32("mic_gain", value)?;
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
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("max_retries must be a u32, got: {}", value))?;
                if parsed > MAX_RETRIES {
                    return Err(format!("max_retries must be <= {MAX_RETRIES}, got: {value}"));
                }
                self.max_retries = parsed;
                Ok(())
            }
            "convergence_timeout_secs" => {
                let parsed = value.parse::<u64>().map_err(|_| {
                    format!("convergence_timeout_secs must be a u64, got: {}", value)
                })?;
                if parsed > MAX_CONVERGENCE_TIMEOUT_SECS {
                    return Err(format!(
                        "convergence_timeout_secs must be <= {MAX_CONVERGENCE_TIMEOUT_SECS}, got: {value}"
                    ));
                }
                self.convergence_timeout_secs = parsed;
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
            "post_process_api_key" => Err(
                "post_process_api_key cannot be saved by the config command; use the POST_PROCESS_API_KEY environment variable or edit config.json manually"
                    .to_string(),
            ),
            "post_process_model" => {
                self.post_process_model = Some(value.to_string());
                Ok(())
            }
            "post_process_prompt" => {
                self.post_process_prompt = Some(value.to_string());
                Ok(())
            }
            "post_process_temperature" => {
                self.post_process_temperature =
                    parse_finite_f32("post_process_temperature", value)?;
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
        if let Some(temp) = json["temperature"].as_f64()
            && let Some(value) = finite_f32_from_json("temperature", temp)
        {
            self.temperature = value;
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
        if let Some(gain) = json["mic_gain"].as_f64()
            && let Some(value) = finite_f32_from_json("mic_gain", gain)
        {
            self.mic_gain = value;
        }
        if let Some(prompt) = json["prompt"].as_str() {
            self.prompt = Some(prompt.to_string());
        }
        if let Some(v) = json["max_chunk_duration_secs"].as_u64() {
            match u32::try_from(v) {
                Ok(value) => self.max_chunk_duration_secs = value,
                Err(_) => warn!(value = v, "Ignoring out-of-range max_chunk_duration_secs"),
            }
        }
        if let Some(v) = json["max_chunk_size_bytes"].as_u64() {
            self.max_chunk_size_bytes = v;
        }
        if let Some(v) = json["max_retries"].as_u64() {
            match u32::try_from(v) {
                Ok(value) if value <= MAX_RETRIES => self.max_retries = value,
                Ok(value) => warn!(
                    value,
                    max = MAX_RETRIES,
                    "Ignoring max_retries above supported limit"
                ),
                Err(_) => warn!(value = v, "Ignoring out-of-range max_retries"),
            }
        }
        if let Some(v) = json["convergence_timeout_secs"].as_u64() {
            if v <= MAX_CONVERGENCE_TIMEOUT_SECS {
                self.convergence_timeout_secs = v;
            } else {
                warn!(
                    value = v,
                    max = MAX_CONVERGENCE_TIMEOUT_SECS,
                    "Ignoring convergence_timeout_secs above supported limit"
                );
            }
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
        if let Some(v) = json["post_process_temperature"].as_f64()
            && let Some(value) = finite_f32_from_json("post_process_temperature", v)
        {
            self.post_process_temperature = value;
        }
        if let Some(v) = json["local_mode"].as_bool() {
            self.local_mode = v;
        }
        if let Some(v) = json["local_data_dir"].as_str() {
            self.local_data_dir = Some(v.to_string());
        }
        if let Some(v) = json["local_server_port"].as_u64() {
            match u16::try_from(v) {
                Ok(value) => self.local_server_port = value,
                Err(_) => warn!(value = v, "Ignoring out-of-range local_server_port"),
            }
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
    fn test_resolve_config_path_prefers_existing_cwd_file() {
        let dir =
            std::env::temp_dir().join(format!("viberwhisper-config-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cwd_file = dir.join(CONFIG_FILE);
        std::fs::write(&cwd_file, "{}").unwrap();

        let resolved = resolve_config_path(cwd_file.clone(), Some(PathBuf::from("/cfg")));
        assert_eq!(resolved, cwd_file);

        let _ = std::fs::remove_file(&cwd_file);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_resolve_config_path_falls_back_to_user_config_dir() {
        let missing = PathBuf::from("/nonexistent-dir/config.json");
        let resolved = resolve_config_path(missing.clone(), Some(PathBuf::from("/cfg")));
        assert_eq!(resolved, PathBuf::from("/cfg/viberwhisper/config.json"));

        // Without a config dir, keep the cwd candidate as a last resort.
        let resolved = resolve_config_path(missing.clone(), None);
        assert_eq!(resolved, missing);
    }

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
        let error = config.set_field("api_key", "mykey").unwrap_err();
        assert!(error.contains("environment variable"));
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_groq_api_key_alias() {
        // groq_api_key is an alias for api_key when reading, but secrets cannot
        // be persisted through the config CLI.
        let mut config = AppConfig::default();
        assert!(config.set_field("groq_api_key", "legacykey").is_err());
        assert!(config.api_key.is_none());
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
    fn test_set_field_rejects_non_finite_floats() {
        let mut config = AppConfig::default();

        for (key, value) in [
            ("temperature", "NaN"),
            ("mic_gain", "inf"),
            ("post_process_temperature", "-inf"),
        ] {
            assert!(
                config.set_field(key, value).is_err(),
                "accepted {key}={value}"
            );
        }
    }

    #[test]
    fn test_apply_json_rejects_floats_that_overflow_f32() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "temperature": 1e300,
            "mic_gain": -1e300,
            "post_process_temperature": 1e300
        });

        config.apply_json(&json);

        assert_eq!(config.temperature, 0.0);
        assert_eq!(config.mic_gain, 1.0);
        assert_eq!(config.post_process_temperature, 0.0);
    }

    #[test]
    fn test_retry_and_timeout_limits_are_enforced() {
        let mut config = AppConfig::default();
        assert!(
            config
                .set_field("max_retries", &(u64::from(MAX_RETRIES) + 1).to_string())
                .is_err()
        );
        assert!(
            config
                .set_field(
                    "convergence_timeout_secs",
                    &(MAX_CONVERGENCE_TIMEOUT_SECS + 1).to_string(),
                )
                .is_err()
        );

        config.apply_json(&serde_json::json!({
            "max_retries": u64::from(MAX_RETRIES) + 1,
            "convergence_timeout_secs": MAX_CONVERGENCE_TIMEOUT_SECS + 1
        }));
        assert_eq!(config.max_retries, default_retries());
        assert_eq!(
            config.convergence_timeout_secs,
            default_convergence_timeout()
        );
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
    fn test_apply_json_rejects_out_of_range_narrow_integers() {
        let mut config = AppConfig::default();
        let json = serde_json::json!({
            "max_chunk_duration_secs": u64::from(u32::MAX) + 1,
            "max_retries": u64::from(u32::MAX) + 1,
            "local_server_port": u64::from(u16::MAX) + 1
        });

        config.apply_json(&json);

        assert_eq!(config.max_chunk_duration_secs, default_chunk_duration());
        assert_eq!(config.max_retries, default_retries());
        assert_eq!(config.local_server_port, default_local_server_port());
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
        let error = config
            .set_field("post_process_api_key", "secret")
            .unwrap_err();
        assert!(error.contains("environment variable"));
        assert!(config.post_process_api_key.is_none());
    }

    #[test]
    fn test_save_json_preserves_existing_secret_fields() {
        let config = AppConfig {
            model: "updated-model".to_string(),
            ..AppConfig::default()
        };
        let existing = serde_json::json!({
            "api_key": "transcription-secret",
            "post_process_api_key": "post-process-secret",
            "model": "old-model"
        });

        let saved = config.json_for_save(Some(&existing)).unwrap();

        assert_eq!(saved["api_key"], "transcription-secret");
        assert_eq!(saved["post_process_api_key"], "post-process-secret");
        assert_eq!(saved["model"], "updated-model");
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
