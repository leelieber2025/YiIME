use std::process::Command;

fn query(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_yiime"))
        .arg("query")
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn words_does_not_swallow_next_option() {
    let (code, stderr) = query(&["nihao", "--words", "--tsv"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("missing path for --words"), "{stderr}");
    assert!(!stderr.contains("--tsv 失败"));
}

#[test]
fn words_without_value_is_an_error() {
    let (code, stderr) = query(&["nihao", "--words"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("missing path for --words"), "{stderr}");
}

#[test]
fn no_words_query_still_works() {
    let (code, _) = query(&["nihao", "--no-words", "--tsv"]);
    assert_eq!(code, 0);
}
