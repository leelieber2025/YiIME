# 安全与隐私（Security & privacy）

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

值得关注的问题包括：密钥泄露、输入内容被意外发送或记录、安装包与输入法 DLL 的提权风险、
输入框服务与 TSF DLL 之间的进程间通信被其他程序冒用。

## 什么会离开你的电脑

打字、查候选、选字、自学习都只在本机进行，**不联网**。只有按 Enter 翻译（或在输入记忆里重新翻译）
时，草稿中的中文会通过 HTTPS 发送到**一个**翻译服务：设置里选择的那个。

| 翻译引擎（设置 →“翻译引擎”） | 发送到 | 发送内容 | 凭据 |
| --- | --- | --- | --- |
| Azure 翻译（**默认**） | `api.cognitive.microsofttranslator.com`（微软） | 草稿文本 | Azure 密钥放在请求头 `Ocp-Apim-Subscription-Key`，区域在 `Ocp-Apim-Subscription-Region` |
| ↳ 默认设置下**没填 Azure 密钥时** | 改用下面的 Google 免费接口 | 同上 | 无 |
| Google 翻译（免费） | `translate.googleapis.com`（Google 非官方免费接口，可能限流） | 草稿文本（长文本按句子分段，每段一个请求，放在 URL 查询参数里） | 无 |
| Google Cloud 翻译 | `translation.googleapis.com`（Google Cloud Translation v2） | 草稿文本（请求体） | API 密钥放在请求头 `X-goog-api-key`，不放进 URL |
| DeepL | `api-free.deepl.com`（密钥以 `:fx` 结尾）或 `api.deepl.com` | 草稿文本 | `Authorization: DeepL-Auth-Key …` 请求头 |
| 大模型（OpenAI 兼容） | 你填写的 API 地址（默认 `api.openai.com`） | 草稿文本 + 系统提示词（翻译风格、你的自定义要求）+ 模型名 | `Authorization: Bearer …` 请求头 |

- **备用服务默认关闭。** 只有勾选“失败时允许改用其他翻译服务”（`settings.json` 的 `allow_fallback`），
  所选服务失败时才会把同一段文字再发给另一个服务（已配置的大模型；Azure 失败后的 Google 免费接口）。
  关闭时直接显示错误，并提示可以打开这个选项。
- 输入框在开始打字时会对当前翻译服务（以及已配置的大模型地址）做一次连接预热：只建立 TLS 连接，不发送文字或密钥，最多每 30 秒一次。
- macOS 版（测试版）只用 Google 免费接口；在系统钥匙串（服务 `YiIME`、账户 `llm`）和
  `com.yiime.inputmethod.YiIME` 偏好设置里配置了大模型时，Google 失败后改用大模型。
- 系统代理（`HTTPS_PROXY` 等环境变量）会被遵循；没有其他遥测、统计或更新检查。

## 本机保存的文件（Windows）

所有用户数据都在当前用户目录下。DPAPI = Windows 数据保护 API，按当前 Windows 账户加密，
别的账户或拷到别的电脑上无法解密。

