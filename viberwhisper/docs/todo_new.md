# ViberWhisper TODO 任务列表

## 已完成
- [x] PR #5: 双热键录音模式 (Hold/Toggle)
- [x] PR #6: Changelog 补充
- [x] PR #7: 多平台支持 (macOS + Windows)
- [x] PR #8: 系统托盘图标

## 进行中
- [ ] 悬浮窗实现

## 新增任务 (2026-03-11)

### 9. 日志系统重构
**描述**: 将现有的 `println!` 和 `eprintln!` 替换为专业的日志库
**建议**: 
- 使用 `tracing` 或 `log` + `env_logger`
- 支持不同日志级别 (DEBUG/INFO/WARN/ERROR)
- 支持日志输出到文件
- 配置文件中添加日志级别设置

### 10. 多模型支持
**描述**: 支持配置不同的语音识别模型
**建议**:
- 配置中添加 `model` 选项（已有，需扩展）
- 支持 Groq 的不同 Whisper 模型: `whisper-large-v3`, `whisper-large-v3-turbo`
- 未来可扩展支持其他服务商 (OpenAI, Azure, 本地模型等)
- 模型切换无需重启应用

### 11. LLM 转写层
**描述**: 在语音识别后添加 LLM 处理层，对识别结果进行润色/格式化
**建议**:
- 可选功能，通过配置启用
- 支持自定义 prompt 模板
- 支持不同 LLM 提供商 (OpenAI, Groq, 本地等)
- 使用场景示例:
  - 口语化转书面语
  - 自动添加标点
  - 专业术语纠正
  - 多语言混合处理
