# 安全策略

## 支持的版本

只对最新发布的版本修复安全问题。

| 版本 | 支持 |
| --- | --- |
| 最新版本 | ✅ |
| 更早的版本 | ❌ |

## 报告漏洞

请**不要**在公开的 Issue 里报告安全漏洞，请使用 GitHub 的私密漏洞报告：
[报告漏洞](https://github.com/leelieber2025/YiIME/security/advisories/new)（仓库 → Security → Report a vulnerability）。

请写明问题描述、复现步骤和受影响的版本。

## 范围

YiIME 是本机输入法。翻译时会把草稿里的中文发给你选择的翻译服务（Google 或你配置的大模型）；
API 密钥在 Windows 上用 DPAPI 加密，只保存在本机。
值得关注的问题包括：密钥泄露、输入内容被意外发送或记录、安装包与输入法 DLL 的提权风险。
