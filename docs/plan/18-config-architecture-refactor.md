# 配置架构与模块边界重构

## 状态

**Implemented，等待最终 CI/review。**

用户已在同一 PR 明确批准简化方案。实现、测试、文档和 `changelog` 均继续使用原 bookmark/PR；完成
本地校验后将该 PR 从 draft 标记为 ready。

## 背景

当前 `AppConfig` 同时承担磁盘结构、默认值、环境变量覆盖、CLI 字段访问和运行时依赖容器等职责：

- 配置文件固定为当前工作目录中的 `config.json`，桌面应用从 Finder、开始菜单或不同目录启动时可能读取
  不同文件。
- 一个字段需要在 `AppConfig`、`Default`、`get_field`、`set_field`、`apply_json`、`main.rs` 和文档中
  重复维护，已经出现字段列表漂移。
- transcriber 和 post-process 直接接收完整 `AppConfig`，可以访问无关字段。
- Local 模式通过 clone/mutate 完整配置并写入假密钥来覆盖 endpoint/model。
- 配置错误有时静默回退默认值，用户难以确认实际生效的文件和值。

重构需要解决这些边界问题，但不为配置系统建立通用依赖注入或 DTO 框架。

## 目标

1. 提供唯一、稳定、跨平台的配置路径，并由同一个 `ConfigStore` 负责 load/save。
2. 只接受 `schema_version = 2` 的 canonical 嵌套配置；不读取、合并或迁移 v1。
3. 将持久化 schema、默认值、CLI 字段目录和格式校验集中在 `core::config`。
4. 让 input、audio、transcriber、postprocess、orchestrator 和 local 各自拥有运行时配置类型与业务校验。
5. 在应用层按 Listener、Convert 和 Local 命令组装运行时配置；consumer 不再接收完整持久化配置。
6. 将 API 与 Local 建模为同层 profile，删除 `main.rs` 中 clone/mutate 完整配置和本地假密钥。
7. 保持现有默认值及正常录音、转写、后处理和文本注入行为；除配置位置和格式外不引入额外行为变更。

## 非目标

- 不实现动态配置热加载、托盘设置 UI、v1 reader、迁移命令或旧字段 alias。
- 不改变 STT/LLM HTTP 协议、重试算法、同步 worker 模型或请求取消行为。
- 不在本计划中修改 WAV header/capacity、frame alignment 或 chunk 调度算法；相关修复另立计划。
- 不在本计划中重构 Local 进程 lease、PID 身份校验、跨进程启动锁或 shutdown ownership；相关修复另立计划。
- 不建立 Rust/Python model/route contract snapshot，也不重构 Python server 常量；相关一致性工作另立计划。
- 不引入 Keychain/Credential Manager 或新的传输安全策略。
- 不创建 generic `ValidateInto<Raw, Validated>`、模块 validator trait、validator registry、`Any` 或 downcast。
- 不为每个 command 或一两个字段预先创建 DTO；只有 consumer 边界或真实不变量需要命名类型。

## 简化后的架构

配置处理分为三层：

```text
canonical path
      │
      ▼
ConfigStore ── read/write ── ConfigDocument
                                  │
                         narrow section references
                                  │
                                  ▼
              module-owned parse/validate functions
                                  │
                                  ▼
                  runtime_config application assembly
                                  │
              ListenerConfig / BackendConfig / Local config
                                  │
                                  ▼
                              consumers
```

依赖方向：

- `platform` 提供当前系统的应用配置目录；它不依赖 `core::config`，也不解释配置文件名或 schema。
- `core::config` 使用该平台能力完成持久化、字段目录、密钥来源和配置错误处理，不 import 业务模块。
- 业务模块只接收与自身相关的 schema section 引用或少量显式参数，不接收完整 `ConfigDocument`。
- 业务模块拥有自己的 validated config 和校验函数。
- `runtime_config.rs` 是应用组装层，可以同时依赖 `core::config` 与各业务模块，负责选择 profile、调用具体
  校验函数、汇总问题并组合顶层运行配置。
