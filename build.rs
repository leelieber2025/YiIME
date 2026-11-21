fn main() {
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/tsf/ime.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("windows/app.rc", ["windows_gui"], embed_resource::NONE)
            .manifest_optional()
            .expect("程序图标资源编译失败");
    }
}
