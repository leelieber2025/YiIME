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
    let frost_target = out.join("frost.idx");
    if std::env::var_os("CARGO_FEATURE_BUNDLED").is_none() {
        // Still create the files so `include_bytes!` paths stay valid if features change.
        std::fs::write(&target, b"").expect("write index");
        std::fs::write(&frost_target, b"").expect("write index");
        return;
    }
    // The large lexicon is shipped as a separate file and still read at run time, but its sort
    // orders are precompiled here; they are used only if the file read at run time is identical.
    let frost_path = root.join("frost/YiIME-words.txt");
    println!("cargo:rerun-if-changed={}", frost_path.display());
    let frost = match std::fs::read(&frost_path) {
        Ok(raw) => {
            let hash = table::fnv1a(&raw);
            let len = raw.len();
            let text = table::normalize_frequency_pinyin(String::from_utf8(raw).expect("UTF-8"));
            table::encode_frost(hash, len, &table::frost_index(&text, true))
        }
        Err(_) => Vec::new(),
    };
    std::fs::write(&frost_target, frost).expect("write index");
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
