# 更新记录

本文件记录各版本的主要变化。

## [Unreleased]

## [0.1.0] - 2026-11-21

首个公开版本。

### Windows

- TSF 输入法：五笔 86 优先的五笔/拼音混输、简拼、拼音容错、模糊音（可选）。
- 光标旁浮动框草稿：Enter 翻译上屏英文，Shift+Enter / Ctrl+Enter 上屏中文。
- 翻译引擎：免费 Google 翻译（默认）、Google Cloud Translation、OpenAI 兼容大模型，失败时自动改用大模型。
- 组字中途单击 Shift 夹英文；普通五笔模式（不翻译）。
- 常用输入法约定：以词定字、翻页、万能键、四码上屏、五码顶字、简繁输出、中英文标点、自学习、用户词库、输入记忆。
- Inno Setup 安装包，升级后需要重启。

### macOS（开发中，未发布）

- InputMethodKit 输入法代码与 CI 构建（通用二进制、.pkg）；待签名公证后发布。

### Linux

- 命令行工具 `yiime`（`query` 查候选、`translate` 测试翻译），提供 tar.gz 与 .deb。