- `main.rs` 调用组装入口并把结果交给 consumer，不解释配置字段或业务规则。

不通过 trait registry 反转这些依赖。当前只有一个 production 实现，校验函数是确定性的纯转换；单元测试
直接测试各模块函数，组装测试使用构造好的 section/结果 fixture。以后只有出现第二个真实实现时才引入 trait。

### 目标文件结构

现有 `src/core/config.rs` 收敛为四个文件：

```text
src/core/config/
  mod.rs       — public facade、共享错误与必要的安全 value types
  document.rs  — v2 serde schema、默认值和 ConfigDocument
  fields.rs    — ConfigKey catalog、list/get/set
  store.rs     — 追加 canonical 文件名、load/save 和原子发布
src/runtime_config.rs
               — target/profile 选择、业务校验调用、问题汇总和顶层运行配置
```

平台应用配置目录由现有 `src/platform/macos.rs`、`src/platform/windows.rs` 分别实现，并通过
`platform::config_dir()` 提供统一 crate-private API。当前只有一个路径能力，不为此新增
`platform/macos/`、`platform/windows/` 子目录或独立 `path.rs`。

平台函数只返回 `Option<PathBuf>` 或平台层错误，不返回 `ConfigError`，避免 `platform` 反向依赖
`core::config`。返回值已经包含平台对应的应用目录名：macOS 使用
`com.b1indsight.viberwhisper`，Windows 使用 `ViberWhisper`。`store.rs` 只负责将平台结果转换为
`ConfigError` 并追加 `config.json`。
格式校验随 serde schema 放在 `document.rs`，不再单独创建 `validation.rs`。
业务校验与 validated DTO 位于 owning module，不创建中央 `contracts.rs`。

是否进一步拆文件由实现后的实际大小和职责决定，计划阶段不预设更多层级。

## 统一配置路径

### ConfigStore

```rust
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self, ConfigError>;
    pub(crate) fn at(path: PathBuf) -> Self;
    pub fn path(&self) -> &Path;
    pub fn load(&self) -> Result<ConfigDocument, ConfigError>;
    pub fn save(&self, document: &ConfigDocument) -> Result<(), ConfigError>;
}
```

- `platform::config_dir()` 是生产代码查询应用配置目录的唯一入口；macOS/Windows 在各自现有平台模块中实现。
- `discover()` 是生产代码唯一的路径入口。
- 同一个 store 固化一次路径并同时负责 load/save。
- `at(path)` 供 crate 内测试注入临时路径，不修改 `HOME`、`APPDATA` 等进程级状态。
- 平台配置目录无法解析时由 `discover()` 转换为明确的 `ConfigError`，不回退当前目录。

| 目标 | canonical config path |
|---|---|
| macOS | `~/Library/Application Support/com.b1indsight.viberwhisper/config.json` |
| Windows | `%APPDATA%\ViberWhisper\config.json` |

本地模型、venv、PID、日志以及模型目录自身的 `config.json` 不属于应用配置路径。

### V2-only 文件语义

1. 只检查 target platform canonical path，不探测当前目录的 `./config.json`。
2. canonical path 不存在时返回 v2 默认配置，不创建目录或文件。
3. canonical path 存在时只接受合法 JSON、`schema_version = 2` 和 canonical 结构。
4. 缺少或错误 schema version、v1 平铺 key、unknown field、错误类型和损坏 JSON 均返回可操作错误；不回退
   默认值，也不读取其他文件。
5. `config.example.json` 只是模板，不是隐式运行时配置源。

`save` 在 canonical 同目录写临时文件，flush 后原子替换目标；实现使用
`tempfile::NamedTempFile` 的同文件系统 publish primitive。它按单写者场景设计，不增加文件锁、冲突检测或
并发写入协议。成功后目标是本次完整 v2 文档，写入或发布失败时保留此前文档。

