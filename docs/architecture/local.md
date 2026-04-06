# Local Runtime Architecture

## Purpose

`local` 模块负责把 Python FastAPI 推理服务接入 Rust 主程序，使 ViberWhisper 可以在本机使用 Gemma 4 完成转写和文本后处理，而不依赖外部云端 API。

## Module Layout

```
src/local/
  installer.rs  — Python 虚拟环境、依赖安装、模型下载与校验
  service.rs    — 本地服务进程管理、健康检查、PID/日志状态

server/
  server.py     — FastAPI 服务，提供 OpenAI 兼容接口
  requirements.txt
  tests/test_server.py
```

## Runtime Flow

1. `main.rs` 解析到 `local` 子命令，或检测到 `config.local_mode = true`
2. `ensure_local_install()` 准备 `venv`、Python 依赖、Gemma 权重并做安装校验
3. `LocalServiceManager::start()` 启动 `server/server.py`
4. 健康检查通过后，`apply_local_endpoint_overrides()` 将转写端点改写到 `http://127.0.0.1:<port>`；若启用了后处理，再同时改写 chat completions 端点
5. 之后主流程继续复用现有 `Transcriber` 和 `TextPostProcessor`，无需额外分支

## Installer (`src/local/installer.rs`)

### Responsibilities

- `setup_venv(venv_dir)`: 创建 Python 虚拟环境；优先使用 `PYTHON` 环境变量指定的解释器，否则按平台寻找 `python3` / `python`
- `install_requirements(venv_dir, reqs)`: 安装 [`server/requirements.txt`](../../server/requirements.txt) 中的运行时依赖
- `download_model(model_dir, hf_endpoint)`: 通过 `huggingface_hub` 下载 `google/gemma-4-E4B-it`
- `verify_install(venv_dir, model_dir)`: 校验 Python 可执行文件和模型目录是否存在且非空
- `dependencies_installed(venv_dir)`: 通过导入关键包确认依赖已安装
- `model_weights_present(model_dir)`: 检测 `config.json` 与至少一个 `.safetensors` / `.bin` 权重文件

### Data Layout

默认目录为 `~/.viberwhisper/`：

- `venv/`: Python 虚拟环境
- `model/`: Gemma 模型权重
- `local_server.pid`: 服务 PID
- `server.log`: Python 服务 stderr 日志

## Service Manager (`src/local/service.rs`)

### `LocalServiceManager`

```rust
pub struct LocalServiceManager {
    port: u16,
    model_dir: PathBuf,
    venv_dir: PathBuf,
    quantization: String,
    process: Option<Child>,
    log_file: Option<PathBuf>,
    owned: bool,
}
```

### Key Behaviors

- `start()`: 如果已有健康服务则直接复用，否则启动 Python 进程并轮询 `/health`
- `stop()`: 读取当前子进程或 PID 文件，对目标进程发送终止信号；超时后升级为强制 kill
- `release()`: 仅在当前 manager 自己启动了服务时才停止，避免误杀复用中的后台进程
- `status()`: 返回 `running`、`pid`、`memory_usage` 和最近一次健康检查结果
- `base_url()`: 统一生成 `http://127.0.0.1:<port>`

### Process Management

- PID 会写入 `local_server.pid`
- 非 Windows 平台通过 `ps` / `kill` 检查和结束进程
- Windows 通过 `tasklist` / `taskkill`
- 健康检查超时时间为 120 秒，轮询间隔 500 ms

## FastAPI Server (`server/server.py`)

### Startup

- `create_app(runtime)` 在 lifespan 中异步调用 `runtime.load()`
- 模型加载完成前，`/health` 返回 `503 loading`
- 加载失败后返回 `500 error`

### Exposed Endpoints

| Endpoint | Purpose |
|---|---|
| `GET /health` | 返回 `loading` / `ok` / `error` 状态 |
| `POST /v1/audio/transcriptions` | 接收 multipart WAV 音频，返回 `{text, language, duration}` |
| `POST /v1/chat/completions` | 接收 OpenAI 风格 chat 请求，返回 chat completion 响应 |

### `LocalModelRuntime`

- `load()`: 加载 `AutoProcessor` 与 `AutoModelForCausalLM`
- `_try_quanto_quantization()`: 优先使用 `optimum-quanto`，不可用时回退到 `bitsandbytes`，再不行则无量化加载
- `transcribe_audio()`: 读取 WAV 字节流，最长支持 30 秒，构造带音频输入的 chat prompt
- `chat_complete()`: 渲染对话模板并生成文本，供后处理模块复用
- `_flatten_content()`: 兼容字符串或 OpenAI content array 结构

## Testing

- Rust 侧单元测试覆盖 installer 和 service manager 的关键路径
- Python 侧 [`server/tests/test_server.py`](../../server/tests/test_server.py) 使用 `fastapi.testclient` 验证健康检查、转写接口和 chat 接口
- Python 项目元数据位于 [`pyproject.toml`](../../pyproject.toml)，可通过 `uv run pytest` 运行服务测试
