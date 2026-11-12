# ViberWhisper

用全局热键录音，在气泡中预览和修改识别结果，按 Enter 输入到原来的文本框。支持 macOS 和 Windows。

## 安装

从 [GitHub Releases](https://github.com/b1indsight/viberwhisper/releases) 下载：

- **macOS**：选择 `macos-universal.dmg`，打开后将 `ViberWhisper.app` 拖入「应用程序」，支持 Apple Silicon 与 Intel。
- **Windows x86_64**：安装 `.msi` 后从开始菜单启动；使用便携 `.zip` 时，解压后双击 `viberwhisper-app.exe`。

安装包无需 Rust。当前 macOS 应用为 ad-hoc 签名，未做 Developer ID 签名或公证；Windows 未做 Authenticode 签名，首次运行可能需要系统确认。
下载校验、完整文件名和权限问题见[安装与故障排查](docs/usage.md)。从源码运行见[贡献指南](CONTRIBUTING.md)。

## 首次启动

1. 启动应用，在向导中选择**普通识别**或 **Realtime**。普通识别填写 STT 服务、模型和密钥（默认 Groq），可选 LLM 整理；Realtime 填写 WebSocket 地址、模型和鉴权环境变量名，无需鉴权时留空。需自行提供兼容的识别服务，详见[配置指南](docs/configuration.md)。
2. 选择录音热键和麦克风，可录制语音验证设置。默认按住 **F8** 录音，或用 **F9** 切换录音。
3. 允许应用使用麦克风。macOS 还需在「系统设置 → 隐私与安全性 → 辅助功能」中授权 ViberWhisper；从终端运行时授权对应终端。
4. 完成向导后保存配置，系统托盘或状态栏会出现声波图标。

需要重新设置时运行 `viberwhisper setup`。配置文件位置、环境变量密钥、自定义热键和服务切换见[配置指南](docs/configuration.md)。
发布包中的 CLI 入口见[命令行参考](docs/cli.md#运行命令行工具)。

## 日常使用

1. 将光标放到需要输入文字的文本框中。
2. 用热键或托盘图标录音：

   | 操作 | 开始 | 停止 |
   |---|---|---|
   | 按住录音 | 按住 F8 | 松开 F8 |
   | 切换录音 | 按一下 F9 | 再按一下 F9 |
   | 托盘录音 | 左键点击图标 | 再次左键点击 |

3. 等待识别结果显示在气泡中，直接修改需要调整的文字。
4. 按 **Enter** 确认，应用恢复原目标并输入编辑后的内容；按 **Esc** 取消，按 **Shift+Enter** 换行。中文输入法选词时，Enter 先确认候选词。

气泡待确认时需先输入或取消，才能开始下一次录音。输入失败时保留预览，可重试或复制后手动粘贴；提示可能部分输入时，请先核对目标内容。
启用 [Realtime 听写](docs/dictation.md) 后会逐步显示生成的正文，生成期间只读，最终校验完成后才能编辑和确认。
Realtime 与文字后处理同时开启时，音频只做基础转写，再由文字服务整理，避免把整理示例和个人语境送入短音频识别；启用前需清空旧一体式提示词覆盖，详见[两阶段识别](docs/dictation.md#短音频与两阶段识别)。`convert` 和 `prompt-lab evaluate` 共用这条生产路径，文件按 1 倍速回放。

右键点击托盘图标，在「输入设备」中直接选择麦克风或「系统默认」，选择会保存并从下次录音生效，不中断当前录音。插拔设备后可点击「刷新设备列表」。
右键菜单还可复制最近 5 条已确认文字或退出应用。macOS 在 Chromium 浏览器或兜底粘贴时会把转写文本留在剪贴板，便于手动粘贴。
更多预览操作、历史保存位置及权限说明见[使用详情](docs/usage.md)。

## 命令行使用

```bash
# 启动监听 / 重新配置
viberwhisper
viberwhisper setup

# 查找并检查配置
viberwhisper config path
viberwhisper config check

# 转写已有 WAV 文件
viberwhisper convert input.wav --output output.txt
```

完整参数、配置读写与离线转写说明见[命令行参考](docs/cli.md)。

## 相关文档

- [完整配置与 LLM 文字整理](docs/configuration.md)
- [Realtime、语境记忆与确认后学习](docs/dictation.md)
- [安装、预览、历史与故障排查](docs/usage.md)
- [源码构建、测试和贡献流程](CONTRIBUTING.md)
- [架构、实现记录与发布文档索引](docs/README.md)
- [MIT 许可证](LICENSE)
