# ViberWhisper Logging Refactor Plan

**Date:** 2026-03-11
**Scope:** Replace all `println!`/`eprintln!` statements with structured logging
**Status:** Planning (no code changes yet)

---

## 1. Recommended Library: `tracing`

### Decision: `tracing` over `log + env_logger`

| Concern | `log + env_logger` | `tracing` |
|---|---|---|
| Structured fields | No | Yes (`key = value` pairs) |
| Spans (start/end context) | No | Yes |
| File output | Manual setup | `tracing-appender` crate |
| Async-friendly | Partial | Designed for it |
| Ecosystem maturity | Stable, older | Modern, actively developed |
| Complexity | Low | Moderate |

**Why `tracing` wins for ViberWhisper:**

- The codebase already uses module-level prefixes like `[Audio]`, `[Config]`, `[Groq STT]` — `tracing` spans map directly to this pattern with zero manual tagging.
- Recording start/stop in `audio.rs` and transcription in `transcriber.rs` are natural span boundaries.
- `tracing-appender` makes file output trivial to add later.
- The extra complexity is low for a project this size, and the payoff (filterable, structured logs) is high.

### Required Crates

```toml
# Cargo.toml additions
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"   # only if file logging is needed
```

---

## 2. Integration with the Existing Codebase

### Initialization in `main.rs`

Logging must be initialized once at program startup, before any other code runs.

```rust
// main.rs — at the top of main()
use tracing_subscriber::{EnvFilter, fmt};

fn main() {
    // Initialize logging — reads RUST_LOG env var, defaults to "info"
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")),
        )
        .with_target(false)   // suppress crate path prefix in output
        .init();

    // rest of main...
}
```

### Module-level Instrumentation

Each module gets a `tracing::instrument` span or an inline span. The existing `[Audio]`, `[Config]` etc. prefixes become the span name automatically.

```rust
// audio.rs
use tracing::{debug, error, info, warn, instrument};

impl AudioRecorder {
    #[instrument(name = "Audio", skip(self))]
    pub fn start_recording(&mut self) -> Result<(), ...> {
        // tracing automatically logs entry/exit at TRACE level
        // println! calls inside become info!/debug!/etc.
    }
}
```

For modules without a natural struct method, use inline spans:

```rust
// config.rs
use tracing::{info, warn};

pub fn load() -> Config {
    let _span = tracing::info_span!("Config").entered();
    // ...
}
```

### Level Mapping from Existing Statements

| Existing pattern | New level |
|---|---|
| `println!("[DEBUG] ...")` | `debug!(...)` |
| `println!("[Audio] ...")` info-type | `info!(...)` |
| `eprintln!("WARNING: ...")` | `warn!(...)` |
| `eprintln!("ERROR: ...")` | `error!(...)` |
| `eprintln!("Failed to ...")` | `error!(...)` |
| `println!("Recording started")` | `info!(...)` |
| Heartbeat status | `debug!(...)` |
| Startup banner lines | `info!(...)` |

---

## 3. Configuration Options

### Log Level via Environment Variable

`tracing-subscriber` with `EnvFilter` reads `RUST_LOG` at runtime:

```bash
# Default (info and above)
./viberwhisper

# Show debug output for all viberwhisper modules
RUST_LOG=viberwhisper=debug ./viberwhisper

# Show debug for audio only, info for everything else
RUST_LOG=viberwhisper::audio=debug,viberwhisper=info ./viberwhisper

# Maximum verbosity
RUST_LOG=trace ./viberwhisper
```

### Log Level via CLI Flag

Add an optional `--log-level` flag to `cli.rs` using the existing `clap` setup:

```rust
// cli.rs addition
#[arg(long, default_value = "info",
      value_parser = ["error", "warn", "info", "debug", "trace"])]
pub log_level: String,
```

Then in `main()`:

```rust
let filter = EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| EnvFilter::new(format!("viberwhisper={}", args.log_level)));
```

`RUST_LOG` takes precedence over the CLI flag with this approach (the env var is checked first).

### Output to File

Add `tracing-appender` for file output. The simplest approach is daily rolling files:

```rust
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::prelude::*;

fn init_logging(log_to_file: bool) {
    let registry = tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")));

    if log_to_file {
        let log_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("viberwhisper")
            .join("logs");
        let file_appender = rolling::daily(log_dir, "viberwhisper.log");
        let (non_blocking_writer, _guard) = non_blocking(file_appender);
        // _guard must be kept alive for the duration of main()
        registry
            .with(fmt::layer().with_writer(non_blocking_writer))
            .init();
    } else {
        registry
            .with(fmt::layer().with_writer(std::io::stderr))
            .init();
    }
}
```

File location would be:
- macOS: `~/Library/Application Support/viberwhisper/logs/viberwhisper.YYYY-MM-DD`
- Linux: `~/.local/share/viberwhisper/logs/`
- Windows: `%APPDATA%\viberwhisper\logs\`

---

## 4. Migration Strategy

### Phase 1 — Add dependencies and initialize (1 change)

1. Add `tracing`, `tracing-subscriber`, and optionally `tracing-appender` to `Cargo.toml`.
2. Add `use tracing::...` imports and the initialization block in `main.rs`.
3. Verify the project still compiles.

### Phase 2 — Migrate file by file

Tackle one file at a time. Order by number of statements (easiest wins first, then the complex ones):

| Priority | File | Statements | Notes |
|---|---|---|---|
| 1 | `typer.rs` | 1 | Trivial |
| 2 | `typer_macos.rs` | 1 | Trivial |
| 3 | `typer_windows.rs` | 1 | Trivial |
| 4 | `transcriber.rs` | 4 | Two implementations — good `#[instrument]` candidates |
| 5 | `config.rs` | 4 | Simple load/parse flow |
| 6 | `hotkey.rs` | 6 | Thread lifecycle events |
| 7 | `main.rs` | 27 | Most varied; startup banner needs special handling |
| 8 | `audio.rs` | 26 | Most complex; recording spans, error paths |

