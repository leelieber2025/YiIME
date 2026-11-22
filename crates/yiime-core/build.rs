//! Precompiles the bundled Rime dictionaries into the binary index in `src/table.rs`, so the
//! library never parses YAML at run time.

#[path = "src/table.rs"]
mod table;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let wubi_path = root.join("wubi/wubi86.dict.yaml");
    let extra_path = root.join("wubi_extra/wubi86_extra.dict.yaml");
    let pinyin_path = root.join("pinyin/pinyin_simp.dict.yaml");
    for path in [&wubi_path, &extra_path, &pinyin_path] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=src/table.rs");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target = out.join("bundled.idx");
    if std::env::var_os("CARGO_FEATURE_BUNDLED").is_none() {
        // Still create the file so `include_bytes!` paths stay valid if features change.
        std::fs::write(&target, b"").expect("write index");
        return;
    }
    let read = |path: &std::path::Path| {
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    };
    let mut wubi = table::parse_dictionary(&read(&wubi_path), false);
    table::merge_extra(
        &mut wubi,
        table::parse_dictionary(&read(&extra_path), false),
    );
    let pinyin = table::parse_dictionary(&read(&pinyin_path), true);
    std::fs::write(&target, table::encode(&wubi, &pinyin)).expect("write index");
}
