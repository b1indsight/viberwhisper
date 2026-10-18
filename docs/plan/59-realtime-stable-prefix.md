# 59 — Realtime 稳定前缀、音频与动态上下文

**阶段：** 客户端实现、本地检查、跨平台 CI 与最终实模对照完成；缓存性能与完整质量验收待补，PR 保持 Draft。
**Issue：** [#152](https://github.com/b1indsight/viberwhisper/issues/152)
**PR：** [#153](https://github.com/b1indsight/viberwhisper/pull/153)；方案与实现沿用此 PR。
**基线：** `master@origin`，`1597ed6d`。

## 1. 初始方案

### 动机与现状

目前连接建立时发送固定 instructions，但 `Recording::finish_segment` 在音频上传结束后
才读取完整记忆 Snapshot。`Connection::finish_inner` 的显式输入顺序为 Snapshot 文本、
音频引用。人工词表、最近历史、模型候选和确认纠错混在音频前；第一包音频到达服务端时，
服务端尚不知道本段实际使用的状态。这会妨碍录音期 prefill 与最终请求复用相同前缀。

依据：[客户端请求构造](https://github.com/b1indsight/viberwhisper/blob/1597ed6d/src/dictation/transport.rs#L261)、
[分段结束时读取记忆](https://github.com/b1indsight/viberwhisper/blob/1597ed6d/src/dictation/engine.rs#L410)。

[#152 最新约定](https://github.com/b1indsight/viberwhisper/issues/152)要求完整逻辑顺序为
固定指令 / 稳定状态 → 音频 → 本次动态上下文。关联
[服务端 PR #7](https://github.com/b1indsight/gemmae4b-server/pull/7)已合并；其文本中保留的
早期 Draft 状态不作为当前状态依据。本文核对的服务端版本为
`c64199895f02a0fb07b967c7285f080b7c3234f8`，不假定该版本已部署。

### 目标、范围与验收标准

- 第一包非空音频 append 之前，固定 instructions 与本段稳定文本 item 均已获得服务端确认。
- `response.create` 保留 `conversation:"none"`，显式输入依次引用稳定文本、音频，
  再携带动态文本；保留输出配置、`dictation_id`、严格结果校验和确认反馈。
- 同一段的已声明前缀保持不变。不同录音的前缀可相同、延长、缩短或改变；每次都提供完整
  输入或本连接内已确认且仍有效的引用，不按缓存命中猜测删除内容。
- 保持现有有界记忆、话题隔离、长音频分段、成功确认后学习、部分失败和取消语义。
- 覆盖桌面及复用同一 Recording / Connection 的 Realtime prompt-lab；HTTP 转写接口不变。
- 实际 Qwen 验证必须核对 canonical messages/template 的 token 顺序、有效缓存边界和
  新顺序的质量，不能用旧模板 WER 或只看网络发送顺序代替。

本 PR 只修改 viberwhisper。服务端变更、部署、跨连接缓存、客户端 KV 计算和日期缓存策略
不在范围内；不增加用户配置开关，不为了命中率填充假 token 或减少业务所需信息。

### 拟提供的行为

例如人工配置含 ViberWhisper 词表、历史含上一段对话时，先声明该人工词表，待确认后上传
本段音频；结束时追加最近历史、候选及纠错，再请求结果。下一段只更新历史时，音频前
文本保持逐字一致。人工配置改变或重连时，重新声明完整前缀，使用新的服务端 item ID。

超时、拒绝或不明确的确认使当前连接退出复用；不会继续发送音频或静默回退到旧排列。
服务端缓存 miss、淘汰或只复用较短前缀均属于正常情况，不改变客户端输入完整性。

### 技术设计与涉及模块

#### 业务拆分及稳定序列化

固定 instructions 继续来自 `DictationConfig::prompt()`，保留全文覆盖、组件组合和空字符串
语义。稳定状态只选择用户配置的 `long_term` 人工词表及其话题 ID / label；它们在当前
应用配置生命周期内稳定。运行时学习的 `candidates`、`supporting_recordings`、`history`
和确认生成的 `corrections` 全部放在音频后的动态上下文。纠错虽然持久化，仍会在确认后
变化，因此本次不把它冻结到音频前，也不新增日内冻结或自动晋升规则。

由记忆所属模块提供共享的有类型投影；原 Snapshot 继续表示合并后的业务视图，供结果 ID
校验、记忆学习和场景断言使用。wire 文本分成稳定与动态两部分：同一话题通过相同 ID
关联，人工词表正文只在稳定部分出现一次，动态部分承载本段可用的其余字段。新出现的
话题可只在动态部分定义。所有可引用的话题 ID 来自两部分的联合视图。

保持最多四个话题和现有 6000 字符业务预算；在同一个预算分配入口中为稳定词表预留空间，
动态部分使用剩余预算，并沿用人工词表优先、纠错优先于候选和历史的裁剪原则。稳定部分
在声明前完成自身的有界选择；结束时不能因新的历史挤占空间而重新裁剪已声明词表。
预算测试应覆盖长人工词表与新动态话题竞争的边界，防止 wire 拆分绕过全局限制。

使用固定字段顺序的 serde 结构和紧凑 JSON；人工词表保留具有业务意义的配置顺序，
动态内容保留现有优先级与时效。相同配置得到相同稳定字节，不加入日期、时间戳、随机 ID
或支持次数等动态字段。两部分为空时都使用 `{"contexts":[]}`，仍发送对应文本项；
这样空值与非空值使用同一声明和输入协议。协议 item ID / metadata 不混入模型文本。

默认 prompt 的 context 说明补充两部分按话题 ID 联合理解的约定，配置示例同步；不自动
改写用户自定义 prompt。新质量比较同时记录实际 instructions 和输入布局身份，避免把
模板说明变化隐藏在缓存实验里。

#### 声明、确认和最终输入

1. 建立连接并等待 `session.created`；发送现有 `session.update` 并等待 `session.updated`。
2. 每段开始时绑定 recording / segment 身份及不可变稳定投影，发送一个 user 文本
   `conversation.item.create`。复用连接时也显式声明，不依赖旧会话中的隐式状态。
3. 等待对应文本项完成确认，再允许第一包音频 append。已核对的服务端发送
   `conversation.item.added` 和 `conversation.item.done`，并自行生成 ID，忽略客户端
   自定义 item ID；客户端须从已验证的返回 item 获取 ID，校验 role、content、完成状态，
   并匹配同一 item 的完成事件。不能只等待任意 item 事件。
4. 音频结束后 commit，保存 `input_audio_buffer.committed.item_id`；从本段工作记忆
   构造后置动态投影，保留本段先前声明的稳定文本。
5. `response.create.response.input` 固定为稳定 `item_reference`、音频 `item_reference`、
   一个携带动态 JSON 的 inline user 文本 message。inline 动态文本不写入连接对话，
   因此下一段的 speculative prefill 不会看见旧后置上下文。
6. 校验 response 的 ID、`dictation_id`、完成状态及 JSON 后，清理本段音频和稳定文本项，
   等待各自的删除确认；只有两项清理成功后才可正常复用连接。

协议依据：[服务端 item 确认及 ID 分配](https://github.com/b1indsight/gemmae4b-server/blob/c64199895f02a0fb07b967c7285f080b7c3234f8/gemmae4b_server/session.py#L530)、
[显式输入按序解析](https://github.com/b1indsight/gemmae4b-server/blob/c64199895f02a0fb07b967c7285f080b7c3234f8/gemmae4b_server/realtime.py#L125)、
[录音期前缀构造](https://github.com/b1indsight/gemmae4b-server/blob/c64199895f02a0fb07b967c7285f080b7c3234f8/gemmae4b_server/qwen_realtime.py#L161)。

#### 生命周期与错误处理

前缀声明、前缀 item ID、音频 item ID、response 关联身份均由本段持有。Connection 在
未完成声明时禁止 append；结束时只接受与已声明稳定投影一致的输入。使用能表达声明中、
录音中和提交后状态的局部类型 / 状态，避免散落多组含义重叠的可选字段。

连续分段在上段完成清理后才开始下一段声明；同一录音中的稳定投影不因段间临时学习而
改变，后置动态上下文仍可包含前段结果。worker 继续先处理待确认反馈再接收下一录音，
不引入额外并发队列；排队的音频保留在现有有界通道中。

静音段先 clear 未提交音频，再删除稳定文本；下一段重新声明。无音频录音不创建空音频
或请求生成。取消覆盖声明等待、append、commit 和 response 创建等待，按已确认状态处理
response.cancel / clear / delete；未知结果或清理失败时直接退休连接。已验证结果在清理
失败时沿用现有部分失败语义，不学习失败录音。

重连清空所有 item 身份，下一次新录音重新声明完整输入。保留现有不自动重放不明确提交
的原则；失败后用户重新录音使用新身份，不重复提交旧音频。上一段晚到事件不能满足本段
确认或结果匹配。未来若允许改动活动段的前缀，必须显式清理并重建整段，不能原地覆盖；
本次不给活动段提供修改入口。

#### 模块职责及评测身份

- `src/dictation/memory.rs`：稳定 / 动态投影、联合 ID 和统一预算；不改变个人记忆文件格式。
- `src/dictation/transport.rs`：声明确认、append 前置条件、显式输入顺序和两类 item 清理。
- `src/dictation/engine.rs`：在首包发送前建立本段输入，绑定身份，处理分段、静音与取消。
- `src/dictation/test_support.rs` 及 transport / engine 测试：升级协议桩并验证实际事件边界。
- `src/dictation/evaluation.rs`、`src/prompt_lab/realtime.rs` 及相关报告接线：同一生产投影；
  trace 保留有效合并 Snapshot，并记录本段实际稳定 / 动态输入以便重放校验。
  新 Realtime 场景策略升级为下一版本；旧报告按原身份读取，不能冒充新排列的证据。
  普通 Realtime 评测也须记录布局版本，禁止旧新模板被当作相同请求身份直接比较。
- `assets/dictation-prompt.default.json`、配置示例、架构 / 评测文档及 `changelog`：同步约定。

### 备选方案与取舍

- 只在结束时调换 Snapshot / 音频顺序：实现较小，但人工词表也会移到音频后，录音期无法
  声明实际稳定状态，不能满足 issue。
- 开始时冻结完整 Snapshot：较容易保留原结构，但历史、候选及确认纠错会破坏共享前缀，
  并丢失本录音前段刚产生的动态上下文；也违背 issue 已更新的输入分工。
- 稳定词表放进 instructions：少一个文本 item，但混合任务规则与业务状态，改变既有
  prompt 全文覆盖语义；本方案保留独立文本项，代价是每段一次声明确认及删除。
- 缓存文本 item 跨多段复用：可少一次协议往返，但增加 item 失效、重连和引用计数状态。
  本方案每段显式声明；服务端 KV 是否继续保留独立于客户端 item 删除。

### 成本、风险与缓解

新增一次声明确认会推迟首包上传；录音仍由原有有界通道接收。确认超时应明确失败，
不无限积压。确定性测试覆盖未确认时已有音频排队与取消，联调分别记录首包等待和停录后
延迟，避免只报告其中一项。

拆分同话题字段可能改变模型理解；默认说明与实模结构化检查共同验证，手写 prompt
继续按用户原文生效。更改输入排列需要新质量评估，不能承诺现有 WER 或速度收益。
手动清理新增 item 的失败会减少连接复用，但可阻止旧文本污染下一段。

### 实施顺序

1. 实现共享投影、确定性序列化及预算约束，并用最小业务场景验证信息保留与身份联合。
2. 实现文本声明确认和逐段所有权，改造 Recording / Connection 的首包及清理路径。
3. 更新协议桩和关键生命周期回归，确认旧排列无法通过新增顺序测试。
4. 同步生产评测与报告身份、默认说明和文档，再运行本地质量检查。
5. 联调实际 Qwen 并完成新模板质量 / 缓存对照，记录条件与结果，然后运行代码审阅 gate，
   推送同一 bookmark，继续同一 PR，等待 CI；不在本任务中自动合并。

### 验证与消融策略

**确定性测试。** 使用现有本地 WebSocket 服务桩，按需控制确认和完成事件；不依赖真实
sleep 或外部模型完成时间。覆盖如下行为，避免为简单序列化函数增加重复测试：

- 空 / 非空稳定及动态状态；相同稳定内容的字节一致；前缀延长、缩短及后半部分变化；
  仅动态字段改变时前缀不变；跨日动态过期不影响稳定字节；联合预算和 ID 校验。
- withheld / 错误 item 确认时绝无首包 append；最终输入三个位置及 payload / references
  正确；结果 metadata、输出配置和解析未丢失；明确拒绝活动段前缀替换。
- 连续短录音、长录音多段、静音 clear、未收到音频、确认反馈更新、排队及上一段晚到事件。
- 声明中、commit 确认前、response 创建确认前及生成中的取消；删除失败；断线后新录音
  重连；不得使用旧 ID、重复上下文或自动重放已不明确提交的音频。
- evaluator 实际请求及 trace 重放与生产相同；失败不提交学习；旧新报告身份可区分。

**项目检查。** 实施后按贡献指南执行 `cargo fmt --check`、`cargo build --locked`、
`cargo test --locked`、`cargo clippy --locked -- -D warnings`，并由 Windows CI 验证对应
`windows-app` 构建、测试及 clippy；本地不把未执行的跨平台检查标成通过。

**Qwen 联调与消融。** 实施后、自动代码审阅前，以同一模型 checkpoint、fixed64 计算
约定、WAV、instructions、生成配置及完整最终消息进行以下对照：

1. 检查真正 canonical template 的 token 顺序，以及第一包前声明确实被接受。
2. 比较冷连接与同连接热前缀；比较相同、延长、缩短及修改后半部分的稳定词表；仅改变
   后置上下文时单独检查前面音频 KV 是否保留。以服务端报告的实际有效 token 边界判断，
   接受完整 64-token 块的向下对齐，不按字符数估计命中。
3. 使用相同最终输入的冷 fixed64 路径核对文本、结构化输出与完成状态；以旧排列作为
   单独质量对照，重新评估现有 ready 音频和相关多段场景的 WER、专名及语境断言。
   旧模板历史 WER 不用作本次新顺序的通过依据。
4. 消融时去掉提前声明、其余最终输入保持相同，比较 prefill 是否退回响应阶段；关闭
   缓存 / 使用冷连接作为完整输入仍能工作的一项验证。只在实际服务可提供该控制时使用
   对应开关，不为测试增加产品配置。
5. 分列缓存命中、实际复用 token、计算时间、首包等待及真实停录后的延迟。保留服务端
   pre-commit / pre-response / before-generation 指标的原始含义；没有录音时间轴证据时，
   不把 pre-commit 计数写成录音期间完成量，不据单个样本声称普遍性能收益。

**证据。** 私有 WAV、临时客户端 / 服务端日志、比较脚本和完整报告仅留在本项目
`.cache/<会话开始日期>/`，公开记录仅写非敏感条件、命令、计数与结论。

### 待确认与实施时核对项

批准前请审阅上述业务拆分：仅人工词表属于稳定状态，运行时确认纠错留在动态上下文；
使用逐段创建 / 删除 item，不引入跨段 item 缓存。本方案本身不要求新增产品配置。

实施时需核对实际部署端点、模型版本、缓存指标来源，以及当前可复用的 ready 音频和
冷 fixed64 对照入口。服务不可用或缺少可核对的 token / 质量证据时，将对应验收明确
标为未完成，不用协议桩结果代替实模验证，也不宣称 #152 已完整解决。

## 2. PR 变更时间线

<!-- 初始方案作为审阅基线；后续实质需求、决策和实施变化按发生顺序追加。 -->

### [001] 2026-10-18 — 实现声明前缀与后置动态上下文

- **触发：** #152 要求录音期与最终输入使用同一个已确认稳定前缀。
- **实现：** `memory/input.rs` 保存人工词表投影和动态 Snapshot。`Recording` 首包前冻结
  人工词表；`Connection::begin` 创建文本项，校验返回内容、完成状态和服务端 ID 后才允许
  append。长录音每段重新声明，整次录音的稳定词表保持一致。
- **协议：** 同一连接显式发送稳定引用 → 音频引用 → inline 动态上下文；保留 instructions、
  输出配置、metadata 和严格结果解析。接收 added/done，也兼容 completed 的单次 created
  确认。空部分均为 `{"contexts":[]}`。动态正文不写入连接对话。
- **生命周期：** 完成和静音清空均清理所属文本项；已提交音频另行清理。取消、失败、重连
  不复用上段引用；保留原有失败连接退休、已验证部分结果、确认后学习和有界队列规则。
- **文档与身份：** 默认 context 规则解释两部分按 ID 组合，用户覆盖保持原文；配置示例
  同步。场景报告升级至 v4，trace 保存两部分实际投影；普通及场景 Realtime 报告都标记
  `stable-prefix-audio-dynamic-v1`。跨布局比较在联网前拒绝。

### [002] 2026-10-18 — 统一预算投影，保留历史报告语义

- **背景：** 新动态话题即使没有历史内容，其 JSON 外壳仍占空间。直接重新裁剪旧完整
  Snapshot 会挤掉已经声明的人工词表；只在 wire 层处理则可能使场景断言偏离实际输入。
- **决定 / 实现：** `Memory::snapshot` 统一使用稳定词表优先的联合投影，最多四个话题、
  6000 字符。删除动态内容后仍超预算时，移除动态专属话题外壳；不改写已声明词表。
  分配预算与重建业务视图共用一个话题合并入口。
- **兼容：** 显式保留旧 `legacy_snapshot`，只用于 v1/v2/v3 报告的前后状态和旧 trace
  重放；v4 必须携带新输入证据。预算边界回归先学入新话题，再验证旧策略 trace 仍可重放。

### [003] 2026-10-18 — 消融和删除期间取消的回归

- **提前声明：** 最初新增实际 worker 路径测试时，旧实现直接到达 audio commit，未出现
  文本声明，`recording_declares_prefix_before_audio_and_orders_final_input` 失败；完整实现通过。
- **文本清理消融：** 临时省去文本项 delete，静音 / 空录音 / 下一段场景在本应收到 delete
  时收到下一段 create，回归失败；恢复清理后通过。证据保留在本地
  `.cache/2026-10-18/issue-152-no-prefix-cleanup.log`。
- **联合预算消融：** 临时省去联合裁剪，`dynamic_context_cannot_evict_a_frozen_prefix_at_the_joint_budget_limit`
  在 6000 字符断言失败；恢复后通过。证据为
  `.cache/2026-10-18/issue-152-no-joint-budget.log`。临时消融及质量运行接线均已移出产品源码。
- **删除期间取消：** 检查清理时发现，在 audio delete 或 prefix delete 已发送、确认尚未
  返回时取消 future，会使重新清理重复发送旧 ID。新增确定性回归先复现重复 audio delete，
  再为两类 item 记录删除中状态、逐项消费确认；恢复只等待未完成确认，已删除音频不再
  携带旧 ID。`cancellation_during_cleanup_never_repeats_an_item_delete` 两种中断点均通过。
- **取舍：** 保留声明、联合预算及逐项清理状态，因为上述更简单实现分别违反首包顺序、
  有界输入或 item 生命周期要求。没有用客户端猜测缓存命中，或添加产品实验开关。

### [004] 2026-10-18 — 本地检查与实际 Qwen 对照

**本地检查。** `cargo fmt --check`、`cargo build --locked`、`cargo test --locked -q`、
`cargo clippy --locked --all-targets -- -D warnings` 通过；主测试 313 passed、2 ignored。
另执行 CI 使用的 `cargo test --locked --target-dir target --manifest-path vendor/rdev/Cargo.toml --lib viberwhisper_tests -q`，
1 passed。已有 rdev 的两项 macOS 编译告警保持原状。Windows 交给本 PR 的 CI 验证。

**真实输入及冷连接。** 在用户指定服务上的 `giangndm/qwen2.5-omni-7b-mlx-4bit` 运行
`live_realtime_structured_multiturn --ignored`：空词表、延长、缩短、改变后半部分，以及
仅后置历史变化，共五组复用连接输入；随后五次新连接使用逐组完全相同的最终消息。
全部返回合法结构化结果，五组文字均与对应冷连接一致。音频不按真实录音时间轴 pacing，
该结果不证明录音期间的完成量，也不证明热连接实际命中了多少 KV。

复跑命令（端点通过测试环境变量显式提供）：

```bash
VIBERWHISPER_REALTIME_TEST_URL="$QWEN_REALTIME_URL" \
VIBERWHISPER_REALTIME_TEST_MODEL=giangndm/qwen2.5-omni-7b-mlx-4bit \
cargo test --locked live_realtime_structured_multiturn -- --ignored --nocapture
```

**质量对照。** 使用已有 19 条开发录音、10 条曾使用的留出录音的隔离副本，实际生产
`OfflineDictation` / `RealtimeEvaluator` 和项目 WER、专名评分器。基线是 `1597ed6d` 的旧
客户端；两侧均覆盖相同完整 ready 集，同一服务、WAV、语言和实际 instructions。
两侧数据 digest 及 prompt SHA-256 相同；本轮 prompt SHA-256 为
`68255f64e95c67b6f3df87601dfcda5864450dd6ea79ce7720528edb6d65db92`。

| 场景 | 条数 | 旧顺序 WER | 新顺序 WER | 旧 / 新专名准确率 | 传输 / 转写失败 |
| --- | ---: | ---: | ---: | ---: | --- |
| 开发集，无记忆 | 19 | 23.7838% | 18.3784% | 72.7273% / 72.7273% | 两侧均 0 |
| 曾使用的留出集，无记忆 | 10 | 12.0690% | 15.5172% | 66.6667% / 66.6667% | 两侧均 0 |
| 留出集，人工词表场景 | 10 | 21.5517% | 19.8276% | 75% / 75% | 完整对照两侧均 0 |

人工词表新场景的全部本地语境断言通过；旧顺序场景有断言未通过。第一次新词表场景
在一条 send timeout 后又遇到两次 `session_unavailable`；另起完整独立回归得到 10/10
完成，原失败报告保留并排除在完整质量对照之外。没有在同一次录音中重放不明确的音频。
无记忆留出集中，一条模型名称和版本的连写变化增加了分词错误；不能据开发集改善声称
所有子集改善。上述是客观指标；报告仍待语义审阅，质量阈值未整体通过。

历史场景的新顺序回归尚未完整完成：一条 response timeout 后遇到四次
`session_unavailable`，另有已完成样本的 metadata 断言失败；不使用五条成功样本的分数
冒充十条场景的完整指标。独立重跑仍有一条 response timeout 和三次
`session_unavailable`。旧顺序同条件十条全部完成，WER 15.5172%、专名 66.6667%，
语境检查通过。这些初始新顺序结果说明不能只凭协议通过就宣布历史场景验收。

**本地证据与复现。** 完整报告、音频副本和公共 prompt 保留在
`.cache/2026-10-18/issue-152-quality/`，私有临时 harness 为
`.cache/2026-10-18/issue-152-quality.rs`。harness 调用生产 evaluate，只通过环境变量
选择布局身份和现有 cases；临时接入相应工作区的 lib 测试模块后执行
`ISSUE152_MODE=<old|new|new-vocabulary-retry|old-vocabulary|new-history> cargo test --locked issue_152_live_quality -- --ignored --nocapture`，
场景运行另提供 `ISSUE152_CASES`。正式源码不包含本机 harness 路径。公开文档不复制私有
音频、转写正文或个人状态。

**剩余验收。** 用户说明目前不能读取服务端 prefill 日志，因此实际缓存命中、有效 token
边界、计算时间及录音结束后延迟没有验收证据；没有把 cold/warm 文字一致当作缓存命中证明。
历史场景亦未通过完整验证。客户端实现及协议回归已完成，PR 保持 Draft，等待这些验收项。

### [005] 2026-10-18 — 明确固定提示词中后置参考与音频的关系

- **背景 / 证据：** 初始历史场景重复在同一条录音等待 response 时超时；已完成的首条
  样本还出现 uncertain 语境下返回候选、候选 quote 无法支持词语的元数据。客户端按
  既有规则保留文字并丢弃不合格记忆，但这不能代替模型侧场景验收。
- **备选与决定：** 保持请求顺序和输出配置，只增强默认 context 组件对新排列的说明；
  不静默退回 context-before-audio，不提高超时上限，也不更改用户已有自定义提示词。
  新句子为“音频后的参考JSON只用于帮助听写前一条音频；读取它后继续输出同一段音频的
  听写JSON，历史内容不写入本次text。”它仍位于音频前的固定 instructions 中。
- **已实现：** 默认 JSON 资源和配置示例同步追加该句，架构文档说明自定义 prompt 的
  迁移要求。协议、文本 item 序列化、模型输出解析和业务预算保持本 PR 的既定实现。
- **缩小样本试验：** 仅取上述两条触发录音，原有历史 seed、模型和生成配置保持一致。
  增强说明后 2/2 完成、语境检查全部通过；WER 4%，专名 2/3。证据为本地
  `.cache/2026-10-18/issue-152-history-pilot/new-history-pilot-holdout.json`。
- **验证边界：** 两条试验只支持继续采用该说明开展完整回归。新的普通 / 词表 / 历史
  矩阵使用同一份最终 instructions，完整证据放在独立的
  `.cache/2026-10-18/issue-152-quality-final/`，避免与 [004] 的初始提示词结果混用。
  最终矩阵仍在运行；本条不声称已经通过全部实模质量验收或获得缓存性能收益。

### [006] 2026-10-18 — 最终提示词完整对照与提交审阅

**完整质量对照。** [005] 的最终固定说明已用于全部回归。旧客户端基线 `1597ed6d` 与
新客户端各完成 49 次请求：19 条开发录音、10 条留出录音、10 条人工词表场景和 10 条
历史场景。两侧均无传输或转写失败。两侧使用相同 Qwen 服务、模型、WAV、语言、场景
seed 和最终 instructions；逐组核对 ready sample ID、数据 digest、场景内容及 prompt
SHA-256 相同。最终 prompt SHA-256 为
`dfa543dabaa929c91703958cabc3d862a99ef655478e4a308853d109e90c62f8`。

| 场景 | 条数 | 旧顺序 WER | 新顺序 WER | 旧 / 新专名准确率 | 场景检查 |
| --- | ---: | ---: | ---: | ---: | --- |
| 开发集，无记忆 | 19 | 20.0000% | 14.5946% | 72.7273% / 72.7273% | 不适用 |
| 曾使用的留出集，无记忆 | 10 | 15.5172% | 15.5172% | 66.6667% / 66.6667% | 不适用 |
| 留出集，人工词表场景 | 10 | 19.8276% | 19.8276% | 75% / 75% | 两侧各 310/310 |
| 留出集，历史场景 | 10 | 13.7931% | 13.7931% | 75% / 75% | 两侧各 370/370 |

最终完整历史场景覆盖 [004] 的触发录音，未再出现此前的 response timeout 或 metadata
检查失败。初始失败及 [005] 的两条试验仍保留为独立证据，不混入最终矩阵。上述结果只说明
本轮样本中的客观质量与场景检查；WER 和专名仍有未达目标的项目，报告状态仍为
`awaiting_agent_review`，`meets_targets` 未被置为 true，不能据此声称全部语义质量门槛
通过或普遍改善。

**证据与复现。** 最终八份 JSON 及汇总 `comparison-summary.json` 保留在本地
`.cache/2026-10-18/issue-152-quality-final/`；开发集 digest 为
`1e1334f72e13af4bfa4b15d1f2f34ea2d9463536e1df2c7d88de38f72504cd68`，留出集为
`10bd6f65aa657ce56cfdeab781bf467d83b669b4f2236aa5b46d4947a62fd057`。
沿用 [004] 的私有 harness，以 `ISSUE152_MODE=old-final` 或 `new-final` 和
`ISSUE152_QUALITY_ROOT` 指定最终目录，串行执行同一测试命令；两侧临时 lib 接入均已移除。

**审阅与 CI。** 独立 Codex review 对代码提交
`099406be69e88c47d23cb3a352738e56bce5d895` 返回无 findings，报告为本地
`.git/jj-reviews/rlwquzluqozt-099406be69e8.json`。该提交的
[macOS 与 Windows CI](https://github.com/b1indsight/viberwhisper/actions/runs/39130114954)
均成功；本地 313 测试通过、2 个测试默认忽略，fmt、locked build、all-targets Clippy
及 CI 使用的 rdev 定向测试通过。本条只补充验证文档，代码保持该审阅提交的内容。

最终固定说明下再次执行正式忽略测试 `live_realtime_structured_multiturn`：同一连接五轮
与五个新连接的完整输入逐组相同，覆盖空前缀、延长、缩短、尾部变化及仅动态后缀变化。
10 次均返回合法结构化结果，五组文字全部一致，测试耗时 73.84 秒。该测试不按录音时间
轴发送音频，不能证明录音期间的计算进度或实际 KV 命中。

**剩余边界。** 用户说明目前不能读取服务端 prefill 日志；实际命中 token 数、有效缓存
边界、prefill 计算时间和停录后延迟仍无验收证据。PR 继续保持 Draft，不自动合并。
