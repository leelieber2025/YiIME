"""Package both Windows distributions from the same EXE and source snapshot."""
from hashlib import sha256
from pathlib import Path
from shutil import rmtree
from shutil import which
import os
import subprocess
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

root = Path(__file__).resolve().parent.parent
registry_root = str(Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / "registry/src")
remapped_flags = os.environ.get("RUSTFLAGS", "").replace(registry_root, "${CARGO_HOME}/registry/src").replace(str(root), "${PWD}")
source_paths = [root / "Cargo.toml", root / "Cargo.lock", root / "rust-toolchain.toml"]
source_paths += sorted((root / "src").rglob("*.rs"))
source_paths += sorted((root / "windows").rglob("*.rs"))
source_paths += sorted((root / "windows").rglob("*.rc"))
source_paths += sorted((root / "windows").rglob("*.ico"))
source_paths += sorted((root / "windows").rglob("*.ps1"))
source_paths += sorted((root / "windows").rglob("*.sh"))
source_paths += sorted((root / "windows").rglob("*.txt"))
source_paths += [root / "windows/package.py", root / "windows/tsf/Cargo.toml"]
source_paths += sorted((root / "crates").rglob("*.rs"))
source_paths += sorted((root / "crates").rglob("Cargo.toml"))
source_paths += sorted((root / "crates").rglob("*.tsv"))
source_paths += sorted((root / "windows").rglob("*.iss"))
data_files = [root / "data/README.md",
              root / "data/wubi/wubi86.dict.yaml", root / "data/wubi/LICENSE", root / "data/wubi/AUTHORS",
              root / "data/pinyin/pinyin_simp.dict.yaml", root / "data/pinyin/LICENSE", root / "data/pinyin/AUTHORS",
              root / "data/wubi_extra/wubi86_extra.dict.yaml",
              root / "data/wubi_extra/LICENSE-rime-wubi86-jidian",
              root / "data/wubi_extra/LICENSE-chinese-xinhua",
              root / "data/frost/YiIME-words.txt", root / "data/frost/LICENSE",
              root / "data/opencc/LICENSE", root / "data/opencc/STCharacters.txt",
              root / "data/opencc/STPhrases.txt"]
source_paths += data_files
source_paths = [path for path in source_paths
                if not any(part.startswith(".") for part in path.relative_to(root).parts)]
source_hash = sha256()
for path in source_paths:
    source_hash.update(str(path.relative_to(root)).encode())
    source_hash.update(b"\0")
    source_hash.update(path.read_bytes())
    source_hash.update(b"\0")

dist = root / "dist"
if dist.exists():
    rmtree(dist)
dist.mkdir(exist_ok=True)
exe = dist / "YiIME.exe"
dll = dist / "yiime_tsf.dll"
exe.write_bytes((root / "target/x86_64-pc-windows-gnu/release/windows_gui.exe").read_bytes())
dll.write_bytes((root / "target/x86_64-pc-windows-gnu/release/yiime_tsf.dll").read_bytes())
info = (
    f"Source SHA-256: {source_hash.hexdigest()}\n"
    f"EXE SHA-256: {sha256(exe.read_bytes()).hexdigest()}\n"
    f"DLL SHA-256: {sha256(dll.read_bytes()).hexdigest()}\n"
    "Target: x86_64-pc-windows-gnu\n"
    f"rustc: {subprocess.check_output(['rustc', '-Vv'], text=True).strip()}\n"
    f"cargo: {subprocess.check_output(['cargo', '-V'], text=True).strip()}\n"
    f"Linker: {Path(os.environ.get('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER', 'x86_64-w64-mingw32-gcc')).name}\n"
    f"Linker setting: {os.environ.get('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER', 'x86_64-w64-mingw32-gcc').replace(str(Path.home()), '~')}\n"
    f"Linker version: {subprocess.check_output([os.environ.get('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER', 'x86_64-w64-mingw32-gcc'), '--version'], text=True).splitlines()[0]}\n"
    f"Linker SHA-256: {sha256(Path(which(os.environ.get('CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER', 'x86_64-w64-mingw32-gcc'))).read_bytes()).hexdigest()}\n"
    f"SOURCE_DATE_EPOCH: {os.environ.get('SOURCE_DATE_EPOCH', 'unset')}\n"
    f"Path remapping: {'enabled' if '--remap-path-prefix' in os.environ.get('RUSTFLAGS', '') else 'disabled'}\n"
    f"RUSTFLAGS template: {remapped_flags}\n"
)
(dist / "BUILDINFO.txt").write_text(info, encoding="utf-8")
version = subprocess.check_output(["bash", str(root / "tools/version.sh")], text=True).strip()
licenses = [data_files[index] for index in (0, 2, 3, 5, 6, 8, 9, 10, 11, 12)]
for name, files in {
    f"YiIME-{version}-portable.zip": [exe, root / "windows/使用说明.txt", dist / "BUILDINFO.txt", *licenses],
    f"YiIME-{version}-tsf-test.zip": [exe, dll, root / "windows/Install-YiIME.ps1",
                                      root / "windows/Uninstall-YiIME.ps1",
                                      root / "windows/TSF-使用说明.txt", dist / "BUILDINFO.txt", *licenses],
}.items():
    with ZipFile(dist / name, "w", ZIP_DEFLATED) as archive:
        for path in files:
            arcname = path.relative_to(root) if path.is_relative_to(root / "data") else path.name
            # Fixed timestamps and permissions to make ZIP reproducible.
            entry = ZipInfo(str(arcname).replace(os.sep, "/"), date_time=(1980, 1, 1, 0, 0, 0))
            entry.compress_type = ZIP_DEFLATED
            entry.external_attr = 0o644 << 16
            archive.writestr(entry, path.read_bytes())
print(info)