新增只读 `viberwhisper config path`。本计划不增加 `--config` 或环境变量路径覆盖。

## 持久化 Schema v2

v2 使用嵌套配置表达用户可理解的配置分组，并以必填的 `schema_version: 2` 作为唯一可接受格式：

```json
{
  "schema_version": 2,
  "input": {
    "hold_hotkey": "F8",
    "toggle_hotkey": "F9"
  },
  "audio": {
    "mic_gain": 3.0
  },
  "chunking": {
    "max_duration_secs": 30,
    "max_size_bytes": 24117248,
    "max_retries": 3
  },
  "session": {
    "convergence_timeout_secs": 30
  },
  "transcription": {
    "language": "zh",
    "prompt": "以下是一段简体中文的普通话句子，去掉首尾的语气词",
    "temperature": 0.0
  },
  "post_process": {
    "enabled": false,
    "preheat_enabled": true,
    "prompt": null,
    "temperature": 0.0
  },
  "inference": {
    "active": "api",
    "api": {
      "provider": "groq",
      "transcription": {
        "api_url": "https://api.groq.com/openai/v1/audio/transcriptions",
        "model": "whisper-large-v3-turbo"
      },
      "post_process": {
        "api_url": "https://api.openai.com/v1/chat/completions",
        "model": "gpt-4o-mini"
      }
    },
    "local": {
      "data_dir": "~/.viberwhisper",
      "server_port": 17265,
      "quantization": "int8"
    }
  }
}
```

- `inference.api` 与 `inference.local` 是同层 profile；`active` 只选择当前运行 profile。
- 两个 profile 同时持久化，切换模式不会丢失非活动 profile。
- language/prompt/temperature、chunking 和 post-process 行为策略保持公共配置。
- Local model 和 route 继续使用当前既有实现；本重构只停止通过 clone/mutate `AppConfig` 生成 Local backend。
- 持久化 section 是反序列化和校验输入，不直接作为 consumer API。

### 密钥边界

- v2 允许手工配置可选的 `inference.api.transcription.api_key` 和
  `inference.api.post_process.api_key`；canonical example 默认省略它们。
- 只读取 `TRANSCRIPTION_API_KEY` 和 `POST_PROCESS_API_KEY`；不再支持 `GROQ_API_KEY`。
- 环境变量优先于磁盘值，但环境变量产生的密钥不得被 serializer 写入磁盘。
- runtime 使用 inner value 私有且 `Debug`/`Display` 脱敏的 `SecretValue`。
- HTTP consumer 接收 `ApiAuth::None | ApiAuth::Bearer(SecretValue)`；Local 使用 `None`，不再写入假密钥。
- `SecretSource` 是一个窄接口，生产实现读取进程环境，测试使用 map-backed 实现，避免并行测试修改全局环境。
- CLI 对密钥只显示 `Unset | Disk | Environment | EnvironmentOverridesDisk`，不显示长度或内容，也不接受
  `config set` 写入明文密钥。

`ConfigStore::load()` 只读取磁盘文档；`runtime_config` 借用 document 和 secret source 解析 effective secret，
不得修改 document。`ConfigStore::save()` 只接受 `ConfigDocument`，不接受任何 runtime config。

## 字段目录与 CLI

`fields.rs` 定义唯一的 `ConfigKey` catalog。每个字段记录：

- canonical dotted key；
- writable 属性；
- getter/setter 分派。

`schema_version` 进入 catalog，但固定为 read-only metadata。使用一个小型 declarative macro 生成 key、lookup、
list/get/set 分派；持久化 struct 保持普通 Rust 定义，并用 schema-leaf 与 catalog coverage test 防止漂移。

CLI 行为：

- `config path/list/get` 只依赖 `ConfigStore`/`ConfigDocument`，不需要业务校验。
- known-but-unset、unknown key 和 secret status 使用不同结果类型。
- `config set` 在 cloned document 上完成 canonical field 与 primitive 类型解析后直接保存，不做跨字段校验，
  允许用户分步骤完成配置。
