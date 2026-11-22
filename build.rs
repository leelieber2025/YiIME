fn main() {
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/tsf/ime.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("windows/app.rc", ["windows_gui"], version_macros())
            .manifest_optional()
            .expect("程序图标资源编译失败");
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
