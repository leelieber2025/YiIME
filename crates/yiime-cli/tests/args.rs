#![allow(clippy::unwrap_used)] // Tests may unwrap.
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
fn frost_words_are_reachable_by_pinyin_and_wubi() {
    let words = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/frost/YiIME-words.txt"
    );
    let run = |words_args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_yiime"))
            .args(["query", "tongguan", "iuud", "--pages", "2", "--tsv"])
            .args(words_args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    let with_words = run(&["--words", words]);
    assert!(
        with_words
            .lines()
            .any(|line| line.contains("tongguan") && line.contains("潼关"))
    );
    assert!(
        with_words
            .lines()
            .any(|line| line.contains("iuud") && line.contains("潼关"))
    );
    let without_words = run(&["--no-words"]);
    assert!(!without_words.contains("潼关"));
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

#[test]
fn import_converts_exports_into_user_words() {
    let dir = std::env::temp_dir().join(format!("yiime-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let export = dir.join("sogou.txt");
    std::fs::write(&export, "'ni'hao 你好\n'zhong'guo 中国\n").unwrap();
    let target = dir.join("user_words.txt");
    std::fs::write(&target, "你好 nihao\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_yiime"))
        .args([
            "import",
            export.to_str().unwrap(),
            "--into",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "你好 nihao\n中国 zhongguo\r\n"
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_yiime"))
        .args(["query", "--no-words", "--shuangpin", "xiaohe", "vsgo"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("1. 中国"));
    std::fs::remove_dir_all(&dir).unwrap();
}