- `config check` 组装 active profile 的 listener 配置并报告实际阻止该 workflow 构造的问题。
- CLI 只接受 canonical dotted key；旧平铺 key 返回 unknown key。

## 校验边界

### core::config 拥有的校验

- JSON 语法、`schema_version = 2`、object shape、unknown field 和 serde primitive type。
- canonical key、read-only/secret/writable 属性和 CLI 字符串到 primitive 的解析。
- 浮点值必须是可持久化的 finite JSON number。
- 默认值、密钥来源、脱敏和环境密钥不落盘。
- canonical path 和 active profile 枚举的格式。

### 业务模块拥有的校验

| owner | 输入 | 输出与规则 |
|---|---|---|
| input | `&InputSection` | `HotkeyConfig`；非空值为 F1–F12、hold/toggle 互异；两者均为空时使用纯托盘控制 |
| audio | `&AudioSection`, `&ChunkingSection` | infallible `AudioConfig` / `ChunkLimits` 投影，不添加推测性范围规则 |
| transcriber | selected profile、transcription/chunk sections、optional auth | `TranscriberConfig`；可构造的 URL、非空 model、既有 retry 上限 |
| postprocess | selected profile、post-process section、optional auth | `PostProcessConfig`；enabled 条件下需要可构造的 URL 和非空 model |
| orchestrator | `&SessionSection`, language | `OrchestratorConfig`；保持现有 convergence timeout 规则 |
| local | `&LocalSection`, config/home context | `LocalPaths` / `LocalServiceConfig`；路径、port、quantization |

每个模块使用普通函数或具体类型的 `TryFrom`/constructor：

```rust
pub(crate) fn validate_config(
    section: &InputSection,
) -> Result<HotkeyConfig, Vec<ValidationIssue>>;
```

不要求所有模块套用同一个 generic trait。模块可以一次返回多个问题；`runtime_config` 合并各函数结果，按
`(ConfigKey, issue code)` 稳定排序。存在问题时不返回部分顶层配置。

需要真实资源才能判断的规则继续在 owning module 的实际操作边界检查；`config check` 不探测麦克风、网络、
文件存在性、进程或模型状态。

### 错误语义

- 已存在但无法读取或解析的配置 fail closed，不再静默使用默认值。
- unknown canonical field 明确报错。
- API authentication 可选；未提供密钥时不发送 `Authorization` header，由兼容端点决定是否接受请求。
- 用户启用 post-process 但配置不完整时返回配置错误；运行期间 LLM 请求失败仍保持现有 soft-fail 行为。
- `local stop` 等恢复命令只校验实际使用的 local path，不因未使用的 API profile 或 quantization 错误而失败。

## 运行时配置与 DTO 粒度

### 保留的类型

只保留两类命名类型：

1. 表达真实不变量或安全边界的 value object，例如 `SecretValue`、`ApiAuth`、`ChunkLimits`、`LocalPaths`。
2. 与实际 consumer/workflow 边界对应的配置，例如 `HotkeyConfig`、`AudioConfig`、`TranscriberConfig`、
   `PostProcessConfig`、`OrchestratorConfig`、`LocalServiceConfig`。

这些 validated config 由 owning module 定义。简单 schema section 直接作为校验输入，不复制成
`RawHotkeyConfig`、`RawChunkLimits` 等一一对应的 raw DTO。只有实现中出现多来源组合且函数参数已经不清晰时，
才在 owning module 内增加 candidate；计划不预先规定 candidate 层。

Local 命令复用 `LocalPaths` 和 `LocalServiceConfig`，不创建只有一个字段的 `LocalInstallConfig`、
`LocalStopConfig`、`LocalStatusConfig` 或 `LocalRepositorySpec`。installer、service manager 和 status handler
从这两个类型借用各自所需字段。

### 顶层运行配置