| 位置 | 内容 | 保护 | 默认 |
| --- | --- | --- | --- |
| `%APPDATA%\YiIME\settings.json` | 翻译引擎、API 地址、模型、翻译风格、自定义要求、Azure 区域、是否允许备用服务 | 明文（不含密钥） | — |
| `%APPDATA%\YiIME\key.dpapi`、`google.dpapi`、`azure.dpapi`、`deepl.dpapi` | 各服务的 API 密钥 | DPAPI | 填写后才有 |
| `%APPDATA%\YiIME\history.dat` | **输入记忆**：翻译过的中文原文、译文和时间，最多 500 条 | DPAPI | **默认开启**；设置里可关闭（关闭时删除）或“清除输入记忆” |
| `%APPDATA%\YiIME\learned.dat` | 自学习：编码 → 选过的词、分数和最后使用时间（带时间衰减，最多 2 万条） | DPAPI；临时文件 + 原子替换写入 | 默认关闭 |
| `%APPDATA%\YiIME\user_words.txt` | 用户词库（“词 编码”，含导入的搜狗/QQ 词库） | 明文 | 由你创建 |
| `%APPDATA%\YiIME\ime-list-added` | 标记：已把输入法加入当前用户的输入法列表 | — | — |
| `HKCU\Software\YiIME` | 开关、快捷键、每页候选数、输入方案、是否已显示隐私说明 | 注册表 | — |
| `%TEMP%\YiIME-tsf.log`（超过 512 KB 轮换为 `.log.old`） | TSF DLL 诊断事件：时间、进程号、宿主程序文件名、事件（如“Shift 单击切换中/英”“按键交给应用：只读/禁用”） | 明文；**不含输入的文字** | 总是 |
| `%TEMP%\YiIME-translate.log`（同样轮换） | 每次翻译请求的引擎、模型、主机名、字数、耗时、成功或错误类别 | 明文；**不含原文、译文和密钥**（有单元测试保证） | 总是 |
| `%TEMP%\YiIME-service.log`（同样轮换） | 输入框服务的事件：会话开始/结束/切换应用、输入框隐藏的原因、看门狗退出；时间、进程号 | 明文；**不含输入的文字** | 每次会话一两行，超过 512 KB 轮换 |
| `HKLM\Software\YiIME\CurrentDir`、`Program Files\YiIME\<版本目录>` | 安装程序写入的当前版本目录和程序文件 | 需要管理员权限修改 | — |

0.1.x 版本用的是 `%APPDATA%\InputTranslation`、`HKCU\Software\InputTranslation` 和明文 `learned.txt`；
新版首次启动时会自动迁移到上面的新位置（`learned.txt` 转成加密的 `learned.dat` 后删除）。
卸载默认保留这些设置；用 `Uninstall-YiIME.ps1 -RemoveSettings` 一并删除（新旧位置都会删）。

首次启动托盘/设置程序时会显示一次简短的隐私说明（记录在 `HKCU\Software\YiIME\PrivacyNoticeShown`）。

## 进程与权限

- TSF 输入法 DLL 被加载到每个有文本框的程序里；它只负责转发按键和把结果写回文本框，
  候选和翻译都在单独的输入框进程（`YiIME.exe`，所有程序共享一个常驻实例，空闲 30 分钟后退出）里完成。
- DLL 只接受它自己启动、或校验过可执行文件路径与自己同目录版本的输入框进程发来的提交；
  输入框只接受窗口类为 TSF 接收窗口的会话请求。
- **以管理员身份运行（提升权限）或高完整性级别的程序**不会连接共享输入框服务：低权限进程不能往高权限程序里打字。
  这类程序里输入法启动自己的独立输入框（继承该程序的权限级别、窗口标题不同，别的程序找不到），只为它服务，程序退出或切走输入法时一起退出。
  读不出权限信息时也按这种方式处理。
- 普通程序只连接**同一用户、同一会话、同一完整性级别、未提升权限**的共享服务（检查进程令牌和会话，并核对可执行文件路径）；
  找到的服务不符合时忽略它，另起一个自己级别的服务。不同完整性级别的共享服务窗口名称不同，不会互相冲突。
- DLL 对输入框服务的每个请求都有 300 毫秒超时（`SendMessageTimeout` + `SMTO_ABORTIFHUNG`），服务卡住时不会冻结宿主程序：
  本次会话放弃、按键还给程序，下次激活时结束卡住的进程（先核对可执行文件路径）并重启，连续出问题时按 2 秒起、最长 1 分钟的间隔退避。
  服务自身有看门狗：消息循环超过 5 秒没有心跳（启动后 20 秒内不检查）就记录并退出。输入框关闭了键盘类文本服务（`GUID_COMPARTMENT_KEYBOARD_DISABLED`，浏览器的密码框就是这样）、为空上下文、只读，或是带 `ES_PASSWORD` 的 Win32 编辑框时，不启用输入法。InputScope 声明为密码或 PIN（`IS_PASSWORD`、`IS_NUMERIC_PASSWORD`、`IS_*PIN*`，如浏览器网页里的密码框）的输入框也不启用输入法，字母直接交给应用，不会在候选条或草稿框里明文显示。只声明为私密（`IS_PRIVATE`，如浏览器无痕窗口）的输入框照常输入，但不学习、不记入历史、不发送到翻译服务。
- 发布包可以选择进行代码签名，见 [SIGNING.md](SIGNING.md)。
