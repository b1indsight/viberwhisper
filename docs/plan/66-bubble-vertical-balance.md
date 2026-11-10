# 66 - 修正气泡底部留白，让识别文字垂直居中

**阶段：** 计划待批准；已完成获准的 Windows 原生组件测量，尚未实施布局修复。初始方案与历次调查限制保留，当前实测结论见 [004]。
**PR：** [Draft #172](https://github.com/b1indsight/viberwhisper/pull/172)，仅计划与临时只读诊断；未合并、未发布修复。
**Bookmark：** `fix/bubble-vertical-balance`；后续计划与实现沿用同一 bookmark / draft PR，目标分支为 `master`。
**基线：** 已 fetch 的 `master@origin` / v0.5.3，[发布准备提交 34288ff9](https://github.com/b1indsight/viberwhisper/commit/34288ff9bc2ad6cf4fdba4864ac2ec6c7896c275)。目标是下一版本，不在本计划指定版本号或执行发布。
**工作区：** 独立 JJ workspace `bubble-vertical-balance-plan-20261110`，路径 `.amp/in/feature-workspaces/bubble-vertical-balance`；不修改源主工作区或另两个 feature 的工作区。

## 1. 初始方案

### 背景、目标与边界

用户澄清原文：“文字目前不在气泡中间，文字下方有空白区。”目标是让识别文字块的上下留白均衡，继续左对齐；不改为水平居中，也不把整个气泡移到屏幕中央。

[57 号记录的 [006]](57-floating-confirmation-preview.md#006--2026-10-15--短句收窄与输入区垂直居中) 已要求“输入区垂直居中，文字保持左对齐”。本 feature 作为下一版本的独立修正记录，保留该历史与原生控件方案；不能把旧的短句光标居中检查视为本次问题已经解决。

本次仍缺用户截图、出问题的平台、具体文字和出现步骤。当前没有已证实根因，也没有能够确定对应用户报告的失败用例。拟采用“先记录实际文字块与容器几何，再只修正失败的平台布局”的最小方案；如果相关场景均正确，补充证据并停下来请用户决定，不做推测性修补。

保留现有编辑、选区、撤销、滚动、Enter 释放后确认、Esc 取消、Shift+Enter 换行及输入法组词保护；保留流式只读到最终可编辑的切换、尾部跟随与用户回看位置、目标恢复和投递语义。不修改文本内容或修剪换行，不新增设置、依赖、布局框架、控件或协议，不改变字体、横向收窄、颜色、描边、屏幕锚点和定位策略。

### 已核读的布局与本次原生证据

| 入口 | 源码确认的行为 | 不能由此推出的结论 |
|---|---|---|
| macOS `PreviewWindow::resize_to_text(&self)` | 设置目标滚动容器宽度后请求原生布局；取 `usedRect` 的纵向终点，将编辑区高度限制为 28–196 pt，设置文本容器垂直 inset；普通气泡外层上下各 3 pt；隐藏 status 不分配 footer | 公式存在不等于所有字形、换行和更新顺序的实际文字块均衡；实际测量宽度是否等于最终显示宽度仍需核对，不能认定是 footer 或旧 frame 引起 |
| Windows `WindowState::layout(&self) -> Result<()>` | 按当前窗口 DPI、控件字体和原生换行行数定高；编辑区为 28–196 DIP；通过 `EM_SETRECTNP` 居中格式矩形；上下外边距各 3 DIP；只有可见 status 才分配消息区 | `行数 × tmHeight`、实际行位置与字体回退是否相符，仍需原生证据；不能把某种度量偏差当作根因 |
| 两端 `preview/smoke.rs` | 原生编辑检查包含增长、删除、尾随换行和高度上限；垂直居中主要检查短句或空输入的光标 | 一个光标矩形不能证明完整多行文字块及流式更新后的上下留白正确 |

来源：[macOS 测量和布局](../../src/platform/macos/preview.rs#L356-L447)、[Windows 字体测量](../../src/platform/windows/preview.rs#L447-L490)、[Windows 布局](../../src/platform/windows/preview.rs#L530-L714)、[macOS 旧检查](../../src/platform/macos/preview/smoke.rs#L453-L530)、[Windows 旧检查](../../src/platform/windows/preview/smoke.rs#L1054-L1155)。

2026-11-10 在本工作区的 macOS 上，未修改产品代码，实际执行：

```sh
cargo build --locked --features integration --bin viberwhisper-preview --target-dir .amp/in/target-preview
./.amp/in/target-preview/debug/viberwhisper-preview --artifacts .amp/in/artifacts/bubble-vertical-balance/baseline
```

两条命令均退出 0；构建仅有 `vendor/rdev` 的既有警告。现有 `--artifacts` 模式不捕获投递目标、不注入文字、不保存个人历史，直接缓存截取自身原生 content view；未对 PNG 后处理。[模式边界](../../src/ui/preview_demo.rs#L40-L69)、[截图实现](../../src/platform/macos/preview/smoke.rs#L6-L31)。

- 已检查 `ready.png`、`streaming-prefix.png`、`streaming-draft.png`：单行文字左对齐，上下观感大致均衡，未见明显额外底部空白或 footer。
- 已检查 `ready-edited.png` 和 `streaming-retracted.png`：中英与 emoji 单行未见明显裁切；回撤为空后无残留文字。这不是对用户实际编辑或空文字居中的完整验证。
- 已检查 `ready-long.png`、`ready-warning.png`：高度受限长文下方有空区及末行光标，警告图有真实提示文字。该样本重复的每段都以 `\n` 结尾，光标位于末尾空行；因此不能据此认定复现了无意留白，也不能确定全部空区的成因。[样本](../../src/ui/preview_demo.rs#L318-L333)、[保留末尾光标行的既有契约](../../src/platform/macos/preview/smoke.rs#L471-L479)。
- 长文缓存图另有顶部半行现象，仅记录，不将它扩展为本 feature 的独立修复目标；后续布局改动不得扩大已有裁切。

上述运行没有新增文字块几何插桩；没有执行会真实投递的 `--smoke`，没有 Windows 原生运行、真实滚轮、完整编辑/按键/IME/目标投递验证，也没有低于高度上限的独立两三行、自动换行边界和无尾随换行长文对照。这些都不能写成已经通过。

原始 PNG、构建和原生日志、尺寸及观察说明保存在本工作区 `.amp/in/artifacts/bubble-vertical-balance/`，属于忽略提交的本地证据；共享入口为[本调查线程](https://ampcode.com/threads/T-01a1c439-cfb3-74ca-943e-e5d91fdf7a4c)。历史方法参考 [#169 的调查线程](https://ampcode.com/threads/T-01a1bfcd-54c3-738e-81be-d67110954ef3)，本次结论以当前源码与本次实际运行为准。本地 Claude Code Opus 5.5 已依据明确提供的材料生成计划草案；已核对初始化和回复模型均为 `claude-opus-5-5`，工具和 MCP 列表为空。咨询建议不是原生验证结果，原始事件流不入库。

### 拟议行为与具体布局验收

**先界定什么算文字块。** 使用原生排版首行到末行的行框，包含行距和用户保留的空行；末尾换行产生的光标行也计入。正常字形的 ascent / descent 可导致墨迹边距略不相等，不追求每个字符的像素墨迹完全对称，不逐字形增加动态光学偏移。几何验收之外仍需检查真实截图，行框通过但有明显额外空区仍不算完成。

**两个层级一起验收，不能只把文字居中到一个偏心的编辑框中：**

1. 无可见提示时，检查原生正文框相对整个气泡的上下外边距相等，底部不存在隐藏 footer、旧尺寸或其他空占位；继续沿用现有 3 pt / DIP 外边距。有实际警告时，记录正文、警告和真实间隙的各自矩形，仅将可见消息区从正文验收范围扣除，不删除提示来制造均衡。
2. 将首末行框、文本容器原点、正文可见框转换到同一原生坐标系，再比较文字块上下间距。非溢出内容的上下间距差：macOS 不超过 1 pt；Windows 不超过 1 个物理像素，覆盖整数平分的余数。记录实际 backing scale / DPI；不把 DIP 再与物理像素混算。
3. macOS 从 layout manager 的实际行片段与末尾 extra line fragment 取边界，再通过 `textContainerOrigin` 和视图坐标转换定位；不能直接把 `used_height` 的计算结果当期望值。Windows 从实际 EDIT 的行起点/字符位置及原生光标或文字范围验证首末行位置，连同 `EM_GETRECT`、客户区、实际字体/DPI 记录。若末尾空行或只读状态不能独立测得，补充原生查询或专用对照后再判定，不能退回生产公式自证正确。
4. 内容超过现有 196 pt / DIP 编辑区上限时，保留原生滚动及尾部跟随/回看语义，不把整篇超长文档强行居中；不能增加无意义的居中 inset，开头与末尾都应可滚达。空结果、真实空行和纯错误提示各按已有状态契约检查，不把它们本身的空白当作缺陷。

**批准后需要运行的代表性矩阵：**

| 状态与输入 | 要证明的结果 |
|---|---|
| 单行 `短句`、`English gjpq`、`中文 English 🎙️` | 左对齐；首尾行框与上下间距符合容差；中英、下伸部和 emoji 不被裁切 |
| 同一字体/宽度下刚好不换行及再增加字符后自动换行，未达高度上限 | 最终显示宽度与测量宽度一致；一行到两行后仍均衡，不按字符数估高度 |
| `第一行\nEnglish gjpq`、`第一行\nEnglish gjpq\n末行 👩‍💻` | 无尾随换行的两、三行整体均衡；不只检查最后光标行 |
| 上述样本分别加末尾换行、内部空行；删除回单行和空输入 | 保留真实空行及换行文字；删除后框与文档收缩，不能残留旧高度或额外滚动空区 |
| 原生字体测得刚好可容纳与多一行溢出的边界，再回缩 | 仅溢出时保留滚动；回缩后恢复非溢出的均衡，不留下失效首个可见行 |
| 流式一行→自动换行/多行→长文→短句→空→恢复→最终可编辑 | 每次更新及 ready 切换后均符合相应布局契约；前缀选区、只读状态、尾部跟随和回看位置不被重置 |
| 短句/多行加真实警告，再移除警告；空错误恢复到正文 | 消息清晰可见且不重叠；隐藏后无空 footer 占位；空错误不重新占一个空正文框 |

同一组输入分别通过初次显示、现有流式更新入口和真实用户编辑通知路径运行。Windows 覆盖现有 96 / 144 / 192 DPI 路径及实际窗口 DPI，记录模拟 DPI 与真实显示器验收的区别；macOS 记录实际缩放。对有关状态保存并检查未后处理的截图及对应几何日志，不把样本中自带的换行、缓存裁切或警告区域混写为报告根因。

### 技术设计与影响模块

不新增公共方法、核心类型或状态字段，不另建布局层；现有签名和流式协议保持不变：

- `src/platform/macos/preview.rs`：修复责任位于 `PreviewWindow::resize_to_text(&self)`。若失败证据指向最终宽度、行框范围、原点、inset 或文档框不一致，在此统一测量与布局；不在初次显示、编辑和流式路径各叠一份补偿。
- `src/platform/windows/preview.rs`：修复责任位于 `WindowState::layout(&self) -> Result<()>`；`measure_text(&self, control: HWND, width: Option<i32>) -> Result<(RECT, i32)>` 仅在证明确有度量偏差时调整。保留最终格式矩形、DPI 重入处理、原生 EDIT HWND、现有滚动条所有权和首个可见行恢复。
- `src/platform/{macos,windows}/preview/smoke.rs`：在现有 `smoke_confirmation` / `smoke_streaming` 与一次性原生检查中补齐完整文字块和容器几何验收。扩展可观察行为回归，不测试私有计算步骤或调用顺序；长期只保留能够防止真实复发的代表性断言，其余矩阵作为验证材料。
- `src/ui/preview_demo.rs`：在现有 render-only / artifacts 路径补无尾随换行、未溢出两三行和流式回缩对照；保留原有长文/错误样本。重型矩阵沿用一次性检查，不在每个进度回调中重复整套验证或放宽原有 10 秒 deadline。

初次显示、编辑和流式更新已共用布局入口：[macOS 编辑通知](../../src/platform/macos/preview.rs#L244-L247)、[尾部更新](../../src/platform/macos/preview.rs#L467-L502)、[ready 更新](../../src/platform/macos/preview.rs#L744-L768)、[Windows 编辑通知](../../src/platform/windows/preview.rs#L907-L914)、[更新与滚动恢复](../../src/platform/windows/preview.rs#L1218-L1299)。因此优先修正源布局，不增加一次性包装器或新的流式 renderer。共享 `ui/preview`、识别模型、会话、历史、目标捕获和位置计算都不属于本次改动面。

现有 native smoke 的签名不变：`smoke_confirmation(&self, edited_text: &str, verify_pending: &dyn Fn() -> Result<()>) -> Result<bool>`、`smoke_streaming(&mut self) -> Result<()>`；必要的局部测量代码留在各自平台 fixture 内。实际修复只能在对应失败样本确定后选择，不预先承诺移除 footer、替换字体度量或强制重置编辑器尺寸。

### 替代方案与取舍

- **只改固定偏移或统一减少下边距：** 代码更少，但可能只改善一种字体或 DPI，破坏本次已经正常的短句；不选。也不通过增加上方空白掩盖本应缩小的旧容器或隐藏占位。
- **修剪模型/用户文字末尾换行：** 会改变编辑与投递内容，并隐藏真实光标行；不选。
- **取消所有 footer 或重建控件/跨平台布局框架：** 前者会丢真实警告，后者扩大 IME、选区、撤销及滚动风险；没有证据支持。
- **在原生布局处局部修正：** 首选有条件方案。只改证实出错的平台与必要测量/布局顺序，另一平台保留对照；避免未经复现的对称改写。
- **不做产品修改：** 若补充的相关用例仍全部正确，报告覆盖范围和缺少的原始信息，等待用户决定；不能据本机样本反推用户只是在看尾随换行。

### 成本、风险与应对

主要风险是把墨迹边界与行框混用、漏掉末尾空行、混淆 AppKit 坐标方向或 Windows DIP/像素，以及布局时重置选区/撤销/回看位置。独立读取原生几何、同时检查外层占位和文字块、保留非零滚动与非空前缀选区的对照，避免仅验证空文档或末尾光标。

改变字体度量或垂直 inset 可能引起流式跳动、溢出阈值变化和 IME 候选位置变化。以同一最终宽度下的增长/回缩、阈值两侧、只读→可编辑和真实按键场景约束；不为每个样本增加字形特例。若几何正确但视觉仍有明显空区，需要继续核对实际字形/行框，不能只凭容差宣称完成。

Windows 当前没有本线程的原生实机证据。交叉编译、原生 CI、模拟 DPI 和用户显示器上的视觉结果分别记录，不互相替代；两端修复可能不对称。当前长文顶部缓存半行不另开修复范围，但相关改动不得造成新的首末行裁切。

### 批准后的实施顺序

1. 收集用户平台、原文和触发路径；若尚拿不到，补齐上述矩阵、原生几何和无尾随换行对照。在未改产品的基线上建立可重复的失败，记录字体/DPI、warning 可见性、正文/文档/格式框、首末行和滚动位置。
2. 在现有 fixture 中保留能覆盖该失败的行为断言，确认它在未修复产品上失败。若所有相关场景通过或与用户原始问题无法对应，先报告限制并停下，不选择某个候选原因直接改代码。
3. 只在确认有偏差的平台源布局中做最小必要修正；保持现有原生控件和接口。按同一输入、状态、窗口宽度和 DPI 重跑；正常短句和有效末尾空行是负对照。
4. 执行几何、截图、滚动和编辑/确认回归，实际检查截图；在专用可丢弃的编辑器目标运行真实 `--smoke`，核对确认文字与投递结果，验证 Enter 释放只确认一次、Esc 不投递、Shift+Enter 换行和组词期间不误确认。
5. 做下述简化/消融后更新本记录时间线，在同一个 bookmark / draft PR 进入仓库代码审阅门禁。真实 Windows 验收未完成就保留限制，不修改为已通过。

不需要 schema 或数据迁移，不清理个人历史/记忆。回退时只撤回本 feature 的布局改动和对应文档，不修改已确认文字、输入目标或其他 feature；版本、合并和发布另按授权及发布流程处理。

### 验证与消融

本轮实际验证仅为前述 macOS 未修改基线的构建、现有 `--artifacts` 执行和截图检查；下列为批准后的验证命令与要求，不是本轮已通过记录：

```sh
cargo fmt --check
cargo test --locked
cargo test --locked --features integration
cargo clippy --locked --all-targets --features integration -- -D warnings
cargo build --locked --features integration --bin viberwhisper-preview
cargo run --locked --features integration --bin viberwhisper-preview -- --artifacts .amp/in/artifacts/bubble-vertical-balance/after
```

Windows 沿用现有 `windows-app,integration` 原生测试、一次性 DPI/setup 检查和 preview captures；无需为本方案增加 CI 框架或手动触发发布。[现有 CI](../../.github/workflows/ci.yml#L98-L139)。还需在可用 Windows 桌面运行适用的几何、滚动和按键检查并查看截图；模拟 DPI 不等于真实跨屏和字体回退验收。

预期值来自独立原生首末行和容器查询，不能直接从生产 `used_height` 或 `content_height` 生成。只看光标、只断言窗口高度或只看截图都不充分。失败证据应能区分错误度量、外部占位、用户空行与可滚动溢出，避免以全部相同长度/高度的简单样本制造通过。

**消融/简化：** 这是局部几何修正，不引入多机制或可配置算法，无需新建消融框架。在同一原生环境和同一组样本比较“原布局 + 新断言”“修复布局 + 同一断言”“仅撤去该修正 + 同一断言”；前后都保留正常短句、真实末尾空行及 warning 对照。已定位的失败应只在修复版本消失。若增加的补偿/分支去掉后仍通过，删除它并重跑相关检查。于实现后、自动代码审阅前完成，不能用放宽容差、删除样本或改变文本替代修复。

### 待定事项与延后范围

- **计划批准阻塞：** 无需先决定 footer 或字体是不是根因；本计划只承诺带复现门禁的局部修正路径，实施仍须显式批准。真实失败尚未建立，不能批准一个指定猜测为已证实修复，也不能承诺已解决用户原始问题。
- **实现前待补：** 受影响平台/系统版本、实际 DPI/缩放、完整原文（尤其末尾空行）、截图、首次显示/流式/ready/手工编辑的哪一步出现，以及 warning 是否可见。它们不阻止讨论本计划，但最终修复归因与验收必须有对应证据。
- **实现中待定：** 哪个平台需要产品改动、准确偏差在哪个坐标/测量边界、只读和末尾空行的独立原生几何取得方式；以失败日志决定，不回退为公式自证或硬编码偏移。
- **延后：** 独立屏幕定位改进、识别/分段策略、全面 UI 重设计和历史缓存顶部半行问题。若实际报告落在这些范围，先带证据回报再决定是否另立事项，不借本计划顺带修改。

## 2. PR 变更时间线

### [001] · 2026-11-10 · 补充 Windows 截图，限定本轮只做静态调查

**新增材料与要求。** 用户提供截图并要求“检查windows下布局，先不跑实机”。本条更新初始方案中“尚无截图、平台待确认”的调查状态：本轮主查 Windows，已收到可见原句的截图；仍不知道运行版本、Windows 版本、实际 DPI、底层完整文字及操作步骤。原始计划和此前 macOS render-only 记录保留为历史；本轮没有启动 Windows 或其他原生预览，没有改产品或测试。

**截图证据。** 已下载并直接检查原始 528 × 80 PNG：可见一行“用Codex连接Redis，再让Codex检查。”，保持左对齐，没有可见警告或 footer。中央描边位于 y=12、70，内部背景为 y=13–69；在多个亮度阈值下，可见文字墨迹均落在 y=24–47，因此上方约 11 像素、下方约 22 像素。它确认了这张图中文字偏上的外观，不能给出原生行框、实际 DPI、逻辑行数或尾随换行，也不能据此指定“下移 5.5 像素”的修复。原图与 `windows-image-analysis.txt` 留在本工作区 `.amp/in/artifacts/bubble-vertical-balance/`，共享来源为[用户截图](https://ampcode.com/user-content/attachments/650547800801415bcbf21573686a0f1cc43c69b2d2c7d5d5b273fdbdd1f1adc4-image.png)与[本调查线程](https://ampcode.com/threads/T-01a1c439-cfb3-74ca-943e-e5d91fdf7a4c)。

**已证实的静态事实。**

- 普通气泡外层上下各 3 DIP；隐藏提示的高度为 0，不分配提示区及其间隙。截图没有可见提示，源码也不能支持“普通结果一直预留 footer”的判断。[高度分配](../../src/platform/windows/preview.rs#L602-L622)
- 正文按控件实际字体的 `tmHeight` 和原生 `EM_GETLINECOUNT` 定高，下限为 28 DIP、上限为 196 DIP。最终设置 EDIT 尺寸后，先以 NULL 重置格式矩形，再读取 `EM_GETRECT`，用其高度计算对称 inset。代码因此将文字居中到读回的格式矩形，未核对该矩形的中心是否与整个 EDIT 客户区相同。[文字测量](../../src/platform/windows/preview.rs#L447-L490)、[行数与尺寸](../../src/platform/windows/preview.rs#L579-L606)、[居中计算](../../src/platform/windows/preview.rs#L644-L671)
- 微软文档明确说明格式矩形独立于 EDIT 窗口尺寸，读回值也可能与设置值相差几像素；不能把 `EM_GETRECT` 默认等同于完整客户区。[EM_GETRECT](https://learn.microsoft.com/en-us/windows/win32/controls/em-getrect) 另据 [EM_SETRECTNP](https://learn.microsoft.com/en-us/windows/win32/controls/em-setrectnp) 与 [EM_SETRECT](https://learn.microsoft.com/en-us/windows/win32/controls/em-setrect)，NULL 用于恢复默认值，但触摸设备或发送线程有 hook 时存在不生效的文档限制；未证实用户机器满足这些条件。
- 现有短句居中断言读取插入符，没有检查整个文字块；DPI 用例检查高度、单行、位置和滚动条，没有检查各 DPI 下的文字块留白。本轮未运行这些 Windows 检查，不能把它们列为已通过。[短句断言](../../src/platform/windows/preview/smoke.rs#L1103-L1123)、[DPI 断言](../../src/platform/windows/preview/smoke.rs#L403-L428)

**优先假设及反证条件，均非已证实根因。**

1. 首先核对默认格式矩形相对完整 EDIT 客户区的位置。若格式矩形顶部贴着客户区，而其高度被限制为整行，现有 inset 只处理格式矩形内部余量，会漏掉下方客户区余量。纯算术例：客户区 y=0–28、格式矩形 y=0–20、文字块高 20 时，现有 inset 为 0，上下余量为 0 / 8；这是条件演示，不是用户实测值。反之，若同样高 20 的格式矩形已位于 y=4–24，它已居中；仅比较两个高度不足以判错。若原生文字块与客户区的中心已经相同，应排除这一解释。
2. [Wine 1.3.19 的 `EDIT_AdjustFormatRect`](https://goma.googlesource.com/wine/+/wine-1.3.19/dlls/user32/edit.c) 将多行格式矩形底部设置为顶部加整行高度，再受客户区底部限制。这只说明兼容实现存在这样的规范化，不能当成微软 Windows 或用户截图状态的证明；未据此认定 Windows 一定向下取整。
3. 底层文字自带末尾 CRLF、墨迹与行框不一致、回退字体度量和 NULL 未重置都仍是待区分的解释。截图不能排除隐藏空行，也不能确认字体问题；不为这些候选各加一个补偿分支，不删除有效空行。

**有条件的最小方案，待批准后验证。** 优先在 Windows 的同一 `layout` 入口核对最终客户区、默认/设置后的格式矩形及实际首末行位置。若失败由格式矩形中心偏上造成，且文字可以完整容纳，以最终 EDIT 客户区计算正文的垂直起点，提交显式 RECT；不再仅在默认格式矩形内平分余量。不增加新控件、字体替换、固定像素下移或状态字段。水平 margin、最终换行宽度和左右对齐沿用既有契约，避免把已含 margin 的坐标重复施加；具体 RECT 的水平坐标以原生读回和换行对照确定。若默认格式矩形本就居中、问题出在真实空行或行框与墨迹差异，此方向不足以修复，继续取证而不交付无效改动。

**风险与验收补充。** 最终格式矩形高度还决定滚动可见行数，不能为视觉均衡牺牲完整末行或制造错误滚动容量；溢出时保留现状，不将超长文档居中。初次显示、手工编辑及流式更新已经共用布局；`update_text` 保留的前缀选区、只读切换、尾部跟随/回看不应重置。[滚动容量与恢复](../../src/platform/windows/preview.rs#L392-L414)、[手工编辑通知](../../src/platform/windows/preview.rs#L907-L914)、[流式更新](../../src/platform/windows/preview.rs#L1218-L1299)

批准后先把截图可见原句分别作为无尾随换行、带有效末尾换行的对照，再覆盖初稿的一行→自动换行→显式多行→溢出→回缩及 streaming→ready 序列。记录字体、实际 DPI、客户区、默认和最终格式矩形、首末行、逻辑行数、末尾换行和滚动位置；不将整段识别内容写入常规日志。保留原先不超过 1 个物理像素的非溢出行框间距容差，独立核对实际行框与截图，不把 `content_height` 或 `EM_GETRECT` 高度本身当唯一期望值。优先补充 125% / 175% 分数缩放对照并区分模拟 DPI 与用户显示器；不得从此截图反推它是哪种缩放。编辑、滚动、Enter 释放确认、Esc、Shift+Enter、IME 仍须在获准的原生验证中检查。

**状态与未验证部分。** 本条为调查结论和计划修订，尚未实现。Windows 实际格式矩形、真实行数/字体/DPI、用户版本和触发路径均未知；不标注 Windows 原生测试通过，不将既有 macOS 正常短句外推到 Windows。本轮按要求不跑实机；日后计划获批也不自动解除这项限制，开始原生运行前须先获用户同意。实现、产品修复结论、提交、推送及 PR 都不属于本轮交付。

### [002] · 2026-11-10 · 核验同一基线的 Windows 原生截图，确认短句偏上现象

**要求与证据来源。** 用户随后要求“看来要实际跑一下windows效果检查”，更新 [001] 中暂不运行 Windows 的限制，未批准产品或测试实现。本机仅有 macOS runner，未发现可用 Windows runner 或本地 Windows 虚拟机入口。本次没有新启动 Windows 作业，而是只读取得并检查同一 v0.5.3 基线的[既有 Windows CI 作业](https://github.com/b1indsight/viberwhisper/actions/runs/42720532347/job/129513039936)：提交与本工作区基线完全一致，2026-11-09 23:27–23:39 UTC 执行成功，系统为 Windows Server 2025、10.0.26100。下载其 `windows-preview-captures` 原始附件和完整作业日志，没有重跑 CI、提交或修改外部状态。不能将本次查阅称为本线程新跑了 Windows，也不能将 Server CI 等同于用户桌面环境。

**原生画面及日志。** 已直接查看最终、流式、中英/emoji 编辑、长文、真实警告、九行回缩及合成 DPI 短句截图。普通 `ready.png` 和 `streaming-draft.png` 均为 176 × 34，日志明确记录 actual/layout DPI=96、`lines=1`、`warning=false`、首个可见行 0、无垂直滚动条；对应样本文字没有尾随换行。[最终样本](../../src/ui/preview_demo.rs#L24-L25)、[流式样本](../../src/ui/preview_demo.rs#L80-L85)、[原生截图与状态日志实现](../../src/platform/windows/preview/smoke.rs#L7-L125)

使用标准库解码原始 PNG，在中央 60% 宽度排除圆角外部背景及滚动条，以四档中性亮色阈值测量墨迹，结果如下。上下间距均相对中央描边的内部背景；它们只证明可见外观，不能代替首末行框或直接作为修复偏移：

| 原始截图 | 尺寸 / 状态 | 上方 / 下方可见空隙 |
|---|---|---|
| `ready.png`、`streaming-draft.png` | 176 × 34，真实窗口 DPI 96，单行无提示 | 5 / 13 物理像素；四档阈值一致，文字明显偏上 |
| `ready-edited.png` | 313 × 34，DPI 96，中英与 emoji，单行无提示 | 6 / 11 物理像素；仍偏上，字形改变了墨迹边界 |
| `synthetic-144-short-step-1.png` | 185 × 52，layout DPI 144、actual DPI 96 | 9 / 17 物理像素；只证明合成布局路径 |
| `synthetic-192-short-step-2.png` | 242 × 68，layout DPI 192、actual DPI 96 | 11 / 24 物理像素；不是实际 200% 显示器验收 |
| `scroll-shrink-fit-ready-true.png` | 176 × 186，DPI 96，9 行，无尾随换行/提示/滚动条 | 上方 5、下方 3–4 物理像素；未见同样明显的额外底部空区 |

九行样本从滚动长文经 `update_text` 回缩而来；其输入由行数组 `join("\n")` 生成，没有尾随换行，是有意义的多行负对照。[回缩夹具](../../src/platform/windows/preview/smoke.rs#L511-L553) 长文截图则达到高度上限，包含有效尾随换行和滚动状态，不能拿它的底部空区与短句统一归因，也不能据本次截图断言自动换行边界、独立两三行或编辑/按键/IME 已完成验收。

**桌面合成对照。** 常规 PNG 使用自身 HWND 的 `WM_PRINT`；另已直接检查同一作业的 `geometry-screen-1-1.png`（190 × 38），它由 `DwmFlush` 后 `CopyFromScreen` 保存，是 Windows 桌面实际呈现的原始局部画面。该流式单行无提示的墨迹上下空隙为 6 / 13 物理像素，对应离屏图 `geometry-expected-1-1.png` 也为 6 / 13。这支持偏上外观同样存在于桌面显示，并非只能在 `WM_PRINT` 图中观察到。[屏幕截取](../../src/platform/windows/preview/geometry_probe.ps1#L22-L45)、[输入序列及捕获区域](../../src/platform/windows/preview/geometry_probe.rs#L118-L175) 既有 geometry 自动比对排除了 EDIT/caret/提示控件，只验证外框重绘；其通过不能视为正文垂直居中通过，本条文字观察和像素检查是另行完成的。

**更新后的判断与最小修复路径。** 已在同一基线 Windows 原生输出中确认“单行正文偏上、下方更多空白”的同类外观，并且单行日志排除了这些样本中的隐藏提示、有效尾随空行及滚动条；不再仅凭静态公式讨论是否存在现象。普通结果没有额外 STATIC 占位；外层上下各 3 DIP 的事实不变，不能将 STATIC 的 y 坐标或 192 DPI 下 6 个物理像素误解为正文底部固定 6 DIP。现有截图和日志仍未给出默认/最终 `EM_GETRECT`、EDIT 客户区及实际行框，故 [001] 的“格式框中心偏上”仍是假设。保留其有条件方案：先在可用 Windows 原生环境补独立几何，再只修正 Windows 共用布局入口中被证实的坐标/度量问题；不为所有状态统一加固定下移量。九行回缩、macOS 现有正常短句及真实警告作为不能破坏的对照。

**待补与交付边界。** 本地证据位于 `.amp/in/artifacts/bubble-vertical-balance/windows-native-v0.5.3/`、`windows-native-v0.5.3-ci.log` 和 `windows-native-image-analysis.txt`，共享入口仍为[本调查线程](https://ampcode.com/threads/T-01a1c439-cfb3-74ca-943e-e5d91fdf7a4c)。新增取证需要可用 Windows 运行环境；现有原生检查成功不填补几何、用户原句、用户 Windows/字体/实际缩放及触发步骤的缺口。下一步在同一原生样本记录客户区、默认/最终格式框及行位置，再完成初稿中的自动换行、显式两三行、流式更新、滚动和按键矩阵；仍不调整 1 个物理像素的行框容差。macOS 本线程只有此前 render-only 检查，未发现对应单行问题，也未完成完整编辑/投递实机验收。本轮仅修订计划，无产品/测试实现、提交、推送或 PR。

### [003] · 2026-11-10 · 改为直接记录原生组件高度，准备只读诊断

**要求与处理。** 用户要求直接测试各组件的高度占用。停止继续以截图推测根因；在现有 Windows `smoke_capture` 中临时加入原生尺寸输出，只读取每次截图时的气泡、EDIT、STATIC，不重新实现布局或调整任何尺寸。[只读诊断](../../src/platform/windows/preview/smoke.rs#L25-L67) 通过 `GetWindowRect`、`GetClientRect`、`ClientToScreen` 输出外框、客户区、原点和高度；另读取最终 `EM_GETRECT`、当前控件字体的原生行高及 `EM_GETLINECOUNT`。这能直接区分外层占位、EDIT 客户区和文字格式区分别占多少高度，正常、流式、回缩及合成 DPI 仍沿用现有原生样本。没有添加居中偏移、重置格式框或文字修改。

**验证与执行限制。** 诊断代码已通过 `rustfmt --check`。尝试 `cargo check --locked --target x86_64-pc-windows-gnu --features integration --bin viberwhisper-preview`，在基线依赖 `ort-sys 2.0.0-rc.9` 构建脚本处失败：没有该 GNU target 的预编译 ONNX Runtime；不能将其写成 Windows 编译或运行通过，也不改依赖来绕过。本机重新确认只有 macOS runner，且无 Windows VM/Wine 入口。既有 CI 只响应 `master` push 或目标为 `master` 的 PR，没有手动 dispatch；新诊断需推送本 feature 分支并开 draft PR 才能在 Windows 执行，这两项外部写入待用户授权。[CI 触发条件](../../.github/workflows/ci.yml#L1-L7)

**当前状态。** 新增的是临时取证日志，未实施产品布局修复，未得到新一轮 Windows 组件实测值。此前同一基线的 34px 普通单行窗口与可见警告的 STATIC 读回日志保留，不能把源码算出的 EDIT 高度或像素墨迹间距标为新测量。计划和诊断都仅保存在本独立 workspace，尚无提交、推送或 PR；不直接修改 `master`，也不把用户此次测试要求扩展为产品修复批准。取证结束后清理临时日志；只保留有依据的布局修正及必要行为回归。

### [004] · 2026-11-10 · Windows 原生高度测量定位到 EDIT 内部的底部余量

**执行与来源。** 用户授权推送只读诊断并创建 draft PR，新增诊断经两个独立审阅通过，无 findings。诊断提交为 [27a52a1f](https://github.com/b1indsight/viberwhisper/commit/27a52a1f341864f47584eaa8e6ea68f42aea4ea1)，对应 [Windows CI 原生作业](https://github.com/b1indsight/viberwhisper/actions/runs/42795731316/job/129760308159) 于 2026-11-10 10:02–10:09 UTC 实际执行成功，同轮 macOS CI 也成功。系统为 Windows Server 2025 / 10.0.26100。已下载本轮原始日志与截图，重新直接检查普通单行、流式和九行回缩画面；本条是新增原生 API 读回结果，不是旧截图估算或源码反推。

**实测组件占用。** 下表统一使用物理像素，格式矩形 y 坐标相对 EDIT 客户区。字体行高来自实际字体的 `GetTextMetricsW`，行数来自原生 EDIT；矩形由 `GetWindowRect`、`GetClientRect`、`EM_GETRECT` 直接读取。

| 原生状态 | 气泡客户区高 | EDIT 客户区高 | 格式矩形 y / 高 | 字体行高 / 行数 | STATIC 高 / 可见 |
|---|---:|---:|---|---|---|
| 最终单行、流式单行、中英/emoji 编辑单行；actual/layout DPI 96/96 | 34 | 28 | 0–20 / 20 | 20 / 1 | 0 / 否 |
| 合成 layout DPI 144，actual DPI 96，单行 | 52 | 42 | 0–31 / 31 | 31 / 1 | 0 / 否 |
| 合成 layout DPI 192，actual DPI 96，单行 | 68 | 56 | 0–41 / 41 | 41 / 1 | 0 / 否 |
| 九行回缩，actual/layout DPI 96/96，无尾随换行/滚动条 | 186 | 180 | 0–180 / 180 | 20 / 9 | 0 / 否 |
| 达到高度上限的长文，actual/layout DPI 96/96，滚动状态 | 202 | 196 | 0–180 / 180 | 20 / 37 | 0 / 否 |
| 同一长文加真实警告，actual/layout DPI 96/96 | 237 | 196 | 0–180 / 180 | 20 / 37 | 32 / 是 |

普通 96 DPI 单行的气泡内部，EDIT 的顶部为 y=3、底部为 y=31，外层上下各 3；EDIT 客户区与其窗口同为 28 高，没有额外非客户区高度。格式矩形为 `[4, 0, 152, 20]`，正好容纳一个 20 高的字体行，却位于 EDIT 顶部，因此 EDIT 内部上方余量为 0、下方为 8；相对整个气泡的格式区上方为 3、下方为 11。STATIC 不可见且实际高度为 0，虽然其零高窗口原点位于气泡底部，不能把该坐标当作额外 footer。警告状态则真实分配 32 高提示，正文到提示及提示到底部各 3，整体为 3 + 196 + 3 + 32 + 3 = 237。

**已确认问题与边界。** 在本轮 Windows 环境中，短句最终格式框贴在较高的 EDIT 顶部、剩余空间全部留在其下方，是已证实的布局缺陷；更新 [001] / [002] 中“格式框是否偏上仍是假设”的状态。它与截图中文字偏上的外观一致，流式和 ready 状态均存在。此次读的是布局完成后的格式框，未记录 NULL 重置后、设置 inset 前的中间值；不据此宣称已经验证微软内部规范化的具体实现。九行回缩没有这段内部余量；长文的可视格式区与整行容纳、溢出和尾随空行有关，仍按滚动视口验收，不能统一移动全部内容。

**最小修复方向，尚待批准和前后验证。** 将 Windows 同一 `layout` 中非溢出文字的垂直起点改为基于最终 EDIT 客户区与正文高度平分剩余空间，不继续只在读回的较小格式框内部计算 inset。[现有决策点](../../src/platform/windows/preview.rs#L660-L671) 本次 96 DPI 短句的目标是把 20 高的格式区置于 EDIT 的 y=4–24，上下各 4；保留外层 3 后，相对气泡各 7。合成 144 / 192 的剩余空间分别是 11 / 15，应按实际像素平分为 5/6、7/8，不能固定增加 4px 或 6 DIP。水平 margin、左对齐、字体、控件、3 DIP 外边距、真实警告与超过上限的滚动策略都保持不变；macOS 不做对称改写。对设置后读回的实际格式框和首末行重新验收，不能只断言请求了某个 RECT。

**验收与交付边界。** 普通/流式单行、中英/emoji 与九行回缩已有本轮实测；低于高度上限的独立自动换行边界、显式两三行、用户原句及真实 125% / 175% 显示器缩放仍需按初稿矩阵补齐。当前没有运行新修复，因此不能将 CI 成功称为“居中修复通过”，也不能声称完整 IME、确认/投递或用户 Windows 桌面已验收。临时诊断仍在 draft PR 供复查，未修改布局，未合并；后续修复验证后移除临时输出。原始数据保存在本工作区 `.amp/in/artifacts/bubble-vertical-balance/windows-component-ci.log`、`component-height-table.txt`、`windows-components-27a52a1/`，共享入口为[本调查线程](https://ampcode.com/threads/T-01a1c439-cfb3-74ca-943e-e5d91fdf7a4c)。
