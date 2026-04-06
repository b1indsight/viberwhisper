# Plan 12: Local Gemma 4 E4B Inference Service

## Background

ViberWhisper currently relies on cloud APIs (Groq/OpenAI-compatible) for both STT and
LLM post-processing. This plan adds a fully local inference path using Google's
**Gemma 4 E4B-it** multimodal model, which supports text, image, audio, and video
input natively.

Primary target hardware: **Apple Silicon Mac Mini (16 GB unified memory)**.
Windows support is included as a second-class target for the same code path.

### Why a Python server (not llama.cpp)?

llama.cpp has day-0 support for Gemma 4 text and image inference, but audio input is
not yet implemented ([ggml-org/llama.cpp#21325](https://github.com/ggml-org/llama.cpp/issues/21325)).
Ollama has the same gap. The only runtime that supports Gemma 4 audio today is
**HuggingFace `transformers`** (the canonical implementation from Google).

The plan uses a thin Python FastAPI server wrapping `transformers`, exposing an
OpenAI-compatible HTTP API. The Rust side manages the server as a child process and
calls the existing `ApiTranscriber` / `LlmPostProcessor` unchanged — the only new
Rust code is process lifecycle management and installer CLI.

When llama.cpp audio support lands, the Python server can be swapped for llama.cpp
server behind the same endpoints with zero changes to the rest of the Rust codebase.

---

## Architecture

```
viberwhisper (Rust)
  │
  ├── viberwhisper local install   ── downloads model + sets up venv
  ├── viberwhisper local start     ── spawns Python server, then starts main loop
  ├── viberwhisper local stop      ── kills Python server
  └── viberwhisper local status    ── checks process + HTTP health
         │
         ▼
  LocalServiceManager (src/local/service.rs)
    └── spawn: python server.py --port 8765
               │
               ├── POST /v1/audio/transcriptions   ← ApiTranscriber (zero changes)
               └── POST /v1/chat/completions        ← LlmPostProcessor (zero changes)
```

The server loads Gemma 4 E4B-it once at startup and handles both endpoints with the
same model instance.

---

## Model

| Property | Value |
|---|---|
| Model | `google/gemma-4-E4B-it` |
| HuggingFace repo | `ggml-org/gemma-4-E4B-it-GGUF` (GGUF, future) / `google/gemma-4-E4B-it` (safetensors, now) |
| Effective parameters | 4.5 B (8 B with embeddings) |
| Precision | `bfloat16` → ~8–9 GB weights |
| Quantization (default) | `int8` via `optimum-quanto` → ~5 GB weights, ~8 GB total |
| Fits in 16 GB unified memory | Yes (int8), comfortable |
| Audio input max | 30 seconds per call |

---

## New Files

```
server/
  server.py            — FastAPI inference server (STT + chat completions)
  requirements.txt     — Python dependencies

src/
  local/
    mod.rs               — pub re-exports
    service.rs           — LocalServiceManager (spawn, health-check, stop)
    installer.rs         — setup_venv(), download_model(), verify_install()
```

### Changes to existing files

| File | Change |
|---|---|
| `src/core/config.rs` | Add `local_*` config fields (see Config section) |
| `src/core/cli.rs` | Add `local` subcommand with `install / start / stop / status` |
| `src/main.rs` | Wire `local` subcommand dispatch |
| `config.example.json` | Add local mode example block |

---

## Python Server (`server/server.py`)

### Endpoints

**`GET /health`**
Returns `{"status": "ok", "model": "gemma-4-E4B-it"}` once the model is loaded.
Returns 503 while loading.

**`POST /v1/audio/transcriptions`** (multipart/form-data)
Accepts the same fields as the OpenAI Whisper endpoint:
- `file` — WAV audio file (≤ 30 s enforced by chunking on the Rust side)
- `language` (optional)
- `prompt` (optional, prepended as text context)

Returns: `{"text": "...", "language": "...", "duration": 12.3}`

The handler decodes the WAV bytes with `soundfile`, builds a Gemma 4 multimodal
message with `{"type": "audio", "audio": samples}`, runs inference, and extracts the
transcription from the generated text.

**`POST /v1/chat/completions`**
Standard OpenAI chat completions (text only). Used by the existing `LlmPostProcessor`.
Supports `stream: false` only in this initial implementation (streaming is a stretch
goal, see below).

### Dependencies (`requirements.txt`)

```
torch>=2.3
transformers>=4.51
fastapi>=0.111
uvicorn[standard]>=0.29
soundfile>=0.12
optimum-quanto>=0.2   # int8 quantization on MPS / CUDA / CPU
huggingface_hub>=0.23
```

### Startup sequence

1. Parse `--model-dir`, `--port`, `--quantization` (int8 / bf16) args.
2. Load `AutoProcessor` and `Gemma4ForConditionalGeneration` from the local model dir.
3. Apply int8 quantization if requested (default on ≤ 24 GB unified memory).
4. Move model to `mps` (macOS Apple Silicon), `cuda` (Windows + GPU), or `cpu`.
5. Set `GET /health` to return 200 and begin accepting requests.

---

## Rust: `LocalServiceManager` (`src/local/service.rs`)

```rust
pub struct LocalServiceManager {
    port: u16,
    model_dir: PathBuf,
    venv_dir: PathBuf,
    process: Option<std::process::Child>,
}

impl LocalServiceManager {
    pub fn start(&mut self) -> Result<(), Error>;      // spawn + wait for /health
    pub fn stop(&mut self);                             // SIGTERM / TerminateProcess
    pub fn is_running(&self) -> bool;
    pub fn base_url(&self) -> String;                  // "http://127.0.0.1:{port}"
}
```

`start()` polls `GET /health` with 500 ms intervals for up to 120 s (model load time)
before returning an error.

---

## Rust: `Installer` (`src/local/installer.rs`)

```rust
pub fn setup_venv(venv_dir: &Path) -> Result<(), Error>;
pub fn install_requirements(venv_dir: &Path, reqs: &Path) -> Result<(), Error>;
pub fn download_model(model_dir: &Path, hf_endpoint: &str) -> Result<(), Error>;
pub fn verify_install(venv_dir: &Path, model_dir: &Path) -> Result<(), Error>;
```

`download_model` uses `huggingface_hub` Python CLI (`huggingface-cli download`) via
the venv's Python interpreter. This avoids adding a large Rust HF download crate and
gives progress output for free.

HF endpoint is configurable via `HF_ENDPOINT` env var for mirror support
(e.g. `https://hf-mirror.com`).

---

## CLI (`viberwhisper local <subcommand>`)

```
viberwhisper local install   Download model and set up Python environment.
                             Safe to re-run; skips steps already done.

viberwhisper local start     Start the inference server, then start the main
                             recording loop using local endpoints.

viberwhisper local stop      Stop the background inference server.

viberwhisper local status    Print server process status, port, memory usage,
                             and last health-check result.
```

`start` implicitly runs `install` if the model is not yet present.

When the local server is active, `start` overrides config at runtime:
- `transcription_api_url` → `http://127.0.0.1:{port}/v1/audio/transcriptions`
- `post_process_api_url` → `http://127.0.0.1:{port}/v1/chat/completions`
- `post_process_enabled` → `true`
- `post_process_model` → `gemma-4-E4B-it`

No changes to `config.json` are made; overrides are in-memory only.

---

## Config Changes (`src/core/config.rs`)

New optional fields (all `skip_serializing_if = "Option::is_none"` or with sane defaults):

```rust
/// "api" (default) or "local". Selects the STT+LLM backend.
pub local_mode: bool,                          // default: false

/// Directory for model weights and Python venv. Default: ~/.viberwhisper
pub local_data_dir: Option<String>,

/// Port for the local inference server. Default: 8765
pub local_server_port: u16,

/// Quantization level: "int8" (default) or "bf16"
pub local_quantization: String,
```

---

## Implementation Steps

### Step 1 — Python server (TDD)

1. Write `server/requirements.txt`
2. Write `server/server.py` with all three endpoints
3. Manual integration test: `python server.py --model-dir /path/to/model`
   - Test `/health` returns 200 after load
   - Test `/v1/audio/transcriptions` with a 5-second Chinese WAV clip
   - Test `/v1/chat/completions` with a simple cleanup prompt

### Step 2 — Rust `local` module

1. `src/local/mod.rs`
2. `src/local/service.rs` — `LocalServiceManager` with unit tests using a mock HTTP stub
3. `src/local/installer.rs` — venv + download helpers with integration test behind
   `#[cfg(feature = "integration")]`

### Step 3 — Config fields

1. Add fields to `AppConfig`
2. Update `apply_json`, `get_field`, `set_field`, `Default`
3. Extend existing config tests

### Step 4 — CLI wiring

1. Add `LocalCommand` enum to `cli.rs`
2. Dispatch in `main.rs`
3. `install` and `start` share `LocalServiceManager`

### Step 5 — Documentation

1. Update `config.example.json` with local mode block
2. Update `changelog`

---

## Migration Path to llama.cpp (Phase 2)

When [ggml-org/llama.cpp#21325](https://github.com/ggml-org/llama.cpp/issues/21325)
is resolved:

1. Add `local_backend: "python" | "llama_cpp"` config field
2. Implement `LlamaCppServiceManager` alongside `LocalServiceManager`
3. `installer.rs` gains llama.cpp binary download + GGUF model download
4. `server.py` becomes optional — the Python path remains for fallback

No changes to `ApiTranscriber`, `LlmPostProcessor`, or the main recording loop.

---

## Open Questions (to resolve during implementation)

1. **Gemma 4 audio processor API**: The exact `transformers` call signature for passing
   raw audio samples to `Gemma4ForConditionalGeneration` needs to be verified against
   the model card examples and the upstream processor code.

2. **`optimum-quanto` MPS support**: Verify int8 quantization works correctly on
   Apple Silicon MPS backend. Fallback: load in `bfloat16` (~9 GB, fits in 16 GB).

3. **Windows Python path**: `python3` vs `python` executable name, venv activation
   path differs (`Scripts/` vs `bin/`). Handle in `installer.rs`.

4. **Process persistence across app restarts**: Should the server keep running after
   `viberwhisper` exits? Initial answer: yes, managed via a PID file in
   `local_data_dir`. `start` reuses an already-running server; `stop` kills it.