```rust
pub struct ListenerConfig {
    pub hotkeys: HotkeyConfig,
    pub audio: AudioConfig,
    pub orchestrator: OrchestratorConfig,
    pub backend: BackendConfig,
}

pub struct BackendConfig {
    pub transcriber: TranscriberConfig,
    pub post_process: PostProcessConfig,
    pub local_service: Option<LocalServiceConfig>,
}
```

API/Local 是组装时的 profile 选择，不是 consumer 的运行时分支。两条路径都产生相同的 transcriber 与
post-process 依赖；只有 Local 需要额外管理 service，因此用 `Option<LocalServiceConfig>` 表达该差异，
不重复字段或增加投影 getter。

`convert` 直接接收 `BackendConfig`：offline splitter 已由 `TranscriberConfig` 中的 `ChunkLimits` 驱动，额外的
`ConvertConfig` 只会重复表达同一数据。因此不创建单字段 wrapper，也不额外创建 `ResolvedBackendConfig`
与 `ActiveInference` 两层包装。

目标 consumer：

| consumer | 重构后接收 |
|---|---|
| `HotkeyManager` | `HotkeyConfig` |
| `AudioRecorder` | `AudioConfig` |
| offline splitter | `ChunkLimits` |
| `ApiTranscriber` | `TranscriberConfig` |
| `SessionOrchestrator` | `OrchestratorConfig` + transcriber |
| post-process facade / LLM | `PostProcessConfig` |
| local installer | `&LocalPaths` |
| local service/status | `&LocalServiceConfig` 或 `&LocalPaths` |

## API 与 Local 解析

应用层提供少量显式入口：

```text
resolve_listener(Configured | Local)       -> ListenerConfig
resolve_convert()                           -> BackendConfig
resolve_local_paths()                       -> LocalPaths
resolve_local_service()                     -> LocalServiceConfig
check()                                     -> ValidationReport
```

- persisted listener/convert 根据 `inference.active` 选择 API 或 Local。
- `local start` 使用一次性 `LocalOverride`，但不修改或保存 persisted active profile。
- API profile 从配置和 effective secrets 构造 consumer config。
- Local profile 从既有 Local model/route 常量构造 loopback consumer config，auth 使用 `ApiAuth::None`。
- 两条路径都直接构造 `BackendConfig`，不 clone/mutate `ConfigDocument`。
- Local service 的启动、复用和释放继续保持当前行为；本计划只改变它接收配置的方式。

## V2-only clean break

| 旧输入 | v2-only 行为 |
|---|---|
| 当前目录 `./config.json` | 不探测、不读取、不复制 |
| canonical path 中的平铺 JSON 或缺少 `schema_version` | 返回 `UnsupportedSchema` |
| `schema_version != 2` | 返回 `UnsupportedSchema`，不尝试版本 dispatch |
| `hotkey`、`local_mode` 等旧 CLI key | 返回 unknown key |
| `GROQ_API_KEY` | 不读取；只支持 `TRANSCRIPTION_API_KEY` |
| 旧文件与 canonical v2 同时存在 | 只看 canonical v2，不比较、不合并 |

writer 永远只输出 v2 canonical nested key；环境变量密钥不写入磁盘。发布说明明确标为 breaking change，
用户需要自行在 `config path` 显示的位置创建新文件。

`inference.local.data_dir` 为 null 时使用 `~/.viberwhisper`。local 模块负责展开 `~/...`；普通相对路径固定
相对 canonical config directory 解析，不依赖运行时 cwd。该过程只做 lexical normalization，不要求目录存在。

## 文件影响范围

