# 发布流程

每个正式版之前先发一个**候选版本（rc）**，在真实 Windows 上装一遍、用几天，没有问题再发正式版。
发布始终是手动步骤：`release.yml` 只构建并上传工作流产物，不会自动创建 GitHub Release。

## 1. 准备

1. 从 `main` 开始，确认 CI 全绿（Linux 测试、Windows 构建与 `--self-test`、macOS 构建）。
2. 在 `CHANGELOG.md` 的 `[Unreleased]` 下写好本次变化（新增 / 变更 / 移除 / 修复 / 安全）。
3. 本地检查：

   ```bash
   cargo fmt --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo clippy --workspace --all-targets --locked --target x86_64-pc-windows-gnu -- -D warnings
   cargo test --workspace --locked
   bash windows/build.sh
   ```

## 2. 候选版本（rc）

1. 根目录 `Cargo.toml` 的 `[workspace.package] version` 改为 `X.Y.Z-rc.N`（版本号只在这里设置），
   运行 `cargo check --workspace` 更新 `Cargo.lock`，提交：`release X.Y.Z-rc.N`。
   Windows 版本信息与 macOS bundle 版本使用数字部分 `X.Y.Z`，文件名带完整版本
   （`YiIME-X.Y.Z-rc.N-setup.exe`）。
2. 打标签并推送：`git tag vX.Y.Z-rc.N && git push origin vX.Y.Z-rc.N`。`release.yml` 会构建
   EXE/DLL/zip、安装包和 macOS pkg；配置了证书时自动签名（见 [SIGNING.md](SIGNING.md)）。
   Linux 的 tar.gz 与 .deb 来自同一标签上 `ci.yml` 的 `linux-*` 产物。
3. 从工作流产物下载文件，生成 `SHA256SUMS.txt`（`sha256sum * > SHA256SUMS.txt`），
   在 GitHub 上新建 Release，勾选 **Set as a pre-release**，正文写 CHANGELOG 中本次的内容。
4. 在真实 Windows 10/11 上验证，至少：
   - 全新安装与从上一个正式版升级（设置、用户词库、输入记忆是否保留；重启后新 DLL 生效）；
   - 记事本、浏览器、Office/WPS、终端、管理员权限窗口中打字、翻译、上屏；
   - 多个应用之间切换（共享输入框服务）、密码框不启用；
   - 已签名时用 `Get-AuthenticodeSignature` 检查 EXE、DLL、安装包；
   - 卸载（含 `-RemoveSettings`）。
5. 发现问题就修复后发 `rc.N+1`。

## 3. 正式版

1. 版本改为 `X.Y.Z`，把 `CHANGELOG.md` 的 `[Unreleased]` 改成 `[X.Y.Z] - YYYY-MM-DD`（发布当天，美东时间），
   新建空的 `[Unreleased]`，更新文末的比较链接，提交：`release X.Y.Z`。
2. `git tag vX.Y.Z && git push origin vX.Y.Z`，等 `release.yml` 完成。
3. 下载产物、生成 `SHA256SUMS.txt`、创建 Release（正式版不勾选 pre-release），附上：
   `YiIME-X.Y.Z-setup.exe`、`YiIME-X.Y.Z-portable.zip`、`YiIME-X.Y.Z-macos-beta.pkg`、
   `yiime-X.Y.Z-linux-*.tar.gz`、`yiime_X.Y.Z-1_*.deb`、`SHA256SUMS.txt`。
   `YiIME-X.Y.Z-tsf-test.zip` 只用于手动测试，不上传。
4. 发布正文提醒升级后重启电脑。
