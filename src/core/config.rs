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

/// One user-facing config field: its key plus string-based accessors.
/// `get_field`, `set_field`, `apply_json` (config.json loading), and the CLI
/// `config list` all derive from this single table, so adding a field means
/// adding exactly one entry here.
struct FieldSpec {
    key: &'static str,
    get: fn(&AppConfig) -> Option<String>,
    set: fn(&mut AppConfig, &str) -> Result<(), String>,
    /// Optional loader override for values coming from config.json. `None`
    /// reuses `set` leniently (invalid values warn and keep the default).
    /// Only needed where loading differs from CLI mutation — secrets are
    /// loadable from the file but rejected by the CLI setter.
    apply: Option<fn(&mut AppConfig, &serde_json::Value)>,
}

/// Map legacy aliases onto their canonical key.
fn canonical_key(key: &str) -> &str {
    if key == "groq_api_key" {
        "api_key"
    } else {
        key
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, String> {
    value
        .parse::<bool>()
        .map_err(|_| format!("{key} must be true/false, got: {value}"))
}

const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        key: "api_key",
        get: |c| c.api_key.as_ref().map(|_| "*** (set)".to_string()),
        set: |_, _| {
            Err("api_key cannot be saved by the config command; use the TRANSCRIPTION_API_KEY environment variable or edit config.json manually".to_string())
        },
        apply: Some(|c, v| {
            if let Some(key) = v.as_str() {
                c.api_key = Some(key.to_string());
            }
        }),
    },
    FieldSpec {
        key: "transcription_api_url",
        get: |c| Some(c.transcription_api_url.clone()),
        set: |c, v| {
            c.transcription_api_url = v.to_string();
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "provider",
        get: |c| c.provider.clone(),
        set: |c, v| {
            c.provider = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "model",
        get: |c| Some(c.model.clone()),
        set: |c, v| {
            c.model = v.to_string();
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "hold_hotkey",
        get: |c| Some(c.hold_hotkey.clone()),
        set: |c, v| {
            c.hold_hotkey = v.to_string();
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "toggle_hotkey",
        get: |c| Some(c.toggle_hotkey.clone()),
        set: |c, v| {
            c.toggle_hotkey = v.to_string();
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "language",
        get: |c| c.language.clone(),
        set: |c, v| {
            c.language = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "prompt",
        get: |c| c.prompt.clone(),
        set: |c, v| {
            c.prompt = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "temperature",
        get: |c| Some(c.temperature.to_string()),
        set: |c, v| {
            c.temperature = parse_finite_f32("temperature", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "mic_gain",
        get: |c| Some(c.mic_gain.to_string()),
        set: |c, v| {
            c.mic_gain = parse_finite_f32("mic_gain", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "max_chunk_duration_secs",
        get: |c| Some(c.max_chunk_duration_secs.to_string()),
        set: |c, v| {
            c.max_chunk_duration_secs = v
                .parse::<u32>()
                .map_err(|_| format!("max_chunk_duration_secs must be a u32, got: {v}"))?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "max_chunk_size_bytes",
        get: |c| Some(c.max_chunk_size_bytes.to_string()),
        set: |c, v| {
            c.max_chunk_size_bytes = v
                .parse::<u64>()
                .map_err(|_| format!("max_chunk_size_bytes must be a u64, got: {v}"))?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "max_retries",
        get: |c| Some(c.max_retries.to_string()),
        set: |c, v| {
            let parsed = v
                .parse::<u32>()
                .map_err(|_| format!("max_retries must be a u32, got: {v}"))?;
            if parsed > MAX_RETRIES {
                return Err(format!("max_retries must be <= {MAX_RETRIES}, got: {v}"));
            }
            c.max_retries = parsed;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "convergence_timeout_secs",
        get: |c| Some(c.convergence_timeout_secs.to_string()),
        set: |c, v| {
            let parsed = v
                .parse::<u64>()
                .map_err(|_| format!("convergence_timeout_secs must be a u64, got: {v}"))?;
            if parsed > MAX_CONVERGENCE_TIMEOUT_SECS {
                return Err(format!(
                    "convergence_timeout_secs must be <= {MAX_CONVERGENCE_TIMEOUT_SECS}, got: {v}"
                ));
            }
            c.convergence_timeout_secs = parsed;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_enabled",
        get: |c| Some(c.post_process_enabled.to_string()),
        set: |c, v| {
            c.post_process_enabled = parse_bool("post_process_enabled", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_streaming_enabled",
        get: |c| Some(c.post_process_streaming_enabled.to_string()),
        set: |c, v| {
            c.post_process_streaming_enabled = parse_bool("post_process_streaming_enabled", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_api_url",
        get: |c| c.post_process_api_url.clone(),
        set: |c, v| {
            c.post_process_api_url = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_api_key",
        get: |c| {
            c.post_process_api_key
                .as_ref()
                .map(|_| "*** (set)".to_string())
        },
        set: |_, _| {
            Err("post_process_api_key cannot be saved by the config command; use the POST_PROCESS_API_KEY environment variable or edit config.json manually".to_string())
        },
        apply: Some(|c, v| {
            if let Some(key) = v.as_str() {
                c.post_process_api_key = Some(key.to_string());
            }
        }),
    },
    FieldSpec {
        key: "post_process_model",
        get: |c| c.post_process_model.clone(),
        set: |c, v| {
            c.post_process_model = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_prompt",
        get: |c| c.post_process_prompt.clone(),
        set: |c, v| {
            c.post_process_prompt = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "post_process_temperature",
        get: |c| Some(c.post_process_temperature.to_string()),
        set: |c, v| {
            c.post_process_temperature = parse_finite_f32("post_process_temperature", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "local_mode",
        get: |c| Some(c.local_mode.to_string()),
        set: |c, v| {
            c.local_mode = parse_bool("local_mode", v)?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "local_data_dir",
        get: |c| c.local_data_dir.clone(),
        set: |c, v| {
            c.local_data_dir = Some(v.to_string());
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "local_server_port",
        get: |c| Some(c.local_server_port.to_string()),
        set: |c, v| {
            c.local_server_port = v
                .parse::<u16>()
                .map_err(|_| format!("local_server_port must be a u16, got: {v}"))?;
            Ok(())
        },
        apply: None,
    },
    FieldSpec {
        key: "local_quantization",
        get: |c| Some(c.local_quantization.clone()),
        set: |c, v| {
            c.local_quantization = v.to_string();
            Ok(())
        },
        apply: None,
    },
];

fn parse_finite_f32(key: &str, value: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{key} must be a float, got: {value}"))?;
    if !parsed.is_finite() {
        return Err(format!("{key} must be finite, got: {value}"));
    }
    Ok(parsed)
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
        let key = canonical_key(key);
        FIELDS
            .iter()
            .find(|field| field.key == key)
            .and_then(|field| (field.get)(self))
    }

    /// Set a config field value (accepts string, auto-converts types)
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<(), String> {
        let canonical = canonical_key(key);
        match FIELDS.iter().find(|field| field.key == canonical) {
            Some(field) => (field.set)(self, value),
            None => Err(format!(
                "Unknown config key: {}. Available: {}",
                key,
                Self::field_keys().collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// Ordered list of user-facing config keys (drives `config list`).
    pub fn field_keys() -> impl Iterator<Item = &'static str> {
        FIELDS.iter().map(|field| field.key)
    }

    /// Lenient loader for config.json: every present field is applied
    /// individually; an invalid or out-of-range value warns and keeps the
    /// default instead of discarding the whole file.
    fn apply_json(&mut self, json: &serde_json::Value) {
        // Legacy aliases first so the canonical keys win when both are present.
        if let Some(key) = json["groq_api_key"].as_str() {
            self.api_key = Some(key.to_string());
        }
        if let Some(hotkey) = json["hotkey"].as_str() {
            self.hold_hotkey = hotkey.to_string();
        }

        for field in FIELDS {
            let value = &json[field.key];
            if value.is_null() {
                continue;
            }
            if let Some(apply) = field.apply {
                apply(self, value);
                continue;
            }
            // Generic path: render the JSON scalar as the string form the CLI
            // setter accepts and reuse its parsing/validation.
            let rendered = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => {
                    warn!(key = field.key, "Ignoring non-scalar config value");
                    continue;
                }
            };
            if let Err(error) = (field.set)(self, &rendered) {
                warn!(key = field.key, error = %error, "Ignoring invalid config value");
            }
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