| 文件 | 计划变更 |
|---|---|
| `Cargo.toml`, `Cargo.lock` | 增加同目录原子发布使用的 `tempfile` 直接依赖 |
| `src/core/config.rs` | 由四文件 `src/core/config/` 目录替代 |
| `src/platform/mod.rs`, `macos.rs`, `windows.rs` | 提供统一应用 `config_dir()` API 和各平台实现，不依赖 config error/schema |
| `src/runtime_config.rs` | 新增应用层 target/profile 解析、校验汇总和顶层配置组装 |
| `src/core/cli.rs` | 增加 `config path/check`，CLI 使用 canonical catalog |
| `src/main.rs` | 使用组装入口，删除字段清单、完整配置传递和 Local clone/mutate override |
| `src/input/hotkey.rs` | 定义并校验 `HotkeyConfig` |
| `src/audio/recorder.rs`, `splitter.rs` | 接收 `AudioConfig` / `ChunkLimits`；不改变 chunk 算法 |
| `src/transcriber/api.rs` | 定义并接收 `TranscriberConfig` / `ApiAuth` |
| `src/core/orchestrator.rs` | 定义并接收 `OrchestratorConfig` |
| `src/postprocess/mod.rs`, `llm.rs` | 定义并接收 `PostProcessConfig` |
| `src/local/*` | 定义 `LocalPaths` / `LocalServiceConfig`，保持现有 service lifecycle |
| `config.example.json` | 保持无密钥 canonical v2 示例 |
| `README.md`, `docs/architecture/*`, `docs/README.md`, `changelog` | 实现完成时更新用户和架构文档 |

本计划不修改 Python server、packaging workflow 或 GitHub Actions 行为。

## TDD 实施顺序

计划批准后严格测试先行。

### Phase 1：ConfigDocument 与 ConfigStore

先写测试：

- v2 defaults、完整 JSON round-trip、unknown field 拒绝。
- 缺少/错误 schema version、平铺 v1、损坏 JSON fail closed。
- canonical 不存在时返回默认值且不创建目录。
- 只读取注入的 canonical path，不探测临时 cwd 中的旧 `config.json`。
- platform facade 在 macOS/Windows 返回各自完整应用配置目录；config store 只负责追加 `config.json`。
- 平台目录不可用时由 store 转换为配置错误，且 platform 模块不依赖 `core::config`。
- save 创建父目录，写入/发布失败时保留此前完整文档。
- macOS/Windows CI 分别验证路径后缀。

再实现 `document.rs`、`store.rs` 和必要错误类型。

### Phase 2：字段目录、CLI 与密钥

先写测试：

- catalog key 唯一、schema leaf 全覆盖、旧 key unknown。
- list/get/set、known-unset、read-only 和 primitive parse。
- secret 的 disk/environment/override/unset 状态和全链路脱敏。
- env-only secret 经 load、普通字段修改、save、reload 后不落盘。
- `config.example.json` 可由 strict v2 decoder 解析且无密钥。

再实现 `fields.rs`、`SecretSource` 和 CLI 的 path/list/get/set 基础路径。

### Phase 3：模块配置与业务校验

按模块逐个先写 raw section 到 validated config 的测试，再实现最小转换：

1. input：hotkey。
2. audio：gain 与既有 chunk limits 语义。
3. transcriber：API/Local URL、auth、model、retry。
4. postprocess：disabled/enabled 组合。
5. orchestrator：convergence timeout。
6. local：路径、port、quantization。

每组测试只覆盖该模块拥有的规则。consumer 构造器随后改为接收对应 validated config，不再接收
`AppConfig` 或多个散落标量。

### Phase 4：应用组装与集成

先写测试：

- listener API、listener Local override、convert API/Local 产生正确顶层配置。
- Local backend 使用 `ApiAuth::None`，API backend 有 effective secret 时使用 Bearer，否则也使用 `ApiAuth::None`。
- 多模块错误能够聚合、排序；失败时不返回部分顶层配置。
- `config set` 只做字段存在性、read-only 和 primitive 类型解析，可分步骤写入尚未完整的业务配置。
- `config check` 只组装 active profile；path/list/get/set 不触发未使用 profile 的业务校验。
- local stop 只要求可解析的 local path，不受无关 profile 错误阻塞。
- main 不再 clone/mutate persisted config 或写入假密钥。

