#!/usr/bin/env bash
# Builds dist/YiIME-<version>-setup.exe with Inno Setup 6 (run windows/build.sh first). On Linux
# ISCC.exe runs under wine; set ISCC to its path (default: ISCC.exe on PATH).
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(tools/version.sh)"
for file in dist/yiime_tsf.dll dist/yiime_tsf_x86.dll dist/YiIME.exe dist/BUILDINFO.txt; do
    [ -f "$file" ] || { echo "缺少 $file，请先运行 windows/build.sh" >&2; exit 1; }
done
hash12() { sha256sum "$1" | cut -c1-12 | tr a-f A-F; }
# The DLL part covers both DLLs (64- and 32-bit), built from the same source.
version_dir="$(cat dist/yiime_tsf.dll dist/yiime_tsf_x86.dll | sha256sum | cut -c1-12 | tr a-f A-F)-$(hash12 dist/YiIME.exe)"
iscc="${ISCC:-ISCC.exe}"
if command -v wine >/dev/null && [ "${OS:-}" != "Windows_NT" ]; then
    export WINEDEBUG="${WINEDEBUG:--all}"
    run=(wine "$iscc")
    script="$(winepath -w windows/installer/yiime.iss)"
else
    run=("$iscc")
    script="windows/installer/yiime.iss"
    # Git Bash (MSYS) would rewrite ISCC's /Q, /D… and /S… switches into Windows paths.
    export MSYS2_ARG_CONV_EXCL='*'
fi
# ISCC_SIGNTOOL: optional sign command for Inno Setup ending in $f (e.g. `signtool.exe sign /fd
# SHA256 /f cert.pfx /p … /tr http://timestamp.digicert.com /td SHA256 $f`); ISCC then signs the
# setup and the uninstaller inside it.
sign_args=()
if [ -n "${ISCC_SIGNTOOL:-}" ]; then
    sign_args=("/DSignTool=yiime" "/Syiime=$ISCC_SIGNTOOL")
fi
"${run[@]}" /Q "/DAppVersion=$version" "/DVersionNumber=${version%%-*}" "/DVersionDir=$version_dir" "${sign_args[@]}" "$script"
# Otherwise sign the finished installer (opt-in; no-op without a certificate, see SIGNING.md).
if [ -z "${ISCC_SIGNTOOL:-}" ]; then
    if [ "${OS:-}" = "Windows_NT" ]; then
        pwsh -NoProfile -File windows/sign.ps1 "dist/YiIME-$version-setup.exe"
    else
        bash windows/sign.sh "dist/YiIME-$version-setup.exe"
    fi
fi
echo "版本目录：$version_dir"
ls -l "dist/YiIME-$version-setup.exe"
