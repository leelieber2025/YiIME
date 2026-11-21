fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // After declaring rerun-if-*, Cargo will no longer rerun this script due to changes in files in
    // the package; embed-resource will not report icons referenced by .rc. If not listed, the DLL
    // will always carry old resources after changing ime.rc / *.ico (v6–v9 DLLs only have the old
    // blue “five” brand icon and no 101–106 mode icon, this is why).
    println!("cargo:rerun-if-changed=ime.rc");
    for entry in std::fs::read_dir(".").into_iter().flatten().flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ico"))
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for_cdylib("ime.rc", embed_resource::NONE)
            .manifest_required()
            .expect("TSF 图标资源编译失败");
    }
}
