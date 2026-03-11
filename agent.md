# ViberWhisper Agent 协作指南

## 项目信息
- **名称**: ViberWhisper
- **语言**: Rust
- **路径**: `~/personal_work/viberwhisper/viberwhisper/`
- **GitHub**: https://github.com/b1indsight/viberwhisper

## 提交规范 (Conventional Commits)

所有提交必须遵循 [Conventional Commits](https://www.conventionalcommits.org/) 标准：

### 格式
```
<type>(<scope>): <subject>

<body>

<footer>
```

### Type 类型

| Type | 用途 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `docs` | 文档更新 |
| `style` | 代码格式（不影响功能）|
| `refactor` | 重构（非 feat/fix）|
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建/工具/依赖更新 |

### Scope 范围

可选，用于指定修改的模块：
- `audio` - 录音模块
- `hotkey` - 热键模块
- `transcriber` - 转录模块
- `typer` - 输入模块
- `tray` - 托盘图标
- `config` - 配置系统
- `cli` - 命令行接口

### 示例

```bash
# 新功能
feat(tray): add system tray icon with recording status

# Bug 修复
fix(hotkey): resolve key repeat issue on Windows

# 文档
docs: update README with installation instructions

# 重构
refactor(audio): extract recording logic into separate module

# 依赖更新
chore(deps): bump tray-icon to 0.21
```

## 开发流程

1. **创建分支**: `git checkout -b feature/<name>`
2. **开发功能**: 小步提交，遵循提交规范
3. **解决冲突**: 如有冲突，使用 `git merge origin/master`
4. **推送代码**: `git push origin feature/<name>`
5. **创建 PR**: 在 GitHub 上创建 Pull Request

## 代码规范

- 使用 `cargo fmt` 格式化代码
- 使用 `cargo clippy` 检查代码
- 所有公共函数必须有文档注释
- 错误处理使用 `Result<T, Box<dyn std::error::Error>>`

## 常用命令

```bash
# 编译检查
cargo check

# 运行测试
cargo test

# 格式化代码
cargo fmt

# 代码检查
cargo clippy

# 构建发布版本
cargo build --release
```
