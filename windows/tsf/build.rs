fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // Once any rerun-if-* is declared, Cargo stops watching the package files, and embed-resource
    // does not report the icons ime.rc references, so list them here or icon changes are not
    // rebuilt.
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
        embed_resource::compile_for_cdylib("ime.rc", version_macros())
            .manifest_required()
            .expect("TSF 图标资源编译失败");
    }
}

/// VERSIONINFO comes from the Cargo package version.
fn version_macros() -> [String; 3] {
    let part =
        |name: &str| std::env::var(name).expect("Cargo sets CARGO_PKG_VERSION_* for build scripts");
    [
        format!("YIIME_VERSION_MAJOR={}", part("CARGO_PKG_VERSION_MAJOR")),
        format!("YIIME_VERSION_MINOR={}", part("CARGO_PKG_VERSION_MINOR")),
        format!("YIIME_VERSION_PATCH={}", part("CARGO_PKG_VERSION_PATCH")),
    ]
}
