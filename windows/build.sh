#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:=x86_64-w64-mingw32-gcc}"
: "${CC_x86_64_pc_windows_gnu:=x86_64-w64-mingw32-gcc}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER CC_x86_64_pc_windows_gnu
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
registry_root="${CARGO_HOME:-$HOME/.cargo}/registry/src"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$registry_root=/cargo/registry/src --remap-path-prefix=$PWD=/src"
# Two separate builds: features are unified within one cargo call, so the TSF DLL would pull in all
# default yiime-core features (dictionaries, OpenCC tables, translation client), and windows-gnu
# does not strip unused code per function, making the DLL several times larger.
cargo build --release --target x86_64-pc-windows-gnu -p input_translation
cargo build --release --target x86_64-pc-windows-gnu -p input_translation_tsf
python3 windows/package.py
