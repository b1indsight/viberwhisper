# Core Module Architecture

## Purpose

The `core` module (`src/core/`) contains two sub-modules: configuration persistence (`config.rs`) and CLI argument parsing (`cli.rs`).

---

## Config (`src/core/config.rs`)

### `AppConfig` Struct

```rust
pub struct AppConfig {
    pub groq_api_key: Option<String>,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: f32,
    pub hold_hotkey: String,
    pub toggle_hotkey: String,
    pub mic_gain: f32,
}
```

Serialized to/from `config.json` via `serde_json`. `groq_api_key` is excluded from the saved file and loaded from the `GROQ_API_KEY` environment variable instead.

**Defaults:**

| Field | Default |
|---|---|
| `model` | `"whisper-large-v3-turbo"` |
| `language` | `"zh"` |
| `temperature` | `0.0` |
| `hold_hotkey` | `"F8"` |
| `toggle_hotkey` | `"F9"` |
| `mic_gain` | `1.0` |

### Key Methods

**`AppConfig::load() -> Self`**

Loads config in priority order:
1. Defaults via `Default::default()`
2. `config.json` (partial override via `apply_json`)
3. `GROQ_API_KEY` env var (overrides json)

**`save(&self) -> Result<()>`**

Serializes to pretty-printed JSON, stripping `groq_api_key` before writing to `config.json`.

**`get_field(&self, key: &str) -> Option<String>`**

Returns a string representation of the named field. Returns `"*** (set)"` for `groq_api_key` if present, `None` for unknown keys.

**`set_field(&mut self, key: &str, value: &str) -> Result<(), String>`**

Sets a field by name, auto-parsing float values for `temperature` and `mic_gain`. Returns an error string for unknown keys or invalid float values.

**`apply_json(&mut self, json: &Value)`** *(private)*

Applies partial JSON overrides. Supports backward compatibility: the old `"hotkey"` key maps to `hold_hotkey`.

---

## CLI (`src/core/cli.rs`)

### `Cli` Struct

```rust
pub struct Cli {
    pub command: Option<Commands>,
}
```

Parsed with `clap::Parser`. No subcommand runs the main recording loop.

### `Commands` Enum

| Variant | Description |
|---|---|
| `Config { action: ConfigAction }` | Configuration management subcommand |
| `Convert { input: String, output: Option<String> }` | Transcribe a WAV file to text |

### `ConfigAction` Enum

| Variant | Description |
|---|---|
| `List` | Print all config fields and current values |
| `Get { key: String }` | Print a single field value |
| `Set { key: String, value: String }` | Update a field and save |
