# 更新记录

本文件记录各版本的主要变化，格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)（0.x 期间次版本号可能包含不兼容的变化）。
日期为发布日期（美东时间）。发布流程见 [RELEASING.md](RELEASING.md)。

## [Unreleased]

升级后请重启电脑。从 0.1.x 升级时设置会自动迁移（见“变更”）。

### 新增

- **双拼**：小鹤、微软、自然码双拼（设置 →“输入方案”，可与五笔混输）；命令行 `--shuangpin`。
- **导入搜狗/QQ 词库**：设置里“导入搜狗/QQ 词库…”，支持 `.scel` 与文本导出（UTF-8/UTF-16），合并进用户词库；命令行 `yiime import`。
- **只显示常用字**（默认开）：隐藏扩展区生僻字、兼容/私用区字，以及低频简拼词和低频单字（如 `bjdx` 不再出“伯喈倒屣”）；命令行 `--all-chars` 关闭。
- `lve`/`nve` 与 `lue`/`nue` 等价（`lve` → 略）；编码里带 `'` 时只查拼音。
- 首次启动时显示一次隐私说明；`SECURITY.md` 列出每个翻译服务发送什么、本机保存了哪些文件，并随安装包和 zip 分发。
- 可选代码签名：Windows（signtool / osslsigncode，签 EXE、DLL 和安装包）与 macOS（Developer ID + 公证），没有配置证书时自动跳过；新增 `release.yml` 构建工作流与 `SIGNING.md`。
- `YiIME.exe --self-test`：无界面跑一遍输入框按键流程，Windows CI 中执行。

### 变更

- **所有应用共享一个常驻输入框服务**：TSF DLL 不再为每个应用各启动一个 `YiIME.exe`，而是找到（并校验路径）已在运行的服务并切换会话；空闲 30 分钟后服务退出。
- **词库预编译**：五笔、拼音和白霜大词库在构建时编成二进制表。命令行冷启动：不带白霜 0.18–0.22 秒 / 约 90 MB → 低于 0.01 秒 / 约 9 MB；带白霜 0.53–0.64 秒 / 约 115 MB → 0.06–0.07 秒 / 约 31 MB（程序体积 9.2 MB → 21.9 MB）。
- **自学习**：改为按“编码 + 词”聚合的加密表 `learned.dat`（DPAPI，原子写入，最多 2 万条并定期压缩），分数随时间衰减（半衰期 30 天）；旧的明文 `learned.txt` 自动迁移后删除。
- **备用翻译服务改为默认关闭**：只有勾选“失败时允许改用其他翻译服务”才会在失败时改用大模型或 Google；关闭时错误提示里说明如何开启。
- Google Cloud 翻译的密钥放进 `X-goog-api-key` 请求头，不再出现在 URL 里；免费 Google 翻译的长文本按句子分段请求，避免 URL 过长。
- 统一命名为 YiIME：`YiIME.exe`、`yiime_tsf.dll`、`Install-YiIME.ps1`/`Uninstall-YiIME.ps1`、`YiIME-words.txt`、`HKCU\Software\YiIME`、`%APPDATA%\YiIME`、新安装目录 `Program Files\YiIME`。旧的 `%APPDATA%\InputTranslation` 与 `HKCU\Software\InputTranslation` 首次启动时自动迁移；安装和卸载脚本同时处理新旧文件名。
- 输入框按键状态机移到 `yiime-core`（`keys` 模块），带单元测试。
- macOS 安装包统一命名为 `YiIME-<版本>-macos-beta.pkg`；候选版本（如 `0.2.0-rc.1`）的 Windows 版本信息和 macOS bundle 版本使用数字部分 `0.2.0`。
- `rust-toolchain.toml` 不再强制安装 Windows 目标（`windows/build.sh` 按需安装）。

### 修复

- 开始菜单“TSF 使用说明”快捷方式指向不存在的文件（0.1.0 起）。
- 升级时删除旧开始菜单快捷方式的文件名与实际不符。

### 安全

- 输入框只接受 TSF 接收窗口的会话请求；DLL 只接受它启动或校验过路径的输入框进程提交的文字。
- 自学习记录不再以明文保存（见“变更”）。

## [0.1.5] - 2026-11-22

### 新增

- 拼音优先：在设置中开启（默认关）。完整拼音排在完整五笔码前面；开启后不再四码唯一自动上屏，也不再五码顶字。命令行可用 `yiime query --pinyin-first`。

### 变更

- 白霜大词库改为后台加载。浮动框先用内置词库打开，加载完成且当前没有组字时再切换到大词库。
- CI 增加最低 Rust 版本检查：核心库 1.85，Windows 工程 1.88。

### 修复

