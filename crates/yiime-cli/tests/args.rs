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

#[test]
fn common_characters_by_default_and_all_chars_option() {
    let run = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_yiime"))
            .args(["query", "yxyy", "--no-words", "--tsv"])
            .args(extra)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(!run(&[]).contains('𧦮'));
    assert!(run(&["--all-chars"]).contains('𧦮'));
}