再实现 `runtime_config.rs`，迁移 main/listener/convert/local CLI composition，删除旧 `AppConfig` consumer API。

### Phase 5：文档与验证

1. 更新 architecture docs、README、本文状态和 `changelog`，移除 v2 preview 警告。
2. 验证 macOS 从 Finder、Windows 从 Start Menu、CLI 从不同 cwd 使用同一配置路径。
3. 运行 `cargo fmt --check`、`cargo check`、`cargo test`、`cargo clippy -- -D warnings`、
   `uv run ruff check server` 和 `uv run pytest`。
4. 检查最终 `jj diff`，推送到同一 bookmark/PR，完成后才将 draft PR 标记 ready。

## 验收标准

- [x] `platform::config_dir()` 是应用配置目录的唯一入口，各平台目录名在现有 platform 模块中维护。
- [x] `ConfigStore::discover()` 组合 canonical file path，load/save 使用同一路径且不重复平台判断。
- [x] platform 层不依赖 `core::config`；目录解析错误由 store 转换为 `ConfigError`。
- [x] 只接受 canonical v2；不包含 v1 reader、旧 cwd probe、迁移或 CLI alias。
- [x] 缺少/错误 schema、unknown field、损坏 JSON 不静默回退默认值。
- [x] `core::config` 只包含四个计划文件，不包含业务 validator registry 或中央 DTO contracts 文件。
- [x] 持久化 schema、默认值、字段 catalog 和格式校验只在 `core::config` 维护。
- [x] 每个业务模块拥有自己的 validated config 和业务规则；consumer 不导入完整 `ConfigDocument`。
- [x] 简单 schema section 不复制为一一对应的 raw DTO；单字段 command wrapper 不存在。
- [x] `runtime_config` 通过具体函数完成 profile 选择、校验汇总和顶层组装，不使用 generic validator trait。
- [x] `ListenerConfig` 和 `BackendConfig` 足以表达顶层 workflow，无重复包装层。
- [x] API 与 Local 是同层 profile；Local override 不修改持久化 document，也不使用假密钥。
- [x] CLI 字段目录不存在 main 手写副本；path/check/list/get/set 使用 canonical dotted key。
- [x] 环境密钥优先但不落盘，CLI/Debug/Display/错误信息不泄密。
- [x] Local service lifecycle、WAV chunk 算法和 Python server 行为保持本次重构前语义。
- [x] config example、README、architecture docs 和 changelog 与实现一致。
- [x] 所有测试不依赖真实 API、麦克风、模型下载或 Python inference server。
- [ ] macOS/Windows CI 和全部 Rust/Python 校验通过。

## 风险与控制

### 破坏性 v2 切换

README、启动错误和 release notes 明确说明新路径与格式；提供 `config path` 和完整 example。应用不读取或删除
旧 cwd 文件，避免产生部分迁移的错觉。

### 应用组装层变大

`runtime_config.rs` 只选择 profile、调用具体校验函数和组合结果，不保存业务常量或实现业务规则。若实现后
文件因真实职责增长，再按 target 拆分；不在计划阶段预先抽象。

### 业务规则重复

边界测试放在 owning module；应用组装测试只检查调用结果和组合，不重复每条范围规则。consumer 只接收
validated config，不再自行解析字段。

### 范围再次扩张

Local PID/lease、并发启动、WAV capacity/frame alignment 和跨语言 model contract 都不作为本 PR 的验收条件；
发现相关问题时记录为后续独立计划，不在配置重构中顺带实现。

## 审批与同一 PR 工作流

1. 本 plan-only change 使用现有 `refactor/config-architecture` bookmark 和 draft PR #80。
2. 用户通过该 PR 的 review/comment 明确批准简化后的计划；PR 保持打开，不合并、不关闭。
3. 批准后在同一工作副本按上述 TDD phases 实现。
4. 实现、文档和 `changelog` 推送到同一 bookmark，更新同一个 PR。
5. 全部验收完成后才将 PR 标记 ready，等待最终 review 和合并。
