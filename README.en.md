# YiIME — a Wubi + Pinyin input method for Windows, written in Rust, with inline Chinese → English translation

[中文](README.md) | English

[![Release](https://img.shields.io/github/v/release/leelieber2025/YiIME?include_prereleases)](https://github.com/leelieber2025/YiIME/releases)
[![CI](https://github.com/leelieber2025/YiIME/actions/workflows/ci.yml/badge.svg)](https://github.com/leelieber2025/YiIME/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/github/downloads/leelieber2025/YiIME/total)](https://github.com/leelieber2025/YiIME/releases)

YiIME 译 is a free, open-source (GPL-3.0) Windows 10/11 input method (TSF). Type Chinese in any
application with Wubi 86 and Pinyin mixed (no switching). The current development build composes
in the target field and picks a candidate there; **Ctrl+Alt+T** opens the draft editor for
translation. Released packages describe their own input flow. A plain Wubi mode without translation
is one shortcut away (Ctrl+Shift+Space).

## Download

Get the latest version from **[Releases](https://github.com/leelieber2025/YiIME/releases)**:

- Windows 10/11 x64: `YiIME-<version>-setup.exe` (installer, needs administrator rights; restart
  after installing or upgrading), `YiIME-<version>-portable.zip` (test build without the IME).
- macOS: `YiIME-<version>-macos-beta.pkg` — beta, not signed or notarized.
- Linux: command-line tool only (`.deb`, `.tar.gz`); an Fcitx5 input method is planned.

Versions with `-rc.N` are release candidates (marked pre-release). Verify downloads with
`SHA256SUMS.txt`. The installer is not code-signed yet, so Windows SmartScreen may show
"Windows protected your PC": choose "More info" → "Run anyway" after checking the checksum.

## Features

- Wubi 86 + full Pinyin mixed input, initials (`bjdx` → 北京大学), typo tolerance, optional fuzzy
  pinyin, `lve`/`nve`; Shuangpin layouts Xiaohe, Microsoft and Ziranma.
- Import Sogou `.scel` and Sogou/QQ text word lists into the user dictionary.
- Translation: Azure Translator (default; free Google Translate when no Azure key), free Google,
  Google Cloud, DeepL, or any OpenAI-compatible LLM. Fallback to another service is off by default.
- Common-characters filter (on by default): rare CJK extension characters are hidden and
  non-GB 2312 single characters (mostly Traditional forms) rank last for Simplified output.
- Optional learning with time decay, encrypted with DPAPI; translation history (on by default,
  encrypted, can be turned off and cleared).
- Privacy: what is sent where and every local file are listed in [SECURITY.md](SECURITY.md).

## Build from source

See the Chinese [README](README.md#从源码构建): `bash windows/build.sh` cross-builds the Windows
EXE, DLL and zips on Linux with MinGW; `cargo test --workspace --locked` runs the tests. Releases
follow [RELEASING.md](RELEASING.md); signing is opt-in, see [SIGNING.md](SIGNING.md). Changes:
[CHANGELOG.md](CHANGELOG.md).

## License

GPL-3.0 (the bundled rime-frost lexicon is GPL-3.0). Third-party data and licenses are listed in
[data/README.md](data/README.md).
