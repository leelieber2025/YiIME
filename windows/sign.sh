#!/usr/bin/env bash
# Authenticode-signs Windows binaries (EXE, DLL, installer) with osslsigncode, on Linux/macOS.
# Opt-in: without a certificate this prints a notice and exits 0, so unsigned builds keep working.
#
#   windows/sign.sh <file>...
#
# Certificate (code-signing PFX/PKCS#12), from the environment:
#   WINDOWS_SIGN_PFX           path to the .pfx, or
#   WINDOWS_SIGN_PFX_BASE64    the .pfx base64-encoded (GitHub secret)
#   WINDOWS_SIGN_PFX_PASSWORD  its password
#   WINDOWS_SIGN_TIMESTAMP_URL RFC 3161 timestamp server (default http://timestamp.digicert.com)
#   WINDOWS_SIGN_DESCRIPTION   description shown by UAC/SmartScreen (default "YiIME 译")
# On Windows, windows/sign.ps1 does the same with signtool. See SIGNING.md for certificates
# (OV/EV, Azure Trusted Signing, SignPath for open source).
set -euo pipefail
[ "$#" -gt 0 ] || { echo "用法：windows/sign.sh <文件>..." >&2; exit 2; }
pfx="${WINDOWS_SIGN_PFX:-}"
cleanup=""
if [ -z "$pfx" ] && [ -n "${WINDOWS_SIGN_PFX_BASE64:-}" ]; then
    pfx="$(mktemp)"
    cleanup="$pfx"
    printf '%s' "$WINDOWS_SIGN_PFX_BASE64" | base64 -d > "$pfx"
fi
trap '[ -n "$cleanup" ] && rm -f "$cleanup"' EXIT
if [ -z "$pfx" ]; then
    echo "未配置代码签名证书（WINDOWS_SIGN_PFX / WINDOWS_SIGN_PFX_BASE64），跳过签名：$*"
    exit 0
fi
command -v osslsigncode >/dev/null || { echo "需要 osslsigncode（apt install osslsigncode）" >&2; exit 1; }
timestamp="${WINDOWS_SIGN_TIMESTAMP_URL:-http://timestamp.digicert.com}"
description="${WINDOWS_SIGN_DESCRIPTION:-YiIME 译}"
for file in "$@"; do
    signed="$file.signed"
    args=(sign -pkcs12 "$pfx" -pass "${WINDOWS_SIGN_PFX_PASSWORD:-}" -h sha256
          -n "$description" -i "https://github.com/leelieber2025/YiIME" -in "$file" -out "$signed")
    if [ "$timestamp" != "none" ]; then
        args+=(-ts "$timestamp")
    fi
    osslsigncode "${args[@]}" || { rm -f "$signed"; exit 1; }
    mv "$signed" "$file"
    echo "已签名：$file"
done
