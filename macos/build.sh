#!/usr/bin/env bash
# Build YiIME.app on macOS (arm64 + x86_64 universal binary) and the installation package
# dist/YiIME-<version>-macos-beta.pkg (installed to /Library/Input Methods).
#
# Signing is opt-in: by default the app is ad-hoc signed and the package unsigned. With
#   MACOS_SIGN_IDENTITY="Developer ID Application: …"      the app is signed with the hardened
#                                                          runtime and a secure timestamp, and
#   MACOS_INSTALLER_IDENTITY="Developer ID Installer: …"   the package is signed;
# then macos/notarize.sh notarizes and staples it. See SIGNING.md.
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(tools/version.sh)"
# Bundle versions must be numeric: a release candidate (0.2.0-rc.1) uses 0.2.0 inside the
# bundle and keeps the full version in the file name.
bundle_version="${version%%-*}"
out=dist/macos
rm -rf "$out" && mkdir -p "$out"
export MACOSX_DEPLOYMENT_TARGET=11.0
for arch in aarch64 x86_64; do
    rustup target add "$arch-apple-darwin" >/dev/null
    cargo build --release -p yiime-ffi --target "$arch-apple-darwin"
    swift_arch=$([ "$arch" = aarch64 ] && echo arm64 || echo x86_64)
    swiftc -O -target "$swift_arch-apple-macos11.0" \
        -import-objc-header macos/YiIME/yiime.h \
        macos/YiIME/main.swift macos/YiIME/InputController.swift \
        "target/$arch-apple-darwin/release/libyiime_ffi.a" \
        -framework Cocoa -framework InputMethodKit -framework Security -framework SystemConfiguration \
        -o "$out/YiIME-$swift_arch"
done
app="$out/root/YiIME.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create "$out/YiIME-arm64" "$out/YiIME-x86_64" -output "$app/Contents/MacOS/YiIME"
sed "s/@VERSION@/$bundle_version/g" macos/YiIME/Info.plist > "$app/Contents/Info.plist"
cp macos/YiIME/icon.tiff "$app/Contents/Resources/"
for lang in en zh-Hans; do
    mkdir -p "$app/Contents/Resources/$lang.lproj"
    printf '"CFBundleName" = "YiIME";\n"com.yiime.inputmethod.YiIME" = "YiIME";\n"com.yiime.inputmethod.YiIME.Hans" = "YiIME";\n' \
        > "$app/Contents/Resources/$lang.lproj/InfoPlist.strings"
done
cp LICENSE "$app/Contents/Resources/LICENSE.txt"
mkdir -p "$app/Contents/Resources/data"
cp data/README.md "$app/Contents/Resources/data/"
for dir in wubi pinyin wubi_extra opencc frost; do
    mkdir -p "$app/Contents/Resources/data/$dir"
    find "data/$dir" -maxdepth 1 \( -name 'LICENSE*' -o -name AUTHORS \) -exec cp {} "$app/Contents/Resources/data/$dir/" \;
done
plutil -lint "$app/Contents/Info.plist"
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --deep --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
else
    codesign --force --deep --sign - "$app"
fi
codesign --verify --strict --verbose "$app"
lipo -info "$app/Contents/MacOS/YiIME"
pkgbuild --root "$out/root" --identifier com.yiime.inputmethod.YiIME.pkg --version "$bundle_version" \
    --install-location "/Library/Input Methods" --scripts macos/pkg-scripts "$out/YiIME-component.pkg"
if [ -n "${MACOS_INSTALLER_IDENTITY:-}" ]; then
    productbuild --sign "$MACOS_INSTALLER_IDENTITY" --timestamp \
        --package "$out/YiIME-component.pkg" "dist/YiIME-$version-macos-beta.pkg"
else
    productbuild --package "$out/YiIME-component.pkg" "dist/YiIME-$version-macos-beta.pkg"
fi
rm "$out/YiIME-component.pkg"
ls -l "dist/YiIME-$version-macos-beta.pkg"
