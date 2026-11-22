#!/usr/bin/env bash
# Builds dist/YiIME-<version>-setup.exe with Inno Setup 6 (run windows/build.sh first). On Linux
# ISCC.exe runs under wine; set ISCC to its path (default: ISCC.exe on PATH).
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(tools/version.sh)"
for file in dist/input_translation_tsf.dll dist/InputTranslation-Portable.exe dist/BUILDINFO.txt; do
    [ -f "$file" ] || { echo "缺少 $file，请先运行 windows/build.sh" >&2; exit 1; }
done
hash12() { sha256sum "$1" | cut -c1-12 | tr a-f A-F; }
version_dir="$(hash12 dist/input_translation_tsf.dll)-$(hash12 dist/InputTranslation-Portable.exe)"
iscc="${ISCC:-ISCC.exe}"
if command -v wine >/dev/null && [ "${OS:-}" != "Windows_NT" ]; then
    export WINEDEBUG="${WINEDEBUG:--all}"
    run=(wine "$iscc")
    script="$(winepath -w windows/installer/yiime.iss)"
else
    run=("$iscc")
    script="windows/installer/yiime.iss"
fi
"${run[@]}" /Q "/DAppVersion=$version" "/DVersionDir=$version_dir" "$script"
echo "版本目录：$version_dir"
ls -l "dist/YiIME-$version-setup.exe"
