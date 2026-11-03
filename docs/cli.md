# 命令行参考

[返回使用指南](../README.md) · [配置指南](configuration.md) · [使用与故障排查](usage.md)

## 运行命令行工具

以下示例使用 `viberwhisper`，请按实际安装位置调用：

- macOS 安装到「应用程序」后，完整路径为 `/Applications/ViberWhisper.app/Contents/MacOS/viberwhisper`。
- Windows 使用 `viberwhisper.exe`；便携版可在解压目录打开 PowerShell，用 `./viberwhisper.exe` 替换下文的 `viberwhisper`。
- Windows 双击启动桌面应用使用 `viberwhisper-app.exe`，命令行操作使用 `viberwhisper.exe`。
- 从源码运行时，用 `cargo run --` 替换 `viberwhisper`，例如 `cargo run -- config path`；构建要求见[贡献指南](../CONTRIBUTING.md)。

```bash
viberwhisper --help
viberwhisper --version
viberwhisper config --help
viberwhisper convert --help
```

## 启动监听与设置

```bash
# 无子命令时启动桌面监听
viberwhisper

# 按需重新运行配置向导
viberwhisper setup
```

监听启动时若配置缺失、不可读或无效，会进入配置向导。配置和离线转写命令不启动托盘或录音监听。
在终端启动的监听可用 Ctrl+C 退出，也可从托盘右键菜单退出。

## 配置命令

```bash
viberwhisper config path
viberwhisper config list
viberwhisper config get input.hold_hotkey
viberwhisper config set input.hold_hotkey RIGHTALT
viberwhisper config set audio.input_device null
viberwhisper config check
```

`path` 显示唯一配置文件路径。`list/get` 显示普通字段及密钥来源状态，不显示密钥值。
`set` 只接受[字段表](configuration.md)中的 canonical dotted key，保存前检查类型；跨字段和平台支持情况由 `check` 或实际启动验证。
`check` 构建并检查当前桌面配置，不录音、不发起 API 验证请求；需要实际语音验证时用 `setup`。

`schema_version` 和 API secret 字段只读，密钥可通过向导或[环境变量](configuration.md#配置文件与首次设置)配置。
`dictation.*` 仅通过配置文件编辑，不属于 `list/get/set` 的字段目录。
修改后需重新启动已运行的应用。

## 离线 WAV 转写

```bash
# 将结果输出到标准输出
viberwhisper convert input.wav

# 写入文件，也可使用 -o
viberwhisper convert input.wav --output output.txt
```

使用当前配置选择的识别路径：默认 HTTP STT，若启用 `dictation.enabled` 则使用 Realtime。
HTTP 路径按 `post_process.enabled` 决定是否整理文字；失败时回退原始 STT。Realtime 不再调用第二个后处理服务。
长 WAV 自动分段，静音分段会在请求前过滤。语言相关的分段拼接规则由 `transcription.language` 决定。

离线转写不显示确认气泡、不输入活动窗口、不写普通识别历史，也不读取、上传或更新个人语境记忆。
`--output` 写入指定文件并覆盖其原有内容。

## STT Prompt 数据集与回归

```bash
viberwhisper prompt-lab --help
viberwhisper prompt-lab record --help
viberwhisper prompt-lab evaluate --help
```

`prompt-lab` 提供录音采集、参考文本校正、数据集验证、全量重跑和编码代理语义复核。
完整命令及评分流程见[贡献指南](../CONTRIBUTING.md#stt-prompt-回归调试)，数据格式见[Prompt-lab 架构](architecture/prompt-lab.md)。
Realtime 的临时提示词覆盖与场景评估见[Realtime 回归说明](dictation.md#离线转写与-prompt-回归)。
采集仍使用 HTTP raw-STT；评估可使用所选 HTTP 或 Realtime 路径，且不接触个人记忆文件。
