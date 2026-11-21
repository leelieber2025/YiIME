#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:=x86_64-w64-mingw32-gcc}"
: "${CC_x86_64_pc_windows_gnu:=x86_64-w64-mingw32-gcc}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER CC_x86_64_pc_windows_gnu
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
registry_root="${CARGO_HOME:-$HOME/.cargo}/registry/src"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$registry_root=/cargo/registry/src --remap-path-prefix=$PWD=/src"
# It is built in two steps: the dependent features in the same cargo call will be merged, and the
# TSF DLL will be brought with all the default features of yiime-core (thesaurus, simplified and
# traditional vocabulary, translation client); the windows-gnu target does not recycle unused code
# according to functions, and the DLL volume will be several times larger.
cargo build --release --target x86_64-pc-windows-gnu -p input_translation
cargo build --release --target x86_64-pc-windows-gnu -p input_translation_tsf
python3 windows/package.py
