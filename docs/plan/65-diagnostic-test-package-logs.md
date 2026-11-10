# 65 - 测试包诊断日志 — Feature record

**Stage:** Implemented；普通/诊断模式本地验证完成。远端 CI 与完整原生打包结果以 PR 和发布流程为准；未授权触发打包或发布。
**PR:** [#171](https://github.com/b1indsight/viberwhisper/pull/171)，`feat/diagnostic-test-logs` → `master`，计划和实现沿用同一 PR。

## 1. Initial plan

### Summary and motivation

用户在[本线程](https://ampcode.com/threads/T-01a1c463-98b2-77ab-bb72-cf352749e3dd)提出：“我希望在测试打包增加debug log，输出log到目录，最好包含发送给模型的prompt和模型返回结果”。

当前入口的 `init_tracing()` 仅将日志写到 stderr，默认 `viberwhisper=info`，没有文件 sink，见 [`src/core.rs`](../../src/core.rs)。Windows GUI 启动还会将 stdout/stderr 重定向到 NUL，见 [`prepare_desktop_output`](../../src/platform/windows.rs)。HTTP STT、HTTP LLM 和 Realtime 通常只记录文字长度或安全错误类别，因此双击运行测试包后缺少可收集的模型输入、输出证据。

提议增加显式启用的诊断测试包：保持 release 优化，自动写本地 DEBUG 日志，并记录实际模型文本输入和返回内容。普通构建保持现有行为。历史[日志整理](42-structured-runtime-logging.md)和[隐私修复](63-audit-fixes.md)的默认保护不变。

规划前已执行 `jj git fetch --remote origin`，本地基线和 `master@origin` 均为 [`34288ff9`](https://github.com/b1indsight/viberwhisper/commit/34288ff9)，工作区原本干净。方案草稿经本机 Claude Code 的 `claude-opus-5-5` 咨询；初始化和响应均确认该模型、工具及 MCP 列表为空、最终成功。主线程已结合源码核对并简化建议；咨询没有运行验证。

### Goals, scope, and success criteria

- 诊断包双击启动即可生成日志，不要求测试者从终端设置环境变量。
- 覆盖 HTTP STT、HTTP LLM 整理及其预热请求、Realtime 听写；记录最终发送的提示词及文本上下文，不只记录配置模板。
- 同一次请求和响应可以配对；HTTP 重试、并发预热及 Realtime 分段不会混淆。
- 普通构建不创建诊断文件，设置 `RUST_LOG=debug` 也不能开启新增的正文记录；CLI stdout 继续只承载命令结果。
- 不记录原始 WAV/PCM、base64 音频、Authorization header 或完整配置，不新增上传、遥测、日志 UI、配置 schema 或模型调用。
- 本次不直接发布版本、不修改个人配置/记忆/历史，也不启动个人桌面监听器收集真实语音。

### Proposed behavior

#### 启用与打包

[`Cargo.toml`](../../Cargo.toml) 增加非默认 feature `diagnostic-logs`；本地构建使用 `cargo build --release --locked --features diagnostic-logs`。这是优化后的测试包，不依赖 `debug_assertions`，也不切换到 Cargo debug profile。

[`release.yml`](../../.github/workflows/release.yml) 的手动打包输入增加 `diagnostic_logs`，默认 false，说明中明确会保存提示词和正文。只允许 `publish=false, diagnostic_logs=true`；preflight 拒绝与发布同时启用。macOS 两个架构、Windows CLI/GUI launcher 和相关 smoke 构建必须使用一致的 feature，不能由后续命令覆盖为普通二进制。

诊断分发文件和 workflow artifact 名称增加 `-diagnostic` 标识，普通包命名和发布契约保持不变。不新增另一套打包 workflow，也不另发诊断 GitHub Release。

#### 文件与目录

默认复用 [`config_dir()`](../../src/core/config/store.rs) 的应用目录，在其下新建 `logs`：

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/logs/`
- Windows：`%APPDATA%\ViberWhisper\logs\`
- 诊断构建可用 `VIBERWHISPER_LOG_DIR` 指定其他目录；普通构建忽略该变量。此入口也用于隔离 Windows 进程级日志测试，避免访问个人 Known Folder。

日志为 UTF-8 JSONL，例如 `viberwhisper.2026-11-10.jsonl`，时间戳注明时区。按 UTC 日期轮转，最多保留七个匹配本应用命名的日志文件，不清理同目录其他文件。此限制是文件数量，单日大小不设硬上限，停止运行时也不承诺按天主动清理。

文件记录应用自身 DEBUG 及以上事件，不跟随 `RUST_LOG` 扩大到第三方网络库。记录版本、平台、PID 和日志目录；模型事件只进入文件，不进入 stderr，即使 `RUST_LOG=trace`。既有普通诊断仍按原有环境过滤写 stderr，stdout 不变。

诊断模式无法发现、创建或打开日志目录时，启动返回明确的路径错误；Windows GUI 复用已有原生启动错误提示，避免应用运行后测试者误以为日志已保存。普通模式不增加该失败条件。运行中磁盘满等写入错误不能递归写日志，须保留可用的 stderr 应急报告；不保证断电、磁盘故障或强制终止下的持久性。

#### 模型内容

- **HTTP STT：** 每次实际上传前记录 model、language、temperature、response_format、实际 prompt 和音频字节数；记录 HTTP status 及收到的响应正文，再执行既有状态检查/JSON 解析。每次上传尝试有独立 ID，保留现有重试行为。
- **HTTP LLM：** 在共用的 `LlmRequest::run` 记录实际请求 JSON，包括最终 system prompt、user 文本和 memory contexts；成功收到 body 后、解析 choices 和业务 JSON 前记录响应。并发预热同样覆盖。非 2xx 继续只记录状态，不为了诊断额外等待或读取当前不会读取的错误 body；缺失 content、无效业务 JSON 等成功 HTTP 响应可以从已保存正文排查。
- **Realtime：** 记录 model、instructions、分段 `dictation_id`、已确认的 stable prefix 原文和实际 dynamic context。单独记录 `response.create` 不够，因为其前缀仅为 item ID 引用，见 [`finish_inner`](../../src/dictation/transport.rs)。记录 `response.output_text.done` 的原始最终文本和 `response.done` 的选定完成状态/结果字段，在业务校验拒绝前保留诊断。不记录所有 WebSocket 帧或逐字 delta；取消/截断而没有最终正文时，记录可用状态，不伪造完整返回。服务端 error 继续只暴露已有的安全 code，不转储可能回显音频和凭据的整个事件。
- **关联：** 文件包含时间戳、PID、路径类别和 request ID。HTTP 使用进程内单调 ID；Realtime 沿用分段 ID。日志代表尝试发送/接收，并不把请求日志当成远端已收到或文字已投递的证明。

### Technical design and affected modules

- **`Cargo.toml` / `Cargo.lock`：** 诊断 feature 启用 optional `tracing-appender` 和 `tracing-subscriber/json`，普通构建不启用 appender。用现成 daily rolling appender 的文件保留能力，不手写轮转器。
- **`src/core.rs` / 新的 `src/core/logging.rs`：** 日志初始化归进程层，迁移原有初始化并增加文件 layer。拟用 `pub(super) fn init() -> anyhow::Result<()>`，由 CLI 和 GUI 两个入口调用，错误沿现有进程边界报告。stderr layer 显式拒绝模型正文专用 target；文件 layer 独立固定过滤应用 DEBUG，不让一个全局 EnvFilter 同时控制两个 sink。优先复用 tracing 格式化和写入，不另建与 tracing 并行的 JSON writer 或后台日志线程。
- **共享诊断辅助：** HTTP 请求关联和已知密钥脱敏若需共用，放在 crate 内日志模块，保持业务调用只构造模型字段、不接触文件。拟用 feature 限定的 `next_request_id() -> u64` 和 `redact(text: &str, secret: Option<&str>) -> String`；不增加公开类型、HTTP 中间件或通用事件框架。原子计数器只代表诊断请求身份，不进入业务协议。所有正文构造、密钥替换和新增计时均限定在诊断 feature 内。
- **`src/transcriber/api.rs` / `src/postprocess/llm.rs` / `src/dictation/transport.rs`：** 在已有请求/响应边界发出专用 target 的结构化事件，不修改输入、返回值、重试次数、超时、流式顺序或确认学习。为诊断保留的 Realtime model 字段如有需要使用 feature 限定的 `String`，含义仅为当前连接配置的模型名。
- **`tests/cli_output.rs` 和现有 API/Realtime stub 测试：** 以实际文件、请求及可观察结果验证，不逐行锁定格式或引入全局测试捕获器。
- **`.github/workflows/release.yml`、`docs/releasing.md`、`docs/usage.md`、`docs/README.md`、`changelog`：** 同步打包开关、日志位置、敏感数据提示和测试范围，沿用当前文档职责。

不引入异步 appender/WorkerGuard，避免新增退出时排空队列的生命周期；同步写入仍有文件 I/O 成本，模型正文事件只在请求边界产生，不在音频逐帧热路径产生。具体 builder 类型、trait bound 和最少辅助函数在实现时按实际依赖 API 确定，不把无必要的类型层级写进计划。

### Alternatives and rationale

- 只设置 `RUST_LOG=debug` 或 shell 重定向：不能提供当前不存在的正文事件，也不能让双击启动的 Windows GUI 自动保存日志。
- 对全部 dry-run 自动打开正文：测试者可能只想检查安装，不一定同意保存敏感内容，因此采用明确命名、默认关闭的打包输入。
- 正式版新增运行时正文开关：可复用同一二进制，但扩大误开启的范围；本次需求限定测试打包，先使用编译期 feature。
- HTTP 中间件/全 WebSocket 帧转储：会引入音频及 headers 泄露和大量重复数据，选择实际模型字段及完成事件。
- 一份普通日志加另一套手写模型 JSON writer：需要两套格式、写入锁和错误处理；用 tracing 专用 target 和每层过滤即可满足隔离要求，须用实际 stdout/stderr 回归保护。
- 不轮转或手写轮转：前者长期积累文件，后者增加文件生命周期维护；optional appender 的现有能力更合适。七个文件不是七天强制擦除，不增加磁盘监控或大小轮转框架。

### Costs, risks, and mitigations

**诊断文件会完整保存识别文字、用户 prompt、历史和语境记忆，可能包含个人或工作秘密。** 只应在可信测试环境使用，不自动上传，分享前必须检查和删除敏感内容；文件名的 diagnostic 标识不等于安装时已有隐私同意界面。

主动排除鉴权头、请求 URL、完整配置和音频；新增正文记录对该调用已知、非空的 API key 做原样替换。替换只用于日志副本，不改变发送或解析内容。它不保证识别经编码、变形或部分回显的秘密，也不识别用户主动口述的密码；不把局部保护说成全应用隐私审计。

既有事件及错误仍可能含文件路径等诊断信息；不在本次顺带清理所有历史日志。文件默认位于用户应用目录，Unix 新建日志文件采用仅用户读写权限；不重设已有共享目录或个人数据权限。单日文件可能较大，同步写入可能增加请求边界耗时；不增加实时音频路径写入，不承诺零开销。Windows installer 与正式包仍使用原安装身份，不承诺可并存安装；必要时先卸载再换包。

### Implementation order

1. 明确批准方案，并取得推送/创建计划 PR 的授权；在同一 bookmark、同一 draft PR 保留方案及后续实现，不在批准前写产品代码或测试。
2. 为新增可观察契约编写最少的失败回归，再实现 feature、两个入口初始化和文件/控制台隔离；验证目录错误及普通构建对照。
3. 依次接入 STT、共用 LLM、Realtime 请求及响应记录，验证关联、无效返回和隐私边界，不扩展模型协议。
4. 增加打包输入、防误发布检查和诊断命名，确保所有目标/后续 smoke 构建保持同一 feature；更新使用/打包文档。
5. 运行以下验证及单机制对照，记录真实结果后按项目代码审查流程交付同一 PR。手动触发远端打包或发布需要独立授权，不与推送授权混用。

配置无需迁移。回滚使用未启用 feature 的普通包；诊断文件由测试者自行删除，不随回滚自动删除。

### Validation and ablation strategy

- **真实 CLI 与磁盘：** 扩展现有隔离 profile、本地 stub 和公开 speech fixture。同一输入分别运行普通/诊断构建：普通包不创建日志；诊断包在默认目录及自定义目录写可解析 JSONL，stdout 正文不变，`RUST_LOG=trace` 下新增 prompt/返回正文仍不出现在 stderr。不可写路径返回明确错误，不访问个人配置。
- **三条模型路径：** 现有 STT、LLM 和 WebSocket stub 使用不同的中文/英文 prompt、不同语境、并发请求和非对称结果，核对日志与实际发送/返回一致、ID 可以配对。覆盖 STT 重试、LLM 预热、成功 HTTP 但无效 JSON/业务 JSON、Realtime stable/dynamic 均非空及最终校验失败；错误分类和文本结果保持原契约。测试已知 key 原样回显不会留下密钥，音频哨兵和 headers 不出现。不中断现有取消、清理、流式和学习测试。
- **轮转与保留：** 通过 appender 可用测试入口或预置匹配文件验证保留上限、非本应用文件不受影响，不等待真实跨日、不写自制时钟/文件系统框架。Windows 进程测试只使用明确的临时日志目录；若需个人配置才能启动，则不执行该路径，保留平台无关文件 sink 检查并说明限制。
- **工作流契约：** 验证四种 publish/diagnostic 组合，诊断发布组合被拒绝；普通包名称不变，两个 macOS 架构和 Windows 两个入口的 feature 相同，bundle 不重编为另一模式。YAML/表达式验证不能替代真实两平台打包，后者以获授权的 CI 结果为准。
- **针对性对照：** 临时只移除正文 target 的 stderr 排除，检查同一 trace 级场景检出泄露；只省略 stable 原文，检查 Realtime 日志不完整；只移除 diagnostic/publish 拒绝，检查非法组合不再失败。保留机制需对应实际失败，不保留实验变体或额外测试框架。
- **整体检查：** `cargo fmt --check`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、普通及诊断的 release build、`cargo clippy --locked --all-targets --features integration,diagnostic-logs -- -D warnings`、普通配置 Clippy、现有 release contract 和 `git diff --check`。本机 macOS 不冒充 Windows 实机验收；不向真实服务发送个人内容，不触发未授权的打包 workflow。

### Open questions and deferred work

待批准的是测试包显式启用、完整文本内容记录、默认目录、七个 daily 文件及日志初始化失败时停止诊断包启动的行为。无需用户提供 API key、真实录音或额外服务；实现细节按现有 APIs 决定。

延后运行时日志设置 UI、文件大小硬上限、加密归档、自动上传、真实模型质量评测、全应用日志脱敏审计以及 Windows 并存安装。当前只有方案；推送、计划 PR、实现和实际诊断包均尚未交付。

## 2. PR change timeline

### [001] — 2026-11-10 — 配置同目录、仅当天及发布选项开关

- **Context / requirement update：** 用户在[本线程](https://ampcode.com/threads/T-01a1c463-98b2-77ab-bb72-cf352749e3dd)要求：“日志放在和配置同一个目录，只保留当天文件，根据发布选项来实现这个功能的开关，普通包不打印debug日志”。这是对原方案的明确修改要求，不视为已批准其余设计或已授权推送、创建 PR、触发打包。
- **Decision / status：** 按用户要求修订待批准方案，以下内容覆盖初始计划中不一致的部分；产品代码和测试仍未实施。
  1. **目录：** 日志直接与 `config.json` 同目录，不创建 `logs/` 子目录，不提供 `VIBERWHISPER_LOG_DIR` 或运行时目录设置。macOS 为 `~/Library/Application Support/com.b1indsight.viberwhisper/`，Windows 为 `%APPDATA%\ViberWhisper\`。路径沿用 [`config_dir()`](../../src/core/config/store.rs)，不得从当前工作目录推导。
  2. **仅当天：** 以应用所在机器的本地日历日期定义“当天”，替代原来的 UTC 日期和七文件保留。只追加到当天命名的 `viberwhisper.YYYY-MM-DD.jsonl`；诊断包启动时及跨日期后下一次写入前，清理本应用命名规则内、日期不是当天的日志，包括因时钟回拨留下的未来日期文件。先停止使用旧文件再删除，兼容 Windows 文件占用规则。配置、历史、记忆、任意其他日志或同名前缀但不符合完整日期格式的文件不受影响。
  3. **清理时机：** 不启动后台清理服务，也不在应用完全退出后定时删除文件；停用期间可能保留上一次运行的文件，下一次启用诊断模式时清理。普通包不创建、写入或清理诊断文件。清理失败应明确报告，不宣称已经满足仅当天保留。
  4. **发布选项开关：** 使用现有 `Package & Release` 的手动发布/打包表单，增加默认关闭的 `diagnostic_logs` 选项，构建时决定是否启用日志 feature。开关开启的包自动写 DEBUG 及模型输入/输出；关闭的普通包不输出 DEBUG 或 TRACE，即使设置 `RUST_LOG=debug` 或 `RUST_LOG=trace` 也不能绕过。普通包仍可输出 INFO/WARN/ERROR 运行诊断，CLI stdout 契约不变。无需修改配置文件或设置环境变量来启用功能；其余诊断包标识、模型正文文件专用及不允许诊断模式正式发布的原方案不变。
- **Options considered：** 不保留“配置目录下再建日志目录”和自定义目录开关，以免与用户指定路径不一致。`max_log_files(1)` 只能约束数量，不能独自保证启动时去除非当天文件及本地日期边界，因此不能把它当成完整实现。优先评估现有 appender 能否满足新契约；如不能，则采用最窄的按本地日期换文件实现，不增加后台线程、通用轮转框架或多份日志 writer。
- **Consequences：** 覆盖初始计划的 `logs/` 路径、自定义目录变量、UTC 七文件轮转、普通包可由环境变量开启 DEBUG，以及仅凭 optional appender 文件数量上限完成保留策略的技术假设。正常日志仍保留 INFO 及更高等级；模型输入/返回记录、凭据保护和不保存音频的要求不变。当天文件没有大小硬上限，诊断文件中仍可能含敏感文字。
- **Validation amendment：** 在临时目录预置昨天、当天、未来日期的应用日志，以及 `config.json`、历史/记忆哨兵和不匹配文件；验证启动及跨本地午夜后仅保留当天应用日志，受保护文件字节不变，同一天重启追加而不覆盖。日期边界使用确定性时钟输入，不等待真实午夜，也不增加产品时区配置。普通包分别在默认、`RUST_LOG=debug` 和 `RUST_LOG=trace` 下验证既有 DEBUG/TRACE 事件和新增模型正文均不可见，INFO/WARN/ERROR 仍按配置过滤。Windows 日志测试通过文件 sink 的临时路径边界隔离，不为测试重新引入产品目录开关，也不读取个人配置。
- **Evidence / validation：** 已只读复核 `src/core.rs` 的现有环境过滤、`src/core/config/store.rs` 的两平台目录和 `.github/workflows/release.yml` 的发布输入。当前仅更新文档；日志行为、清理和打包开关尚未实现或运行验证。
- **Outcome / pending：** 以上四项要求已纳入方案，等待明确批准后实施。未推送、未创建 PR、未触发远端打包。

### [002] — 2026-11-10 — 按修订契约实现日志与打包选择

- **Requirement / context：** 实施已确认的初始计划及 [001]，保留配置同目录、本地当天文件和普通包 INFO 上限。
- **Implemented：** `Cargo.toml` 增加非默认 `diagnostic-logs`，仅启用 optional `chrono` 本地日期和 tracing JSON 格式。`core::logging` 统一 CLI/GUI 初始化；文件 layer 固定仅接收应用 DEBUG 及以上，不受 `RUST_LOG` 影响。控制台统一 INFO 上限并单独排除模型 target，既保护普通包，也避免诊断包在全局 trace 过滤下转储第三方传输内容。
- **Decision / status：** 已实现最窄同步 `DailyWriter`，替代初始计划的 UTC rolling appender。它在一个锁内取得本地日期、关闭旧句柄、删除完整匹配且日期非当天的普通日志文件、打开/追加当天文件，再写入完整 tracing 事件。启动和日期变化才扫描目录；没有后台线程、异步队列、自定义目录或额外配置。`tracing-appender` 的 UTC 文件/数量保留不能直接满足 [001]，因此没有引入该依赖。跨日锁内读时钟避免排队事件把文件错误切回昨日；同名符号链接/目录拒绝作为当前文件打开，新建 Unix 文件为 0600。
- **Model boundaries：** STT 在实际上传边界保存字段及 status/body；LLM 复用实际 request JSON，成功正文在解析前保存，非 2xx 仍只取状态。HTTP 各尝试使用进程内 ID。Realtime 使用分段 ID，保存实际指令、稳定前缀及动态输入；最终文本/完成状态在校验前保存，取消完成也有状态。完成事件只选文本字段，排除意外音频 part，不新增完整帧转储。仅对日志副本中的当前非空已知 key 原样替换，业务输入/输出不变。
- **Packaging：** 默认 false 的 `diagnostic_logs` 表单项在 preflight 中选择 feature 和名称后缀；所有 macOS 架构、Windows CLI/GUI 和后续 Cargo/bundle/smoke 步骤沿用同一选择。`publish=true, diagnostic_logs=true` 提前拒绝，发布 job 也显式排除诊断模式。普通包名称保持不变；诊断分发文件及 workflow artifact 名称带 `-diagnostic`。
- **Affected modules / docs：** `core.rs`、`core/logging.rs`、三条模型调用路径、`tests/cli_output.rs`、release workflow/契约 fixtures，以及 usage/releasing/index/changelog。CI 增加两平台诊断 feature 的测试和 Clippy，不通过手动触发 release 来代替 PR 验证。
- **Consequences：** 同步文件 I/O 只位于日志边界，没有音频逐帧正文写入。初始化/清理失败阻止诊断包启动，沿现有 CLI/原生 GUI 错误边界显示路径；运行中失败开启 tracing 的 stderr 应急报告，Windows 无控制台入口可能看不到，不承诺磁盘故障持久性。日志依然可能包含敏感内容，当天无大小上限；普通包不清理已有诊断文件。

### [003] — 2026-11-10 — 本地验证、单机制对照与剩余范围

- **Evidence：** [汇总验证记录](https://ampcode.com/artifacts/c6fda94b-f6bb-4d47-9786-2c4b90aa0525)。使用临时配置目录、本地 HTTP/WebSocket stub 和公开 WAV，未读取个人配置、记忆或录音，未调用真实模型。
- **Before / after：** 首先新增 CLI 契约回归；原 logger 在 `RUST_LOG=debug` 下输出应用/第三方 DEBUG，回归失败。实现后普通/诊断模式均通过：stdout 仅命令结果，模型 prompt/返回不进入 stderr；普通模式不生成或清理日志，诊断文件在 debug/trace/off 过滤下仍独立工作，文件没有第三方/TRACE 事件。
- **Retention / failure controls：** 确定性日期验证启动删除昨日/未来文件、同日重启追加、跨日及回拨清理，并核对配置/历史/记忆/不匹配文件字节不变；并发写入保持完整 JSONL。真实 release CLI 另验证不可删除旧日志时启动非零退出并保留原数据、当前日志符号链接拒绝且不修改配置、新建文件权限 0600。
- **Model controls：** 保留回归核对 STT 503→200 的独立尝试 ID、并发 LLM 的实际 messages/非空语境与响应配对、短 key 日志副本替换且真实返回未变，以及 Realtime 的真实稳定/动态输入、校验失败前正文、截断/取消状态和音频字段排除。临时隔离用例另执行真实预热后台线程，确认其请求/结果写入文件且配对；验证后移除临时用例，不保留全局测试捕获器。真实 release CLI 的畸形 HTTP 200 STT 对照确认两次响应均在 JSON 解析前保存并脱敏。
- **Ablation setup / results：** 每次只改一个机制并运行相同输入，实验后恢复正式代码：
  - 去掉旧日志删除后，`cargo test --locked --features diagnostic-logs daily_log_keeps_only_local_today_and_preserves_other_app_data` 在昨日文件仍存在处失败；恢复后通过。保留显式日期清理，不能用只换文件名代替。
  - 去掉稳定前缀原文后，`cargo test --locked --features diagnostic-logs diagnostic_realtime_keeps_actual_prefix_context_and_rejected_text_without_audio` 检出日志为 null、实际声明非空的差异；恢复后通过。保留 item 引用之外的原文。
  - 去掉 diagnostic/publish 拒绝后，当时的打包组合对照检出非法组合被接受；恢复后四种组合通过。保留提前拒绝，避免仅靠末尾发布 job 跳过而浪费打包。
  - 将模型事件临时改为 INFO，CLI 隐私回归在保留 target 排除时通过；仅移除该排除后，同一回归检出 prompt 出现在 stderr。恢复模型 DEBUG 和排除后通过。INFO 上限与正文专用 target 分别约束等级和内容边界；不依赖当前事件级别偶然满足隐私要求。
- **Commands / outcome：** `cargo fmt --check`；`cargo test --locked`；`cargo test --locked --features diagnostic-logs`；普通 `cargo clippy --locked --all-targets -- -D warnings`；`cargo clippy --locked --all-targets --features integration,diagnostic-logs -- -D warnings`；普通/诊断的 `cargo build --release --locked` 及 `cargo test --release --locked --test cli_output`；release 契约、Bash 语法、plist/WiX XML 与 `git diff --check` 均通过。诊断 Clippy 首次检出两个可自动合并的 if，已用 `cargo clippy --fix` 处理并重新验证；vendor/rdev 的既有警告不在本次改动内。
- **Limits / pending：** 两个既有测试仍按原规则忽略（私有录音集、真实 Realtime 服务）。打包契约在本机执行实际模式选择及 macOS 命令块的 stub 对照，Windows Cargo/PowerShell 仅静态核对；没有把它们当成 DMG/MSI 或 Windows 实机验收。尚未手动触发原生双平台打包、发布或合并；后续须按独立授权执行。

### [004] — 2026-11-10 — 修复 Windows CI 暴露的并行日志捕获竞态

- **Trigger / evidence：** [实现提交的 Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/42783453851/job/129718785906) 在 `cancellation_between_create_and_ack_still_cancels_the_correct_response` 新增的日志断言失败：取消协议已完成，但 `model_events(...).last()` 为 None。同一提交的 macOS CI 通过。这里继续修复 CI 失败，不涉及此前独立代码审查指出的 STT 错误正文脱敏和音频回调同步写入两项问题；两项仍未修改。
- **Reproduction / cause：** 本机以 `--test-threads=32` 重复运行未修改的诊断单测二进制，第 18 轮在相同测试、相同行复现。临时确定性用例先创建 scoped capture，再让无订阅线程首次触发同一 `model_event` 日志点，最后在 capture scope 内写事件；修复前捕获数为 0。核对所用 `tracing-core 0.1.36` 源码，其 single-dispatch 快速路径会按首次注册线程的当前 subscriber 计算 callsite interest；唯一登记的 dispatch 为另一个线程的 scoped capture 时，未订阅线程可能把 never-interest 缓存到全局。
- **Decision / implemented：** 仅在 `cfg(all(test, feature = "diagnostic-logs"))` 的共用捕获器保留一个惰性、无操作 `Dispatch`，使 callsite 缓存综合它和实际 capture 的 interest，再按当前线程判断。它不安装全局默认 subscriber，不捕获其他测试，不保留文件句柄，也不改变生产日志、协议取消、线程调度或应用配置。没有通过串行测试、增加等待、重试断言或删掉日志断言掩盖失败。
- **Validation / ablation：** 同一临时确定性用例在加入无操作 dispatch 后捕获数为 1；验证后移除临时用例，保留现有协议与日志回归不变。修复后的完整诊断单测二进制在相同 `--test-threads=32` 条件下连续 40 轮通过。`cargo fmt --check`、`cargo test --locked --features diagnostic-logs`、`cargo test --locked`、`cargo clippy --locked --all-targets --features integration,diagnostic-logs -- -D warnings` 和 `git diff --check` 通过。修复前/后日志与命令结果见[汇总验证记录](https://ampcode.com/artifacts/c6fda94b-f6bb-4d47-9786-2c4b90aa0525)。
- **Limits / delivery：** 本地仍为 macOS arm64，重复验证不能代替修复提交的 Windows CI；审查通过后沿用同一 bookmark/PR 推送并订阅精确新提交。未手动重跑 workflow、触发打包、发布或合并。

### [005] — 2026-11-10 — 缩减打包验证配套

- 按用户要求删除额外的打包模拟脚本、现有契约入口对它的调用及使用说明，不引入替代测试框架；日志实现、发布开关和现有 Rust 回归保持不变。
- 验证：禁用已移除脚本的解释器后，原入口在删除前退出 127，删除后通过；既有 Bash 契约、脚本语法、plist/WiX XML 与 `git diff --check` 均通过。此前组合对照保留为历史结果，不再是当前 CI 覆盖；模式/feature/名称传播需在获授权的原生打包中验证。
