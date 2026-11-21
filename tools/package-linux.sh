#!/usr/bin/env bash
# Package Linux command line: dist/yiime-<version>-linux-<architecture>.tar.gz (cargo build
# --release -p yiime-cli first).
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:-0.1.0}"
arch="${2:-$(uname -m)}"
name="yiime-$version-linux-$arch"
stage="dist/$name"
rm -rf "$stage" && mkdir -p "$stage/data"
cp target/release/yiime data/frost/InputTranslation-words.txt LICENSE README.md "$stage/"
cp data/README.md "$stage/data/"
for dir in wubi pinyin wubi_extra opencc frost; do
    mkdir -p "$stage/data/$dir"
    find "data/$dir" -maxdepth 1 \( -name 'LICENSE*' -o -name AUTHORS \) -exec cp {} "$stage/data/$dir/" \;
done
tar -C dist -czf "dist/$name.tar.gz" "$name"
rm -rf "$stage"
ls -l "dist/$name.tar.gz"