- 翻译线程出错时提示“翻译处理出错，请重试”，不再让界面进程退出。
- TSF 回调、注册和接收窗口里的 panic 记入调试日志并返回失败，不再传给宿主程序。
- 草稿退格在光标位置异常时不再 panic。
- Windows 上请求超时不再被记成网络错误。

## [0.1.4] - 2026-11-21

### 新增

- DeepL：在设置中选择并填写 API 密钥（加密保存）。密钥以 `:fx` 结尾时使用免费接口，否则使用 Pro。译文为美式英文（`EN-US`）。失败且已配置大模型时改用大模型。

### 变更

- 大模型翻译改为要求美式英文（美式拼写和用法）。

## [0.1.3] - 2026-11-21

### 新增

- Azure 翻译（Azure Translator，F0 免费档每月 200 万字符）作为默认引擎：设置中填密钥（加密保存）与区域；未填密钥时自动使用免费 Google 翻译。
- 翻译结果缓存：同一句话再次翻译直接出结果，不再请求。

### 修复

- 免费 Google 翻译返回 429（限流）后，5 分钟内直接改用大模型（或 Azure 失败时的后备），不再每次先等 Google 失败。
- 推理模型因 max_tokens 被思考用尽而无输出时，自动去掉上限重试一次，成功后该模型不再加上限。
- Azure 失败且未配置大模型时改用免费 Google 翻译。
- 错误提示在服务商未返回说明时不再多出冒号。

## [0.1.2] - 2026-11-21

### 新增

- 发布中开始附带 macOS 测试版 `YiIME-0.1.2-macos-beta.pkg`（通用二进制，未签名、未公证）。

### 变更

- 版本号只在根目录 `Cargo.toml` 的 `[workspace.package]` 设置；各 crate、Windows 资源（VERSIONINFO）、安装包、macOS Info.plist 与打包脚本都从这里取（脚本用 `tools/version.sh`），打包脚本不再接受版本参数，`tools/package-linux.sh` 的第一个参数改为架构。
- CI 增加 `cargo fmt --check`。
- macOS CI：分别检测输入法的注册、枚举、启用与选中状态，并校验安装包版本与 Info.plist。
- 命令行帮助注明 `--words` 的路径不能以 `-` 开头。
- 整理代码注释：删去重复代码含义或提及旧版本的注释，改写机器翻译不通顺的注释。

### 修复

- macOS 安装包：Info.plist 的版本号不再随构建参数更新、CI 仍按 0.1.0 打包的问题；现在版本号统一取自 `Cargo.toml`。
- 大模型参数降级：只有去掉 temperature / max_tokens 后请求成功才记住该模型，不再因无关的 400（如不支持 system 角色）而永久降级。
- 网络错误提示带上不含网址的具体原因（如证书、DNS 错误），日志仍只记类别。
- Windows 资源版本号在 MinGW（windres）下编译失败的问题：版本字符串改为在 .rc 内由数字宏拼出。

## [0.1.1] - 2026-11-21

### 修复

- 翻译日志只记录错误类别与 HTTP 状态（及引擎、模型、主机、字数、耗时），不再写入服务商返回的错误文本。
- 大模型兼容性降级（不发送 temperature / max_tokens）改为按接口地址与模型分别记录，切换模型后恢复完整参数。
- 命令行 `--words` 缺少路径或后跟其它选项时给出明确错误，不再把下一个选项当作文件名。

## [0.1.0] - 2026-11-21

首个公开版本。

### 新增

Windows：

- TSF 输入法：五笔 86 优先的五笔/拼音混输、简拼、拼音容错、模糊音（可选）。
- 光标旁浮动框草稿：Enter 翻译上屏英文，Shift+Enter / Ctrl+Enter 上屏中文。
- 翻译引擎：免费 Google 翻译（默认）、Google Cloud Translation、OpenAI 兼容大模型，失败时自动改用大模型。
- 组字中途单击 Shift 夹英文；普通五笔模式（不翻译）。
- 常用输入法约定：以词定字、翻页、万能键、四码上屏、五码顶字、简繁输出、中英文标点、自学习、用户词库、输入记忆。
- Inno Setup 安装包，升级后需要重启。免安装测试包 `InputTranslation-Portable.zip`（0.1.1 起改名为 `YiIME-<版本>-portable.zip`）。

macOS（开发中，未发布）：

- InputMethodKit 输入法代码与 CI 构建（通用二进制、.pkg）；待签名公证后发布。

Linux：

- 命令行工具 `yiime`（`query` 查候选、`translate` 测试翻译），提供 tar.gz 与 .deb。

[Unreleased]: https://github.com/leelieber2025/YiIME/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/leelieber2025/YiIME/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/leelieber2025/YiIME/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/leelieber2025/YiIME/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/leelieber2025/YiIME/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/leelieber2025/YiIME/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/leelieber2025/YiIME/releases/tag/v0.1.0