### Phase 3 — Startup banner

The current startup banner uses multiple `println!` calls for a formatted header. These are intentional UI output, not log events. Two approaches:

**Option A (recommended):** Keep a minimal `println!` for the startup banner only, and route everything else through `tracing`. This is pragmatic and common in CLI tools.

**Option B:** Use `tracing` with a custom format layer that suppresses timestamps for the banner lines. More complex, probably not worth it.

### Phase 4 — Validation

After migration:
- Run with `RUST_LOG=trace` and confirm all expected messages appear.
- Run without `RUST_LOG` set and confirm only `info`-level messages appear.
- Verify error paths still log errors (eprintln! replacements).
- Confirm no `println!`/`eprintln!` remain (except the intentional startup banner if Option A was chosen).

```bash
# Quick audit after migration
grep -rn 'println!\|eprintln!' src/
```

---

## 5. Before / After Examples

### Example 1: Simple info message (`config.rs`)

**Before:**
```rust
pub fn load(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(cfg) => {
                println!("[Config] 已从 {} 加载配置", path.display());
                cfg
            }
            Err(e) => {
                eprintln!("[Config] 解析 {} 失败: {}", path.display(), e);
                Config::default()
            }
        },
        Err(_) => {
            println!("[Config] 未找到 {}，使用默认配置", path.display());
            Config::default()
        }
    }
}
```

**After:**
```rust
use tracing::{info, warn};

pub fn load(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(cfg) => {
                info!(path = %path.display(), "配置加载成功");
                cfg
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "配置解析失败，使用默认配置");
                Config::default()
            }
        },
        Err(_) => {
            info!(path = %path.display(), "配置文件未找到，使用默认配置");
            Config::default()
        }
    }
}
```

---

### Example 2: Debug messages in `audio.rs`

**Before:**
```rust
pub fn stop_recording(&mut self) -> Result<Option<PathBuf>, AudioError> {
    if !self.is_recording {
        println!("DEBUG: Not recording, ignoring stop request");
        return Ok(None);
    }
    println!("DEBUG: Stopping recording...");
    // ...
    println!("DEBUG: Stream stopped");
    println!("DEBUG: Buffer size: {} samples", self.buffer.len());
    // ...
    println!("Recording saved to: {}", path.display());
    Ok(Some(path))
}
```

**After:**
```rust
use tracing::{debug, info};

#[instrument(name = "audio::stop", skip(self))]
pub fn stop_recording(&mut self) -> Result<Option<PathBuf>, AudioError> {
    if !self.is_recording {
        debug!("not recording, ignoring stop request");
        return Ok(None);
    }
    debug!("stopping recording");
    // ...
    debug!("stream stopped");
    debug!(samples = self.buffer.len(), "buffer size");
    // ...
    info!(path = %path.display(), "recording saved");
    Ok(Some(path))
}
```

---

### Example 3: Error handling in `audio.rs` stream callback

**Before:**
```rust
let stream = device.build_input_stream(
    &config,
    move |data: &[f32], _| {
        // ...
    },
    |err| eprintln!("Stream error: {}", err),
    None,
)?;
```

**After:**
```rust
use tracing::error;

let stream = device.build_input_stream(
    &config,
    move |data: &[f32], _| {
        // ...
    },
    |err| error!(error = %err, "audio stream error"),
    None,
)?;
```

---

### Example 4: `#[instrument]` on transcriber

**Before:**
```rust
impl GroqTranscriber {
    pub fn transcribe(&self, path: &Path) -> Result<String, TranscribeError> {
        println!("[Groq STT] 正在识别: {}", path.display());
        // ... API call ...
        println!("[Groq STT] 识别结果: {}", result);
        Ok(result)
    }
}
```

**After:**
```rust
use tracing::{info, instrument};

impl GroqTranscriber {
    #[instrument(name = "groq_stt", skip(self), fields(path = %path.display()))]
    pub fn transcribe(&self, path: &Path) -> Result<String, TranscribeError> {
        info!("开始识别");
        // ... API call ...
        info!(result = %result, "识别完成");
        Ok(result)
    }
}
```

The span automatically records the path field, entry, and (on `TRACE` level) exit with duration.

---

## Summary

| Item | Decision |
|---|---|
| Library | `tracing` + `tracing-subscriber` + `tracing-appender` |
| Level control | `RUST_LOG` env var (primary), optional `--log-level` CLI flag |
| File output | `tracing-appender` rolling daily files in platform data dir |
| Migration order | Smallest files first, `audio.rs` and `main.rs` last |
| Startup banner | Keep as `println!` (intentional UI output) |
| Module spans | Use `#[instrument]` on key public methods |
| Structured fields | Replace `format!` interpolation with `key = value` fields |
