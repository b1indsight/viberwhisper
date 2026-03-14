# 多模型支持 (provider + model)

## 目标

为转写模块引入 `provider + model` 配置结构，消除 `main.rs` 对具体转写实现的硬编码，同时保持当前 Groq 默认行为不变，并兼容旧配置里的 `model` 字段。

## 背景

当前实现只支持 Groq，且初始化逻辑直接写在 `main.rs` 中：

- 配置层只有 `model`
- 转写器选择写死为 `GroqTranscriber::from_config(...)`
- 后续若新增 provider，会把配置、CLI 和主流程越改越散

因此这次先做架构收口，把“选哪个 provider / 用哪个 model”收进统一配置和工厂函数里。

## 需求

1. **配置结构升级**
   - 新增 `provider` 字段，默认值为 `groq`
   - 保留 `model` 字段，表示当前 provider 下使用的模型
   - 旧配置文件未声明 `provider` 时，自动按默认值 `groq` 处理

2. **CLI 配置能力同步**
   - `config list` 能展示 `provider`
   - `config get provider` / `config set provider groq` 可正常工作
   - `model` 的读写行为保持不变

3. **转写器初始化解耦**
   - 新增统一工厂函数 `create_transcriber(&AppConfig)`
   - `main.rs` 和 `convert` 流程只依赖工厂，不直接依赖 `GroqTranscriber`
   - 未识别 provider 或 provider 初始化失败时，回退到 `MockTranscriber`

4. **文档与测试**
   - 更新架构文档，明确 `provider + model` 的语义
   - 补充配置兼容、provider 选择和 CLI 行为测试

## 实现方案

### 配置层

在 `src/core/config.rs` 中新增：

```rust
pub provider: String,
```

默认值：

```rust
provider: "groq".to_string()
```

同时更新以下逻辑：

- `Default`
- `get_field`
- `set_field`
- `apply_json`
- `config list` 展示项

### 转写器工厂

新增文件：`src/transcriber/factory.rs`

职责：

- 根据 `config.provider` 构造对应的 `Box<dyn Transcriber>`
- 当前仅真实支持 `groq`
- 未知 provider 统一回退到 `MockTranscriber`

示意：

```rust
match config.provider.as_str() {
    "groq" => { ... }
    _ => Box::new(MockTranscriber),
}
```

### 主流程改造

`src/main.rs` 中两处转写器初始化统一改为：

- `run_listener`
- `handle_convert`

都通过：

```rust
let transcriber = create_transcriber(&config);
```

这样后续新增 provider 时，不需要再修改主流程。

## 文件变更

| 文件 | 变更 |
|------|------|
| `src/core/config.rs` | 新增 `provider` 配置及兼容逻辑 |
| `src/transcriber/factory.rs` | 新增 transcriber 工厂 |
| `src/transcriber/mod.rs` | 导出 `create_transcriber` |
| `src/main.rs` | 改为通过工厂创建 transcriber |
| `docs/architecture/core.md` | 补充 `provider` 字段说明 |
| `docs/architecture/transcriber.md` | 补充工厂模式与扩展点 |
| `README.md` | 更新配置说明 |

## 测试计划

- [x] 默认配置下 `provider == "groq"`
- [x] 旧配置缺少 `provider` 时仍能正常加载
- [x] `config get/set provider` 正常工作
- [x] 已知 provider 能构造对应 transcriber
- [x] 未知 provider 会回退到 `MockTranscriber`
- [x] `cargo test` 通过

## 后续扩展（已进一步抽象）

在原有工厂模式基础上，已将 `GroqTranscriber` 重构为通用的 `ApiTranscriber`，实现以下进一步解耦：

- `GroqTranscriber` → `ApiTranscriber`，通过 `api_key` + `transcription_api_url` + `model` 初始化，不再硬编码 provider 名称
- `factory.rs` 不再 match `config.provider`，而是直接尝试从 config 构造 `ApiTranscriber`
- 配置字段 `groq_api_key` → `api_key`（旧字段保持兼容），新增 `transcription_api_url` 字段
- 旧环境变量 `GROQ_API_KEY` 继续生效（向后兼容），新增 `TRANSCRIPTION_API_KEY`

如果后续要接入格式不兼容的 provider：

1. 新增对应 transcriber 实现文件
2. 在 `factory.rs` 中增加选择逻辑（可基于 `transcription_api_url` 特征或新增 config 字段）
3. 更新文档与测试
