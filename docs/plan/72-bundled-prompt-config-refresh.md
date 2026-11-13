# 72 - 升级时使用新版内置提示词

**阶段：** 001 实现及 live Realtime 测试修复已推送，macOS / Windows CI 通过；004 组件配置修订待确认，尚未修改产品或测试代码。
**PR：** [#180（草稿）](https://github.com/b1indsight/viberwhisper/pull/180)，`feat/config-prompt-refresh` → `master`。

**当前方案：** 初始计划与 001–003 保留为历史记录；已实现行为见 [002](#002--2026-11-13--完整提示词落盘与直接消费已实现)和 [003](#003--2026-11-13--live-realtime-测试使用完整配置)。用户要求改为单一示例配置来源、保存组件并在识别时拼接，拟按 [004 修订](#004--2026-11-13--示例配置作为默认来源识别只拼接已保存组件)替代 001 的正常/短版全文保存设计，待明确确认后实施。

## 1. 初始计划

### 背景与目标

按用户要求，暂时关闭配置文件中的 prompt 自定义。安装新版后首次启动桌面应用时，读取原有配置，将提示词覆盖为当前版本的内置默认选择，保留其他设置。安装器本身不读写用户配置；新规则在运行新二进制时生效。

目前 `ConfigStore::load` 原样返回严格解析的文档，启动时的 `materialize_dictation_defaults` 只填缺失字段。显式全文、空字符串和组件覆盖都会保留，因此旧配置可以阻止新版内置规则生效。[配置读写与补齐](../../src/core/config/store.rs)、[Realtime 来源选择](../../src/dictation/config.rs)

本方案有意替代[计划 53](53-configurable-dictation-prompt-components.md)中允许持久化自定义提示词、以及[计划 56](56-realtime-config-default-materialization.md)中保留显式提示词的政策。保留这些历史记录，不改写原来的决策。

### 范围与成功标准

- 暂停 `transcription.prompt`、`post_process.prompt`、`dictation.prompt`、`dictation.prompt_components` 的持久化自定义，包括空字符串和显式 `null` 的原有覆盖语义。
- HTTP 转写使用 `TranscriptionSection::default().prompt`；LLM 整理使用 `None` 选择内置规则；Realtime 的两个字段使用 `None` 选择内置组件。默认正文继续由现有所有者维护，不复制一份新提示词。
- 保留所有非 prompt 配置，包括 API、模型、磁盘密钥、鉴权环境变量名、热键、设备、增益、语言、温度、开关及长期词表。不读取或修改历史、短期记忆、个人配置或已安装应用来验证本次实现。
- `config get/list/check`、离线转换和 lab 的配置加载保持磁盘只读；桌面成功启动配置校验后才条件写回。正常配置保存也会携带加载后的默认提示词。
- 保留 prompt-lab 的 `--prompt-file`、`--no-prompt`，作为单次音频候选实验，不持久化，也不覆盖文字后处理 prompt。
- 不改提示词正文、输出协议、模型、识别策略或 schema v3，不承诺提高模型识别质量。

### 用户可见行为

例如旧配置同时有自定义 HTTP prompt、LLM 整理规则和 Realtime 全文或组件：新版本加载后，运行时全部使用内置默认提示词。桌面配置校验成功后，只覆盖这四个字段，并继续补齐缺失的 Realtime 字段；地址、模型、词表等保持原值。再次启动时内容相同就不重写。

旧 `dictation.prompt: ""` 不再关闭指令；HTTP 的空字符串、缺失或 `null` 也统一使用内置中文提示词。类型合法但过长、或同时设置了全文及组件的旧配置，在清除覆盖后不再因这些 prompt 校验失败。其他配置仍按原有规则校验。

两个普通 CLI prompt 键改为只读，`config set` 返回已有的只读错误。手工改写旧 prompt 也不会生效；`config get/list` 显示加载后的生效值，磁盘旧值在下次成功桌面启动或正常配置保存时被替换。

### 技术设计与接口

不新增类型或迁移框架，只调整现有配置边界和启动补齐操作。

- `src/core/config/document.rs`：增加 `pub(crate) fn reset_prompts(&mut self)`，仅将四个字段赋为所属配置类型的默认值，不读取凭证、不修改其他字段。无调用者需要变更标志，因此不返回 `bool`。
- `src/core/config/store.rs`：`pub fn load(&self) -> Result<Option<ConfigDocument>, ConfigError>` 签名不变。先按现有方式严格反序列化，再调用 `reset_prompts`，返回可供全部生产调用者使用的文档，不写磁盘。
- 将 `materialize_dictation_defaults` 改为 `pub(crate) fn refresh_defaults(&self, document: &ConfigDocument) -> Result<bool, ConfigError>`。重读磁盘 JSON，沿用 `fill_missing` 补齐 Realtime；随后插入或替换四个 prompt 键，值来自已经归一化的文档。比较更新前后 JSON，仅变化时调用现有 `write_atomic`，返回是否发生写入。不能仅对 prompt 调用 `fill_missing`，否则显式旧值仍会保留。
- `src/ui/setup.rs`：在现有 `resolve_document` 成功之后调用 `refresh_defaults`，保持原来的执行顺序。写回失败继续通过 `?` 报告启动错误，不把失败当作成功更新。显式 setup、托盘设备/增益和 CLI 普通字段保存均以 `load` 或默认文档为起点，不增加另一套清理逻辑。
- `src/core/config/fields.rs`：把 `TranscriptionPrompt` 和 `PostProcessPrompt` 标为不可写，移除相应可达赋值分支，复用现有 `ReadOnly` 错误。读取键保持可用，不新增 `dictation.*` CLI 键。
- `ConfigStore::save` 保持原子保存原语，不另克隆并重置调用方文档。现有生产保存入口没有 prompt 编辑 UI；关闭 CLI 写入后，来源均为归一化加载或默认文档。验证实际保存入口的结果，避免调用方内存与磁盘在保存时分叉。
- `src/dictation/config.rs` 的全文/组件选择、长度校验以及 `TranscriberConfig::with_prompt` 保留，继续支持加载后的 lab 临时覆盖；不在运行时解析器中全局禁用候选。
- `config.example.json` 将 Realtime 的两个 prompt 字段设为 `null`，不再复制内置组件正文。HTTP 保持其内置字面量，LLM 保持 `null`。
- 更新 `docs/configuration.md`、`docs/dictation.md` 和受影响的 core/dictation/prompt-lab 架构说明，说明暂停自定义、覆盖时机、只读检查与实验候选边界；不重写历史计划。

### 错误与兼容边界

无配置时仍返回 `None`，走原来的首次设置；拒绝恢复时仍只使用内存默认值，不创建或覆盖文件。缺少整个 Realtime 节时补入 `enabled: false`，不因升级开启 Realtime。完整配置仍会检查四个 prompt 值是否需要覆盖。

损坏 JSON、不支持的 schema、未知字段、错误字段类型及无效组件结构仍严格报错；清理发生在反序列化成功后，不把损坏配置当成可覆盖文件。写失败保留原子保存的错误语义。普通检查不落盘，setup 取消不落盘。

本次不新增备份或并发协调。原子替换避免半截 JSON，但不保证多个进程同时保存时不互相覆盖；延续[计划 56 的并发范围决定](56-realtime-config-default-materialization.md#005--2026-10-12-remove-concurrency-handling-as-requested)。

### 替代方案与取舍

- 仅在桌面启动时改磁盘：CLI、离线转换及校验仍可能读到旧 prompt，无法完整关闭自定义；因此统一在 `load` 的内存结果清理。
- 在每次 `load` 时写磁盘：会让检查命令产生写入副作用；因此落盘继续放在已有的显式启动更新点。
- 按应用版本号判断一次迁移：需要额外元数据，同版本下的手工覆盖仍会生效。暂停自定义期间按实际字段差异更新即可，不增加版本标记。
- 用 `save(document)` 全量替换启动配置：会改变无关字段的显式形态；沿用原 JSON，只覆盖四个 prompt 键并补缺失的 Realtime 字段。
- 保存时再次归一化：当前没有另一个持久化 prompt 的生产入口，不增加重复防护层；用入口行为测试保护这一约束。
- 删除旧字段：严格 schema 会导致现有文件无法加载；保留字段形状并在成功加载后重置，不做广泛 schema 迁移。

### 成本、风险与回退

旧自定义提示词会在成功更新时直接丢失，包括显式空指令；这是本次覆盖策略的兼容性变化。HTTP 默认仍以字面量写入，Realtime 和 LLM 用 `null` 引用当前内置规则。将来重新开放自定义时，需要单独设计如何识别历史 HTTP 默认值，本次不提前实现。

更新会改变 JSON 排版，但只改变约定字段的值。没有版本号迁移标记，换用另一个二进制会使用那个版本自带的规则；降级旧版不恢复已经覆盖的个人 prompt。没有热重载，已经运行的进程仍需重启。

### 实施顺序

1. 在现有配置测试中用隔离临时文件复现旧 prompt 遮挡新版默认、只补缺失不覆盖的行为，再实现加载归一化与启动条件更新。
2. 关闭两个 CLI prompt 写入键，调整现有 setup/托盘保存回归的 prompt 预期，保留其他字段的保护断言。
3. 验证 HTTP、Realtime、LLM 的实际请求和元数据使用内置规则，且 lab 临时候选仍能覆盖音频 prompt，不改写配置或个人记忆。
4. 更新示例、现行文档和本记录，执行本地检查及代码审查。获得发布授权后再按同一个 bookmark/PR 交付，不修改已安装应用或个人配置。

### 验证与定向消融

扩展现有测试文件，不为每个字段另建测试框架。旧值通过直接写临时 JSON 构造，不能依赖归一化后的 `save` 或 `load` 生成期望值。

- 配置回归：自定义全文、空字符串、`null`、旧七字段组件、全文和组件共存、合法类型的超长覆盖都回到默认。使用非默认的连接、温度、增益、词表和磁盘密钥，独立断言只有四个 prompt 字段变化，环境密钥不写盘。
- 存储回归：首次读取字节不变；缺失或部分 Realtime 字段正确补齐；完整但含旧 prompt 的文档仍会更新；第二次更新返回 `false` 且字节不变。损坏文件、错误类型、缺失文件及已有写失败检查保持有效。
- 入口回归：两个 CLI prompt 写入失败且文件不变；普通字段修改、setup 确认保存、托盘设备/增益修改保留无关设置并保存默认提示词；setup 取消不覆盖旧文件。
- 请求与实验回归：复用现有本地 HTTP/WebSocket fixture 验证发送的正常/短段 Realtime 指令、HTTP prompt、LLM 内置整理与报告元数据一致。lab 候选与 `--no-prompt` 在加载之后生效，配置字节保持不变，不产生真实服务请求或个人状态写入。
- 定向消融：临时去掉加载归一化，内置请求及冲突旧配置回归应失败；临时去掉启动覆盖步骤，磁盘覆盖回归应失败。恢复后重跑。没有新性能或可选机制，不增加通用消融框架。
- 本地检查：`cargo fmt --check`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets --features diagnostic-logs -- -D warnings`、`git diff --check`。Windows 的 CI 验证只在后续获准推送后执行，未运行前不宣称通过。

这些检查均为待实施项目，本轮没有运行产品测试。方案草拟参考本地 Claude Code Opus 5.5 的只读咨询，实际初始化和响应模型均已核对，工具和 MCP 列表为空；咨询不作为代码验证。

### 待批准事项与延期项

批准范围为同时暂停 HTTP、LLM、Realtime 的配置 prompt 覆盖，保留 lab 单次候选；更新时机是新版成功启动配置校验后。除此以外没有阻塞设计问题。

恢复自定义、旧 schema 自动迁移、备份管理、多进程写入协调、安装器更新钩子、提示词正文调优、个人安装和发布均不在本次范围内。

## 2. PR 变更时间线

### 001 — 2026-11-13 — 默认策略先修改配置，识别不做现场覆盖

**要求与状态。** 用户明确要求：“默认设置的效果均体现为对配置的修改，在识别流程中不要有现场覆盖的流程。”这条要求取代初始计划中的 `load` 内存重置、运行时以 `null` 选择默认规则、读取后忽略手改值以及先创建识别配置后落盘的方案。以下为据此修订的设计，待确认实施；本轮没有修改产品或测试代码。

**配置准备先于识别。** `ConfigStore::load(&self) -> Result<Option<ConfigDocument>, ConfigError>` 保持原有只读反序列化，不清除或改写加载值。增加 `pub(crate) fn prepare(&self) -> Result<Option<ConfigDocument>, ConfigError>`，在显式配置更新阶段严格读取旧文件、更新 prompt 和补齐可缺省字段、比较实际 JSON、必要时原子保存，再重新加载返回。不存在时返回 `None`；损坏 JSON、未知字段、错误类型等仍报错并保留原文件。写入失败直接中止，不能把尚未保存的更新交给识别流程。旧 `materialize_dictation_defaults` 的职责合入此准备操作，更新不再放在创建监听器配置之后。

**默认值必须成为实际配置内容。** 不再计划 `reset_prompts`。改为 `pub(crate) fn apply_bundled_prompts(&mut self)`，只在配置准备和首次 setup 的候选配置构建阶段调用，将当前内置默认内容写入配置字段。HTTP 保存已有完整转写 prompt；LLM 的 `post_process.prompt` 保存当前默认的完整 system 文本，包含任务、整理、输出和参考规则；Realtime 的 `dictation.prompt` 保存正常完整文本，新增可选 `dictation.short_prompt: Option<String>` 保存现有短音频版本，`dictation.prompt_components` 清为 `null`。默认正文不改，正常/短段的选择阈值不改。

**识别只消费配置。** Realtime 的 `prompt()`、`short_prompt()` 改为读取已经配置的正常/短版文本，不读取内置资源、不选择组件回退、不临时替换短例子。`prompt_components` 仅保留为旧文件解析与更新的兼容字段，不再参与识别选择。LLM 配置装配将 `post_process.prompt` 解析为完整 `String`，`LlmPostProcessor::new` 原样使用它，不再调用 `postprocess_prompt` 追加内置规则。默认完整文本的生成留在配置更新阶段；缺少必需文本时报配置错误，不在识别流程里补默认。正常请求内容必须与更新前使用内置规则的请求逐字一致。

**保留一个明确的短版字段。** 仅保存组件、在识别时继续调用当前 `compose_for_segment(true)` 会再次替换为代码里的空结果例子，仍有不体现在配置中的内容选择，因此采用保存两个完整版本的方案。新增字段属于 schema-v3 的可选扩展；旧文件可由新版准备阶段读取并补齐。旧版程序可能因未知 `short_prompt` 拒绝新文件；旧版 LLM 又会把完整 system 文本当作清理片段重新包裹，因此不承诺直接降级读取，回退须同时使用对应旧配置。

**入口和只读检查。** 桌面、普通离线转换在配置准备成功、重新加载后才装配识别服务。`config path/get/list/check` 继续检查保存内容，不自动更新、不显示另一套覆盖后的 prompt；旧配置缺少完整识别参数时，`check` 报告需先完成配置更新。两个普通 CLI prompt 键仍为只读；手改值在下次准备时被实际写回替换，不能仅在内存中屏蔽。设备和增益更新仍只保存目标字段，不成为 prompt 更新触发器。

**首次设置和取消。** 首次 setup 在候选配置中先写明完整默认 prompt，再进行现有候选录音验证；验证只消费该候选配置，不另加默认覆盖。最终确认后保存并重新读取，再进入正常监听。配置缺失或无效时，用户拒绝设置则不运行临时默认监听器；不创建未确认的新文件，也不覆盖无法解析的文件。普通 `convert` 缺少配置时要求先 setup，移除进入识别前的 `unwrap_or_default`。已有有效文件的自动准备与随后向导编辑是两个步骤：准备中已完成的 prompt 更新不会因用户取消后续向导而回滚；向导自身取消不再追加保存。

**lab 仍隔离主配置。** 保留 `--prompt-file` 和 `--no-prompt`，但在运行时装配之前修改明确的实验配置，不在已经生成的 transcriber 或 Realtime settings 上追加覆盖。复用 lab 已有临时目录，保存隔离配置、在该副本上执行相同准备步骤，再将候选写入副本并加载；无 prompt 候选同时清除短版，HTTP 保持省略 prompt 的既有语义。基线和候选都不写主配置，普通实验仍使用相同识别入口及隔离记忆。`ConfigStore::at(path: PathBuf) -> Self` 由仅测试可用调整为 crate 内可用，供这份隔离文件复用同一存储逻辑，不新增第二套配置存储。

**模块和文档。** 除初始范围的配置 store/document/fields、setup、示例和现行文档外，调整 `src/dictation/config.rs` 的保存值解析、`src/postprocess.rs` 的完整 prompt 类型及校验、`src/postprocess/llm.rs` 的直接消费，以及 `src/core.rs`、`src/core/prompt_lab.rs`、`src/prompt_lab/production.rs` 的配置准备时机。不修改提示词措辞、模型、协议、采样或段长规则。示例配置展示具体正常/短版与完整 LLM prompt，不再以 `null` 表示临时取默认。serde 读取缺失 prompt 字段仍保持缺失状态，不通过新增字段的反序列化默认值暗中填充正文。

**验证修订。** 下列项目取代初始计划中“只读加载返回内置默认”的断言，仍全部待执行：

- 用旧文件验证 `load` 返回保存值且不写盘；`prepare` 完成后，重新读取的五个 prompt 字段为实际目标值（HTTP、LLM、Realtime 正常、Realtime 短版、退役组件），其他显式配置保留；第二次准备不重写，缺省字段落盘。
- 分别验证非法文件不覆盖、写失败不启动服务、拒绝 setup 不运行内存默认、首次设置确认后才开始正常监听、向导取消不追加保存、普通字段和托盘修改不顺带重置 prompt。
- 先从未改代码取得内置正常/短版和完整 LLM 文本作为独立对照；更新后的真实 HTTP/WebSocket 请求与对应保存字段逐字一致，LLM 没有二次包裹，正常/短段切换及元数据不变。缺少 prompt 时应报错，不能临时从内置资源补齐。
- 在隔离配置中使用与默认明显不同的诊断 prompt，不经过再次准备而直接构建识别服务，断言请求使用该值；此负对照必须能发现残留的现场默认覆盖。
- lab 候选在服务创建之前保存并加载；正常、短版、空候选和报告元数据一致，主配置字节及源记忆保持不变。
- 临时绕过配置准备时，旧文件升级测试应失败；恢复后重跑初始计划中的 fmt、测试和 Clippy。没有执行这些对照前，不声称默认效果已落盘或识别无覆盖。

**取舍与待办。** 接受完整 prompt 的配置体积和一次新短版字段，以使磁盘配置能够解释模型实际收到的固定指令。不增加版本标记、安装器脚本、并发协调、备份管理或热重载；其他默认设置不在此次范围中另做政策调整。当前仅修订本地方案与审阅摘要，未提交、推送或创建 PR。修订草拟经本地 Opus 5.5 无工具咨询；其中“保留运行时短例子拼接”的建议因不满足本次配置边界未采用，其他建议已与现有代码核对，咨询不是执行验证。

### 002 — 2026-11-13 — 完整提示词落盘与直接消费已实现

**要求与实际实现。** 按用户“实现”的要求完成 [001 修订](#001--2026-11-13--默认策略先修改配置识别不做现场覆盖)。`ConfigStore::load` 保持严格只读；`prepare` 从同一份旧文件内容解析文档及 JSON，生成完整默认文本、补齐可缺省字段，只替换五个 prompt 字段，实际变化时原子保存并重新加载。完整旧配置同样更新，非 prompt 显式值保留，重复准备不重写。损坏 JSON、错误类型、未知字段和旧 schema 不自动覆盖；I/O 或原子保存失败直接中止。

**入口与所有权。** 桌面、显式 setup、普通 convert 在准备成功后才装配服务。缺配置时拒绝 setup 不再返回内存默认监听器，convert 要求先 setup；首次候选先填完整 prompt，最终确认保存并重载。向导取消不追加保存，不撤销此前完成的准备。两个 CLI prompt 键只读，普通字段和托盘设备/增益更新保持原目标字段边界。Realtime 只读取保存的正常/短版文本，旧组件仅用于兼容解析；LLM 配置要求完整字符串，构造时直接消费，不再包裹。删除实验使用的运行时覆盖 helper。记忆 seed 校验改为调用其所属 `MemoryConfig::validate`，不再构造无关的 Realtime 配置。

**实验与文档。** lab 普通评测与 frozen-clock 诊断复用已有临时目录和同一存储边界：保存副本、准备、保存候选、重新加载、再创建服务。HTTP 空候选省略 multipart prompt；Realtime 空候选保存空字符串并清除短版。主配置和源记忆不变，LLM 字段保持副本中准备的默认全文。示例展示完整 HTTP、LLM 及 Realtime 正常/短版文本；配置、CLI、Realtime 和相关架构文档同步了持久化边界、取消行为和降级限制。

**独立正文对照与请求验证。** 实施前临时调用旧生成路径捕获默认配置，更新后的 `config.example.json` 与捕获结果经 `cmp` 逐字一致；临时捕获代码已删除。现有本地 HTTP/WebSocket fixture 验证普通 convert 的请求发出前配置已经保存、请求正文等于保存值、LLM 没有二次包裹，以及 Realtime 在 47,999、48,000、48,001 样本和协议补零短尾处仍选择正确分支。直接用带空白及字面量的诊断指令构造服务，实际请求保留原值，作为残留现场覆盖的负对照。lab 的真实本地 HTTP 无 prompt 请求、候选元数据、私有配置和主配置不变均已验证；没有调用外部模型服务或接触个人配置、历史、记忆和已安装应用。

**定向消融。** 对同一临时旧配置运行 `cargo test --locked startup_replaces_old_prompts_on_disk_without_changing_other_settings -- --nocapture`：旧实现先复现失败；实现后分别临时移除 `document.apply_bundled_prompts()`、仅移除 `self.write_atomic(&value)?`，迁移回归都因磁盘仍保留旧 prompt 而失败。两项均已恢复，最终全套测试通过。因此保留显式默认生成和条件原子保存，未引入版本标记、备份、锁或热重载。[审阅摘要与本地验证证据](https://ampcode.com/artifacts/prompt-f147db30-66c4-40a4-ac98-ec34467c1928)

**完成的检查与限制。** `cargo fmt --check`、`cargo test --locked`、`cargo test --locked --features diagnostic-logs`、`cargo clippy --locked --all-targets --features diagnostic-logs -- -D warnings` 和 `git diff --check` 均通过。已有 vendor/rdev 警告未改动；原有忽略测试保持忽略。setup 的交互语义通过注入 UI 测试验证，未执行物理麦克风或真实模型服务验收。本地验证为 macOS；此前纯文档提交的 Windows CI 在无产品代码改动时因 `scrollbar_probe.ps1` 的 `Pointer coordinates differ` 失败，不将其计为本次实现的通过证据，也不在本次修改滚动条。实现推送后的 macOS/Windows CI 仍待执行。新增短版字段及完整 LLM 文本的旧版兼容限制沿用 001，安装器、发布和个人安装仍不在范围内。

### 003 — 2026-11-13 — live Realtime 测试使用完整配置

**触发与实现。** 双路审查报告 `live_realtime_structured_multiturn` 使用 `DictationConfig::default()`，在新增的必需 prompt 校验处提前失败。用户了解测试范围后要求更新。测试现从 `config.example.json` 读取完整 Realtime 配置，只用既有环境变量覆盖服务地址、模型和可选密钥；正常/短版 prompt 直接来自示例。保留 `#[ignore]`、同连接两轮识别和清理，不改变生产代码，也不恢复运行时默认回退或新增 helper。

**验证与限制。** 修改前显式运行 `cargo test --locked --lib live_realtime_structured_multiturn -- --ignored --nocapture`，设置隔离本地地址，复现 `dictation.prompt is missing`。修改后临时复用仓库的本地 WebSocket fixture，在子进程运行原 live 测试，分别验证未设置模型覆盖及指定 `validation-model-override`：服务核对实际模型、空 session instructions、完整保存的 response instructions、两轮关联与清理，两个子进程均通过。临时验证代码随后删除。最终 `cargo fmt --check`、普通及 diagnostic-logs 全套测试、CI 同款全目标 Clippy 通过，默认忽略边界保持不变。[验证日志与审阅结果](https://ampcode.com/artifacts/prompt-f147db30-66c4-40a4-ac98-ec34467c1928)

仅调整现有测试的数据来源，没有新增运行机制，无需新增消融；修改前失败与修改后协议验证已覆盖差异。没有连接外部模型服务、使用真实密钥或修改个人状态，不将模拟服务成功表述为真实模型质量验收。按同一 bookmark / PR 交付，推送前再次运行双路门禁；这次用户要求仅授权本修复及一次后续审查。

### 004 — 2026-11-13 — 示例配置作为默认来源，识别只拼接已保存组件

**要求与状态。** 用户明确要求：“配置都写在config.example.json，更新配置时，更新config.json的对应字段，识别时使用这些组件拼接，短prompt只是少用了一部分组件。”这条要求变更了 [001](#001--2026-11-13--默认策略先修改配置识别不做现场覆盖) 的保存形式：配置保存组件，识别阶段允许对已保存组件作固定顺序的纯拼接；禁止现场读取另一套默认正文或替换组件内容的约束不变。以下是待确认的实施修订，本轮仅更新计划，没有修改产品或测试代码。

**单一默认配置。** `config.example.json` 成为新配置和配置更新的默认来源，构建时嵌入程序，安装后不读取仓库相对路径。示例直接维护 HTTP 的 `transcription.prompt`、LLM 的完整 `post_process.prompt` 和 Realtime 的 `dictation.prompt_components`；Realtime 的 `prompt` 保留为 `null`，不再保存一份重复的正常全文，移除示例中的 `short_prompt`。将现有组件正文移入示例后删除 `assets/dictation-prompt.default.json`、`PromptComponents::default`、Rust 中的 LLM 默认正文生成器和短版替代示例，不保留需要手工同步的第二份 prompt 默认来源。LLM 继续使用配置中的完整 system 文本，不恢复运行时包装。

`ConfigDocument::new(secrets: impl SecretSource + 'static) -> Self` 改为解析嵌入的示例并绑定传入的凭证来源，不触发凭证读取。`ConfigDocument::default()` 仍委托该入口，避免新建配置与示例不同。`apply_bundled_prompts(&mut self)` 保留原接口，改为从示例默认文档复制对应 prompt 字段、清除旧全文和旧短版，不再生成最终 Realtime 文本。serde 解析旧文件所需的结构缺省不能再次调用整份示例解析，避免 `DictationConfig` 的容器级 `#[serde(default)]` 递归；这些兼容缺省不生成 prompt，也不是新配置或准备阶段补缺的默认来源。

**更新只修改对应字段。** `ConfigStore::load(&self) -> Result<Option<ConfigDocument>, ConfigError>` 继续严格只读。`prepare(&self) -> Result<Option<ConfigDocument>, ConfigError>` 继续先严格解析同一份旧文件；补缺的值改为取自示例默认文档，实际覆盖 `transcription.prompt`、`post_process.prompt`、`dictation.prompt`（设为 `null`）和 `dictation.prompt_components`，删除退役的 `dictation.short_prompt` 键。显式非 prompt 设置保留，内容有变化才原子保存，然后重新加载。写失败不创建识别服务，坏 JSON、错误类型和未知字段仍不覆盖。不把整个示例直接覆盖用户配置，也不新增安装器钩子、版本标记、备份、锁或热重载。

**正常与短版的组件规则。** `PromptComponents` 保留现有字段和严格解析形状。`compose(&self) -> String` 按 `task`、`language`、`numbers`、`cleanup`、`output_format`、`context`、`memory`、`examples` 的固定顺序拼接非空组件。`compose_for_segment(&self, short: bool) -> String` 在短段时仅省略 `examples`，其余组件的内容和次序不变；不添加空结果示例或任何代码中的正文。没有独立 `examples` 时两版相同，不扫描或截断其他组件中看似示例的文字。源音频严格小于 2 秒才使用短版，恰好 2 秒使用正常版，协议补零不改变判断。

`DictationConfig::prompt(&self) -> String` 和 `short_prompt(&self) -> Option<String>` 保留调用接口，改为从已保存组件组合正常/短版；已有连接仍可预先组合后按段选择，不在每段补默认。保留全文字段用于旧文件解析和 lab 候选。`validate(&self) -> Result<()>` 要求全文与组件恰好有一个来源：两者都缺时提示先准备配置，两者同时存在时报来源冲突，不静默选择一项。只有全文时原样使用，短段不另外改写；显式空全文仍供 lab 无 prompt 候选使用。继续校验最终文本的 Unicode 字符长度上限。

**旧短版与实验兼容。** 该 PR 尚未发布，但此前分支版本已能保存 `short_prompt`，因此不因删除字段而阻断其配置更新。将现有字段收窄为 `legacy_short_prompt: Option<String>`，以 serde `rename = "short_prompt"` 兼容读取、`skip_serializing` 禁止再次保存；它仅供准备阶段识别并清除，不作为指令来源。带有旧短版值（包括空字符串）的直接识别配置应报需先准备的错误，避免静默忽略用户保存内容。正常保存与更新后不再输出该键，其他未知字段仍严格拒绝。这只是现有字段退役，不增加通用迁移框架。

lab 沿用既有隔离文件顺序：先准备副本，再将候选写入 `dictation.prompt`，同时清除组件和退役短版，保存、重载后才创建服务。无候选时使用保存组件；显式候选和空候选对所有段长使用同一全文，HTTP 空候选仍省略 multipart prompt。报告中的 `stt.short_prompt` 继续记录实际组合结果，该报告字段与退役的配置字段是不同接口，不随配置字段一起删除。主配置和源记忆保持不变。

**取舍与影响。** 不采用继续保存两份全文、维护重复 asset 或运行时替换短示例的方案；不新增可配置组件选择列表。相比 001，正常 Realtime 和完整 LLM / HTTP 文本应逐字保持，短版有一项有意变更：只省略全部独立示例，不再附加空结果示例。示例的其他值也成为新安装默认值，例如当前 `audio.mic_gain` 为 `3.0`，已有显式增益不变。旧版 LLM 会重新包装完整 system 文本，降级仍应配套旧配置；不承诺本次变更提高真实模型识别质量。

**实施顺序与模块。** 确认后先扩展现有配置升级和本地 WebSocket 测试，观察当前实现不能满足组件落盘与短版纯省略；随后修改 `config.example.json`、`src/core/config/document.rs` / `store.rs`、`src/dictation/config.rs`、`src/prompt_lab/production.rs` 及相应 setup / 识别测试，移除冗余资源。保留桌面、setup、convert 的准备入口和取消边界。同步配置、Realtime、core/dictation/postprocess/prompt-lab 架构文档、CLI / UI 的关联描述、changelog 和本 feature record；仍在原 bookmark / PR 交付，代码推送前运行同一双路审查门禁。

**待执行的验证与消融。** 使用现有测试文件覆盖用户可观察的契约，不按每个 helper 增加重复测试：

- 从旧全文、空值、缺字段、旧组件和上一轮正常/短版文件准备配置，实际磁盘 prompt 字段与示例一致、旧短版键消失，非 prompt 设置逐项保持；第二次准备的字节和修改时间不变。错误类型、非法 JSON、未知字段和写失败不改变安全边界。
- 用各组件不同的诊断文本作独立期望，验证正常固定次序、短版仅省略 `examples`；空/缺少示例时两版相同，其他组件中的示例状文字保留。直接用非默认组件构造识别服务，核对真实本地 WebSocket 请求没有默认回退或额外正文。
- 复用 47,999 / 48,000 / 48,001 样本、补零短尾和同连接多轮 fixture，核对请求恰好等于保存组件的对应拼接结果，排除阈值及会话串用错误。
- HTTP 和完整 LLM 请求逐字匹配示例落盘值；新配置与示例一致，缺配置 / setup 拒绝和取消、只读检查与普通字段更新不回归。隔离 lab 候选能覆盖全部段长，元数据、主配置和源记忆边界不变。
- 临时恢复当前硬编码短例子时，同一个短版测试必须失败；临时略过组件落盘或保留 lab 组件时，对应升级或来源冲突测试必须失败。恢复后重跑 fmt、普通 / diagnostic-logs 测试、CI 同款 Clippy 和文档链接 / JSON 检查。

上述产品检查尚未执行。修订草拟经本地 Claude Opus 5.5 无工具咨询，已核对其模型及空工具列表；不采纳“拒绝分支内旧短版文件”和删除全文键的建议，以保留更新路径及稳定的可空字段形状。咨询不构成执行验证。此前 [be36aab 的 macOS / Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/43450252609) 已全部通过，仅覆盖 001–003 的实现，不能用作本修订的通过证据。
