# 代码签名（Code signing）

未签名的 `YiIME-x.y.z-setup.exe` 会被 Windows SmartScreen 提示“未知发布者 / 已保护你的电脑”，
macOS 也会拒绝打开未公证的安装包。签名完全是**可选**的：所有脚本和工作流在没有证书时都会打印
“跳过签名”并继续，产出与以前相同的未签名文件；仓库里不包含任何证书或密钥。

## 签什么

| 平台 | 文件 | 工具 |
| --- | --- | --- |
| Windows | `YiIME.exe`（输入框/托盘）、`yiime_tsf.dll`（TSF 输入法 DLL，装进每个有文本框的进程） | `windows/sign.sh`（Linux，osslsigncode）或 `windows/sign.ps1`（Windows，signtool），由 `windows/build.sh` 在打包**之前**调用，所以 ZIP、BUILDINFO 里的哈希和安装目录名都对应签名后的文件 |
| Windows | `YiIME-x.y.z-setup.exe` 及其中的卸载程序 | `windows/build-installer.sh`：设置 `ISCC_SIGNTOOL` 时由 Inno Setup 同时签安装程序和卸载程序；否则安装包生成后用上面的脚本签名 |
| macOS | `YiIME.app`（Developer ID Application，hardened runtime + 时间戳）、`YiIME-x.y.z-macos.pkg`（Developer ID Installer），然后公证并装订 | `macos/build.sh`（`MACOS_SIGN_IDENTITY`、`MACOS_INSTALLER_IDENTITY`）+ `macos/notarize.sh` |

## 本地使用

```bash
# Windows 二进制（Linux 上交叉编译时）
export WINDOWS_SIGN_PFX=/path/to/codesign.pfx        # 或 WINDOWS_SIGN_PFX_BASE64=...
export WINDOWS_SIGN_PFX_PASSWORD=...
# 可选：WINDOWS_SIGN_TIMESTAMP_URL（默认 http://timestamp.digicert.com；设为 none 不加时间戳）
bash windows/build.sh && bash windows/build-installer.sh

# macOS
export MACOS_SIGN_IDENTITY="Developer ID Application: 名字 (TEAMID)"
export MACOS_INSTALLER_IDENTITY="Developer ID Installer: 名字 (TEAMID)"
export APPLE_ID=... APPLE_TEAM_ID=... APPLE_APP_PASSWORD=...   # appleid.apple.com 生成的 App 专用密码
bash macos/build.sh && bash macos/notarize.sh
```

## GitHub Actions（`.github/workflows/release.yml`）

推送 `v*` 标签或手动运行时构建 Windows 二进制、安装包和 macOS 安装包，并作为 workflow artifact
上传（发布 GitHub Release 仍是手动步骤）。在仓库 *Settings → Secrets and variables → Actions*
里配置以下 secrets 即启用对应签名，不配置则跳过：

| Secret | 用途 |
| --- | --- |
| `WINDOWS_SIGN_PFX_BASE64` | 代码签名证书 `.pfx` 的 base64（`base64 -w0 cert.pfx`） |
| `WINDOWS_SIGN_PFX_PASSWORD` | `.pfx` 密码 |
| `MACOS_CERT_P12_BASE64` / `MACOS_CERT_PASSWORD` | 导出的 Developer ID Application + Installer 证书（一个 `.p12`）及密码 |
| `MACOS_SIGN_IDENTITY` / `MACOS_INSTALLER_IDENTITY` | 两个证书的完整名称 |
| `APPLE_ID` / `APPLE_TEAM_ID` / `APPLE_APP_PASSWORD` | 公证（notarytool）凭据 |

## 去哪里拿 Windows 证书

自 2023 年 6 月起，CA 签发的代码签名证书私钥必须保存在硬件（USB 令牌 / HSM / 云 HSM）里，
**新买的 OV/EV 证书通常拿不到可导出的 `.pfx`**。因此上面的 PFX 方式适合：已有的 `.pfx`、
自建测试证书，或能导出 PKCS#12 的服务。对于新项目更实际的选择：

- **Azure Artifact Signing**（原 Trusted Signing，按月付费、证书由微软托管，需完成身份验证；
  目前面向特定国家/地区的个人与组织）。在 Windows runner 上用
  [`azure/artifact-signing-action`](https://github.com/Azure/artifact-signing-action) 签名：
  把 `windows-binaries` job 改成先构建（`cargo build` 两次 + 不签名），再调用该 action 签
  `target/x86_64-pc-windows-gnu/release/*.exe|*.dll`，最后运行 `python3 windows/package.py`；
  安装包同理在 `windows-installer` job 末尾再签一次。
- **SignPath Foundation**：为开源项目免费提供代码签名（证书由 SignPath 持有，CI 通过其 GitHub
  集成提交签名请求，需要审核项目）。
- **Certum 开源代码签名证书**：价格较低，私钥在智能卡或 SimplySign 云令牌里，适合本地用
  signtool 签名后再上传发布文件。

注意：签名本身并不能让 SmartScreen 立刻放行，信誉是按证书/文件随下载量积累的；EV 证书现在也
不再自动跳过 SmartScreen。签名的主要作用是显示发布者名称、保证文件未被篡改，并为信誉积累打基础。

## 验证

```powershell
signtool verify /pa /v YiIME-x.y.z-setup.exe
Get-AuthenticodeSignature .\YiIME.exe
```

```bash
osslsigncode verify -in YiIME.exe
pkgutil --check-signature YiIME-x.y.z-macos.pkg && spctl --assess --type install -v YiIME-x.y.z-macos.pkg
```
