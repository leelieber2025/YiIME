#!/usr/bin/env bash
# Build YiIME.app on macOS (arm64 + x86_64 universal binary, ad-hoc signed) and the unsigned
# installation package dist/YiIME-<version>-macos.pkg (installed to /Library/Input Methods).
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:-0.1.1}"
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
sed "s/0\.1\.0/$version/g" macos/YiIME/Info.plist > "$app/Contents/Info.plist"
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
codesign --force --deep --sign - "$app"
codesign --verify --verbose "$app"
lipo -info "$app/Contents/MacOS/YiIME"
pkgbuild --root "$out/root" --identifier com.yiime.inputmethod.YiIME.pkg --version "$version" \
    --install-location "/Library/Input Methods" --scripts macos/pkg-scripts "$out/YiIME-component.pkg"
productbuild --package "$out/YiIME-component.pkg" "dist/YiIME-$version-macos.pkg"
rm "$out/YiIME-component.pkg"
ls -l "dist/YiIME-$version-macos.pkg"
