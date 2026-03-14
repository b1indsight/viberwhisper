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

## 后续扩展

后续如果要接入更多 provider（例如 OpenAI / 其他兼容接口），只需要：

1. 新增对应 transcriber 实现文件
2. 在 `factory.rs` 中加入一个 match 分支
3. 为新 provider 增加配置项或鉴权字段
4. 更新文档与测试

这样可以把多模型支持继续扩展成“多 provider + 多 model”，而不会把业务逻辑重新塞回 `main.rs`。
