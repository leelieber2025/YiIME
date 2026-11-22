#!/usr/bin/env bash
# Notarizes and staples dist/YiIME-<version>-macos.pkg (built by macos/build.sh with
# MACOS_SIGN_IDENTITY and MACOS_INSTALLER_IDENTITY). Opt-in: without credentials it prints a
# notice and exits 0.
#
# Credentials (an app-specific password for the Apple ID, see SIGNING.md):
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(tools/version.sh)"
pkg="dist/YiIME-$version-macos.pkg"
if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ] || [ -z "${APPLE_APP_PASSWORD:-}" ]; then
    echo "未配置公证凭据（APPLE_ID / APPLE_TEAM_ID / APPLE_APP_PASSWORD），跳过公证：$pkg"
    exit 0
fi
pkgutil --check-signature "$pkg"
xcrun notarytool submit "$pkg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
xcrun stapler staple "$pkg"
xcrun stapler validate "$pkg"
spctl --assess --type install --verbose "$pkg"
