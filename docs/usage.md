# 使用详情与故障排查

[快速上手](../README.md) · [配置指南](configuration.md) · [命令行参考](cli.md)

## 系统要求

- **操作系统**：macOS 或 Windows；macOS 安装包同时支持 Apple Silicon 与 Intel，Windows 安装包为 x86_64。
- 需要可用的麦克风与转写 API。macOS 还需麦克风和辅助功能权限，Windows 需允许应用访问麦克风。
- 安装发布包不需要 Rust；如需从源码构建，请参阅[贡献指南](../CONTRIBUTING.md)

## 下载安装包

从 [GitHub Releases](https://github.com/b1indsight/viberwhisper/releases) 下载：

- macOS：`ViberWhisper-v<version>-macos-universal.dmg`，同时支持 Apple Silicon 与 Intel；
  `.tar.gz` 是保留应用权限的便携 `.app` 归档。
- Windows：`ViberWhisper-v<version>-windows-x86_64.msi`；开始菜单使用不显示命令行窗口的
  桌面入口。`.zip` 便携版本包含用于双击启动的 `viberwhisper-app.exe`、用于命令行操作的
  `viberwhisper.exe`、`LICENSE` 和 `THIRD-PARTY-NOTICES.txt`。
- `SHA256SUMS` 包含四个发行文件的 SHA-256，可用于下载后校验；GitHub Release 同时提供
  artifact provenance。

当前发行包尚未使用 Apple Developer ID/notarization 或 Windows Authenticode 签名，因此系统可能显示
Gatekeeper/SmartScreen 警告。请先核对校验和与 GitHub provenance，再通过系统提供的确认入口运行。

> **当前发行范围**：DMG、MSI 和便携归档包含 Rust 应用，并通过可配置的
> OpenAI-compatible API endpoint 提供转写和可选文本后处理。

macOS 打开 DMG 后，将 `ViberWhisper.app` 拖入「应用程序」再启动。Windows 安装 MSI 后从开始菜单启动；便携版先解压，再双击 `viberwhisper-app.exe`。

下载后，计算所选文件的 SHA-256，与同一 Release 的 `SHA256SUMS` 对照：

```bash
# macOS，将文件名替换为实际下载文件
shasum -a 256 "ViberWhisper-v<version>-macos-universal.dmg"
```

```powershell
# Windows
Get-FileHash ".\ViberWhisper-v<version>-windows-x86_64.msi" -Algorithm SHA256
```

GitHub provenance 的验证方式见[发布手册的产物验证步骤](releasing.md#5-verify-the-published-release)。

## 预览、编辑与输入

预览显示时会尝试取得键盘焦点；系统拒绝切换时请点击预览后按回车。确认键释放后才恢复
预览出现前的目标并写入文字，第一次回车不会作为换行或发送键交给目标应用。
气泡获得焦点后需要重新按下回车；之前已按住的回车不会确认。macOS 支持
**Cmd+A/C/X/V** 全选、复制、剪切和粘贴。
短句气泡按文字实际宽度收窄，达到最大宽度后换行并随内容增减高度；文字左对齐，输入区垂直居中。
达到最大高度后在框内滚动。气泡优先出现在原输入光标下方；下方空间不足时移到上方，
靠近屏幕边缘时自动避让。应用未提供光标坐标时尝试输入框附近，再回退到屏幕中央。
macOS 可拖动气泡空白区域，拖动后沿用手动位置。
待确认时再次请求录音会回到当前预览，需要先输入或取消。目标关闭、焦点无法恢复或输入
失败时保留预览，可重试或复制后手动粘贴；如果平台报告文字可能已部分输入，应先核对目标内容。

Realtime 模式会逐步显示服务返回的正文，隐藏 JSON 和记忆元数据；生成期间不可编辑或确认，
Esc 会取消整次听写（仍在录音时也会停录）。最终结果通过校验和本地纠错后，保留相同前缀，
从最早出现差异的词开始替换旧尾部，然后才允许编辑和确认。确认前不会向目标应用输入。
录音期间的预览不抢焦点，停录后才尝试聚焦。请求时机仍是每个 30 秒分段结束或停录，
因此短录音在停录后开始显示识别流；服务不发送 delta 时一次显示完整结果。

## macOS 权限与粘贴

macOS 需要辅助功能授权才能完成文字输入。未授权时输入会明确失败且不会改动剪贴板。原生应用没有焦点输入框或焦点位于可识别的密码框时也会拒绝输入。Chromium 浏览器不依赖网页辅助功能树，而是使用原生剪贴板与 Cmd+V；转写文本会保留在剪贴板中，自动粘贴未生效时可手动粘贴。浏览器隐藏网页辅助功能树时，系统无法区分普通网页输入框和密码框；此时行为等同用户手动执行 Cmd+V。

macOS 启动监听时若缺少辅助功能权限，会显示授权说明和「打开设置」按钮；启动后首次因权限失效而输入失败时也会提示。同一次启动最多显示一次，选择「稍后」仍可录音，识别结果可从待确认预览复制后手动粘贴。若更新后已有授权失效，请在「系统设置 → 隐私与安全性 → 辅助功能」中移除旧条目，重新添加当前应用，再退出并重启。离线转写和配置命令不会显示此提示。

## 识别历史与本地数据

监听模式会在确认输入时，把文本框编辑后的非空最终文本追加到配置文件同目录下的 `history.jsonl`。
历史与文字注入使用同一份确认文本，保留大小写、换行和 Unicode 内容。
确认前和取消时不写入；确认后投递失败的重试复用已有记录，不重复追加：

- macOS：`~/Library/Application Support/com.b1indsight.viberwhisper/history.jsonl`
- Windows：`%APPDATA%\ViberWhisper\history.jsonl`

每行是一个按时间先后排列的独立 JSON 记录，例如
`{"text":"识别结果","metadata":{"created_at_unix_ms":1786612345678}}`。启动和追加前只校验
最后一行；如果 JSON 或元数据无效，就删除该行，再继续使用此前记录。文件限制为 5 MiB，
接近上限时淘汰最旧的完整记录，不截断正文。保存失败不会阻止确认后的输入。该文件是本机明文，
不包含音频、API 密钥或目标应用信息。

右键菜单直接显示最新 5 条；长文本在 40 个 Unicode 字符后缩略，但点击时复制的始终是完整原文。
复制动作不会自动粘贴，也不需要 macOS 辅助功能权限。

语境记忆与确认后学习另存于同目录的 `dictation-memory.json`，用途、发送给服务的内容和清理方法见[Realtime 与语境记忆指南](dictation.md)。
`config.json` 中由向导保存的密钥、历史与记忆均是本机明文，请按需保护或备份所在目录。

## 常见问题

| 情况 | 处理方式 |
|---|---|
| 无法录音或选定的麦克风已断开 | 检查系统麦克风权限，运行 `viberwhisper setup` 重新选择设备并验证录音；指定设备不可用时不会自动切换。 |
| 热键不触发或影响当前应用 | 检查 Fn/媒体键模式，运行 `viberwhisper config check`；热键名称、别名与平台限制见[热键配置](configuration.md#单键热键名称)。 |
| 停录后暂时没有文字 | 等待转写完成；默认 HTTP 路径最终显示结果，Realtime 短录音在停录后才开始请求并显示生成流。 |
| 再次录音只回到旧预览 | 先按 Enter 确认或 Esc 取消当前结果，再开始下一次录音。 |
| 已识别但未输入目标应用 | 保留预览，检查目标窗口和 macOS 辅助功能权限后重试，或复制后手动粘贴；报告可能部分输入时先核对目标内容。 |
| 配置不生效或重新出现向导 | 用 `config path` 确认文件，用 `config check` 检查设置；配置和环境变量变化后重新启动应用，详见[配置指南](configuration.md)。 |
| Realtime 无法连接或响应格式错误 | 检查服务地址与鉴权，并核对[服务兼容性](dictation.md#长录音与服务兼容性)；应用不会自动启动服务或切换到另一服务。 |
