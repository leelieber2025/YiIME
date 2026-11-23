#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# The Windows target is no longer listed in rust-toolchain.toml; install it on demand.
if command -v rustup >/dev/null; then
    for target in x86_64-pc-windows-gnu i686-pc-windows-gnu; do
        rustup target list --installed | grep -qx "$target" || rustup target add "$target"
    done
fi
: "${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:=x86_64-w64-mingw32-gcc}"
: "${CC_x86_64_pc_windows_gnu:=x86_64-w64-mingw32-gcc}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER CC_x86_64_pc_windows_gnu
# 32-bit TSF DLL for 32-bit applications (WOW64), which cannot load the 64-bit one; like qingjian,
# which ships qingjian_tsf-<version>-x86.dll next to the 64-bit DLL.
: "${CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER:=i686-w64-mingw32-gcc-posix}"
: "${CC_i686_pc_windows_gnu:=i686-w64-mingw32-gcc-posix}"
export CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER CC_i686_pc_windows_gnu
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
registry_root="${CARGO_HOME:-$HOME/.cargo}/registry/src"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$registry_root=/cargo/registry/src --remap-path-prefix=$PWD=/src"
# Two separate builds: features are unified within one cargo call, so the TSF DLL would pull in all
# default yiime-core features (dictionaries, OpenCC tables, translation client), and windows-gnu
# does not strip unused code per function, making the DLL several times larger.
cargo build --release --target x86_64-pc-windows-gnu -p yiime-app
cargo build --release --target x86_64-pc-windows-gnu -p yiime-tsf
cargo build --release --target i686-pc-windows-gnu -p yiime-tsf
# Opt-in Authenticode signing before packaging (hashes, version directory and ZIPs then cover the
# signed files); a no-op without WINDOWS_SIGN_PFX / WINDOWS_SIGN_PFX_BASE64.
bash windows/sign.sh target/x86_64-pc-windows-gnu/release/windows_gui.exe \
    target/x86_64-pc-windows-gnu/release/yiime_tsf.dll target/i686-pc-windows-gnu/release/yiime_tsf.dll
python3 windows/package.py
