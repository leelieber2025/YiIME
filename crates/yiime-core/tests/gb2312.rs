//! With only common characters shown and Simplified output, single pinyin characters outside
//! GB 2312 (mostly Traditional forms) rank after the GB 2312 ones; --all-chars keeps the old order.

use yiime_core::{ComposeOptions, Composer, Learned, MixedInput, is_gb2312_hanzi};

fn texts(input: &MixedInput, code: &str, common_only: bool, traditional: bool) -> Vec<String> {
    let options = ComposeOptions {
        // One large page so candidate_at reaches the whole list.
        page_size: 60,
        traditional,
        learn: false,
        fuzzy: false,
        pinyin_first: true,
        common_only,
        shuangpin: None,
    };
    let mut composer = Composer::default();
    composer.set_code(input, code.to_owned(), &options, &Learned::default());
    (0..90)
        .map_while(|index| composer.candidate_at(index).map(|pick| pick.text))
        .collect()
}

fn position(list: &[String], text: &str) -> usize {
    list.iter()
        .position(|item| item == text)
        .unwrap_or_else(|| panic!("{text} not in {list:?}"))
}

#[test]
fn gb2312_table() {
    assert!(is_gb2312_hanzi('说') && is_gb2312_hanzi('国') && is_gb2312_hanzi('齄'));
    assert!(!is_gb2312_hanzi('說') && !is_gb2312_hanzi('國') && !is_gb2312_hanzi('a'));
}

#[test]
fn non_gb2312_single_characters_rank_after_gb2312_ones() {
    let input = MixedInput::bundled();
    for (code, simplified, traditional) in [
        ("shuo", "说", "說"),
        ("dui", "对", "對"),
        ("zhe", "这", "這"),
        ("guo", "过", "過"),
        ("guo", "国", "國"),
    ] {
        let list = texts(&input, code, true, false);
        assert!(
            position(&list, simplified) < position(&list, traditional),
            "{code}: {list:?}"
        );
        // Demoted, not removed: after every GB 2312 single character of the pinyin block, which
        // ends at the first word (later blocks such as Wubi prefixes are not reordered).
        let single = |t: &String| t.chars().count() == 1;
        let block_end = list.iter().position(|t| !single(t)).unwrap_or(list.len());
        let at = position(&list, traditional);
        assert!(at < block_end, "{code}: {list:?}");
        assert!(
            list[at..block_end]
                .iter()
                .all(|t| !t.chars().all(is_gb2312_hanzi)),
            "{code}: {list:?}"
        );
    }
}

#[test]
fn all_chars_and_traditional_output_keep_the_dictionary_order() {
    let input = MixedInput::bundled();
    // Without the filter 說 keeps its frequency rank (ahead of rarer GB 2312 characters).
    let all = texts(&input, "shuo", false, false);
    assert!(position(&all, "說") < position(&all, "朔"), "{all:?}");
    let common = texts(&input, "shuo", true, false);
    assert!(
        position(&common, "說") > position(&common, "朔"),
        "{common:?}"
    );
    // Traditional output converts every candidate; the order is not changed by GB 2312.
    let traditional = texts(&input, "dui", true, true);
    assert_eq!(traditional.first().map(String::as_str), Some("對"));
}
