# YiIME 译

[![CI](https://github.com/leelieber2025/YiIME/actions/workflows/ci.yml/badge.svg)](https://github.com/leelieber2025/YiIME/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/leelieber2025/YiIME?include_prereleases)](https://github.com/leelieber2025/YiIME/releases)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
[![DOI](https://zenodo.org/badge/DOI/10.5281/zenodo.24430891.svg)](https://doi.org/10.5281/zenodo.24430891)

五笔/拼音输入法，自带“中文 → 英文”翻译：在任意应用里用中文打字，按 Enter 把译文直接上屏；也可以只打中文、不翻译。

| 平台 | 状态 |
| --- | --- |
| Windows 10/11 x64 | 输入法（TSF），主力平台 |
| macOS | 开发中，暂未发布（见下文） |
| Linux x86_64 / aarch64 | 暂时只有命令行工具（输入法计划用 Fcitx5） |

## 功能

- **五笔 86 + 拼音混输**：完整五笔码优先，其次全拼、简拼（`zgr` → 中国人、`yxyy` → 一心一意）、五笔前缀；拼音候选后标出五笔编码，方便记码。`'` 分隔音节（`xi'an` → 西安）。
- **词库**：rime 五笔 86、rime 简体拼音、补充多字词与成语（编进程序），以及单独随包的白霜（rime-frost）大词库 30 万条。
- **中译英**：
  - 默认 Azure 翻译（F0 免费档每月 200 万字符；填密钥和区域，未填时自动用免费 Google 翻译）；
  - 免费 Google 翻译（无需密钥，非官方接口，可能限流；限流后 5 分钟内自动跳过）；
  - 可选 Google Cloud Translation（填 API 密钥）；
  - 可选 DeepL（填 API 密钥，加密保存；以 `:fx` 结尾走免费接口，否则走 Pro）：目标语言为美式英文；
  - 可选兼容 OpenAI Chat Completions 的大模型（可选翻译风格和自定义要求；提示词要求美式英文）；
  - 翻译失败或超时时，若已配置大模型会自动改用大模型；Azure 失败且未配大模型时改用免费 Google。
  - 密钥用 Windows DPAPI 加密保存在本机。
- **浮动框草稿**：在光标旁弹出小输入框，可多句、带标点、移动光标修改。
  - **Enter**：翻译并上屏英文；
  - **Shift+Enter / Ctrl+Enter**：直接上屏中文，不翻译；
  - **Esc**：取消。
- **组字中途单击 Shift 夹英文**：进入英文段（底行显示“[英]”），字母、数字、空格、标点原样输入；再单击 Shift 回到中文。整段中英混合一起翻译或上屏。切换时还没选字的编码按字母原样上屏（与微软拼音、搜狗、Rime 的做法一致）。浮动框外单击 Shift 是全局中/英切换。
- **普通五笔模式**（不翻译）：Ctrl+Shift+Space、任务栏模式图标右键菜单或托盘菜单切换；选字直接上屏。任务栏图标：译 = 翻译模式，中 = 普通五笔模式，英 = 英文。
- **拼音容错**：相邻字母打反（`zhonggou` → 中国）、uen/iou/uei 简写、全拼简拼混打（`beijingdx` → 北京大学）、按音节逐段选字。
- **模糊音**（设置里开启）：z/zh、c/ch、s/sh、n/l、an/ang、en/eng、in/ing。
- **常用输入法约定**：
  - 空格/数字选字，`;` `'` 选第二/三候选；
  - `-` `=` 与 PageUp/PageDown 翻页（可选 `,` `.` 翻页）；
  - `[` `]` 以词定字；
  - 万能键 z、四码唯一自动上屏、五码顶字、空码清除；
  - 数字后标点保持半角；Ctrl+. 中/英文标点；Ctrl+Shift+F 简/繁输出（OpenCC）；
  - 小键盘数字不选字；引号括号自动配对（可选）。
- **自学习与用户词库**：按选用次数调整候选顺序（默认关），Ctrl+Shift+数字删除某条学习记录；用户词库 `%APPDATA%\InputTranslation\user_words.txt`，每行“词 编码”。
- **输入记忆**：记下翻译过的中文与译文（本机加密，最多 500 条）。Ctrl+Shift+Z 找回上一次的中文，可改后重新翻译。

各项开关在托盘图标菜单（或开始菜单“YiIME 译 设置”）的设置窗口里。

## 安装

1. 从 [Releases](https://github.com/leelieber2025/YiIME/releases) 下载 `YiIME-<版本>-setup.exe`，运行并按提示安装（需要管理员权限）。
2. 按 Win+空格 切换到“YiIME 译（五笔）”，在任意文本框里打字即可。
3. 首次使用可在设置窗口选择翻译引擎；不填 Azure 密钥也能直接用（免费 Google 翻译）。Azure 密钥在 Azure 门户创建 Translator 资源（定价层选 F0）后获取。

**升级**：直接运行新版安装程序，**装完后请重启电脑**（或注销后重新登录）。已经打开的应用里还加载着旧版输入法，不重启会出现新功能不生效的情况。

**卸载**：Windows“设置 → 应用”里卸载“YiIME 译”，或用开始菜单里的卸载项；建议卸载后重启。

另有免安装包 `InputTranslation-Portable.zip`（只含浮动框与设置程序，用于测试；完整功能需要安装输入法）。

### macOS（开发中）

macOS 输入法（InputMethodKit）的代码在 `macos/`，CI 能构建通用二进制和 .pkg，但还没有发布：
在 GitHub Actions 的 macOS 26 虚拟机上，输入法能注册，但系统不让自动启用：要在“系统设置”里由用户确认，而“添加输入法”列表里也找不到它（可能是因为没有签名，或者要重新登录，原因待查），所以端到端测试没能跑完。需要 Apple 开发者签名与公证后再发布。

### Linux（命令行）

Linux 暂时只有命令行工具 `yiime`，输入法（Fcitx5）在计划中。下载 `yiime_<版本>-1_amd64.deb`（或 arm64）用 `sudo apt install ./yiime_*.deb` 安装，或解压 `yiime-<版本>-linux-<架构>.tar.gz` 直接运行：

```bash
yiime query zhongguo        # 查候选：中国 …
yiime translate 你好世界     # 用免费 Google 翻译测试
```

## 从源码构建

Windows 版需要 Rust（版本见 `rust-toolchain.toml`）和 MinGW-w64 交叉编译器，在 Linux 上构建：

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get install gcc-mingw-w64-x86-64-posix binutils-mingw-w64-x86-64
bash windows/build.sh                   # 生成 dist/ 下的 EXE、DLL 和 zip
ISCC=/path/to/ISCC.exe bash windows/build-installer.sh   # 用 Inno Setup 6（Linux 上经 wine）生成安装包
```

macOS（开发中）：在 Mac 上运行 `bash macos/build.sh`，生成 `dist/YiIME-<版本>-macos.pkg`。Linux 命令行：`cargo build --release -p yiime-cli`，打包用 `tools/package-linux.sh [架构]` 和 `cargo deb -p yiime-cli`。

版本号只在根目录 `Cargo.toml` 的 `[workspace.package]` 里设置，各 crate、Windows 资源、安装包、macOS Info.plist 与打包脚本都从这里取（脚本用 `tools/version.sh`）。

测试：`cargo test --workspace`。平台无关的核心库 `crates/yiime-core` 和命令行 `yiime`（`yiime query <编码>`、`yiime translate <中文>`）可在任何平台运行。

| 目录 | 内容 |
| --- | --- |
| `crates/yiime-core` | 平台无关核心：词典与索引、候选引擎、拼音容错、草稿、标点与简繁、设置模型、翻译客户端 |
| `crates/yiime-cli` | 命令行工具 `yiime` |
| `src/bin/windows_gui.rs` | Windows 浮动框、托盘与设置窗口 |
| `windows/tsf` | TSF 输入法 DLL |
| `windows/installer` | Inno Setup 安装脚本 |
| `crates/yiime-ffi` | 给 macOS 输入法用的 C 接口 |
| `macos` | macOS 输入法（Swift + InputMethodKit）、打包与端到端测试脚本 |
| `data` | 词库与许可证（来源见 `data/README.md`） |
| `tools` | 词库生成脚本 |

## 路线图

- 更多 Windows 实机兼容性验证（32 位应用、管理员窗口等）；
- macOS 版：签名与公证、真机验证、设置界面，然后发布；
- Linux 输入法（Fcitx5）；
- 核心库保持平台无关，各平台只做输入法框架接入。

## 引用

如果 YiIME 对你的工作有帮助，请引用：

> Zhao Li. *YiIME 译*. Zenodo. https://doi.org/10.5281/zenodo.24430891

```bibtex
@software{li_yiime,
  author    = {Li, Zhao},
  title     = {YiIME 译},
  publisher = {Zenodo},
  doi       = {10.5281/zenodo.24430891},
  url       = {https://doi.org/10.5281/zenodo.24430891}
}
```

## 许可与致谢

本项目以 **GPL-3.0** 发布（见 `LICENSE`），因为随包分发了 GPL-3.0 的白霜词库。使用的第三方数据：

- [rime-frost 白霜拼音](https://github.com/gaboolic/rime-frost)：大词库与五笔词码，GPL-3.0；
- [rime-wubi](https://github.com/rime/rime-wubi)：五笔 86 词库，LGPL-3.0；
- [rime-pinyin-simp](https://github.com/rime/rime-pinyin-simp)：简体拼音词库，Apache-2.0；
- [OpenCC](https://github.com/BYVoid/OpenCC)：简繁转换表，Apache-2.0；
- [chinese-xinhua](https://github.com/pwxcoo/chinese-xinhua)：成语，MIT；
- [rime-wubi86-jidian](https://github.com/KyleBing/rime-wubi86-jidian)：五笔补充词条，MIT。

详细来源、快照版本与生成方式见 [`data/README.md`](data/README.md)，各许可证原文在 `data/` 对应目录下。

交互约定参考微软拼音/五笔、搜狗、Rime。

## 安全

见 [SECURITY.md](SECURITY.md)。更新记录见 [CHANGELOG.md](CHANGELOG.md)。
