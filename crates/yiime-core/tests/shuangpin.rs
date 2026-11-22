//! Shuangpin through the composer: keys stay as typed, candidates come from full pinyin only.
#![allow(clippy::unwrap_used)] // Tests may unwrap.

use yiime_core::{ComposeOptions, Composer, Learned, LetterRules, MixedInput, Shuangpin};

fn options(scheme: Shuangpin) -> ComposeOptions {
    ComposeOptions {
        page_size: 9,
        traditional: false,
        learn: false,
        fuzzy: false,
        pinyin_first: false,
        common_only: true,
        shuangpin: Some(scheme),
    }
}

fn first(input: &MixedInput, scheme: Shuangpin, code: &str) -> Option<String> {
    let mut composer = Composer::default();
    composer.set_code(
        input,
        code.to_owned(),
        &options(scheme),
        &Learned::default(),
    );
    composer.candidate_at(0).map(|pick| pick.text)
}

#[test]
fn layouts_reach_the_same_words() {
    let input = MixedInput::bundled();
    assert_eq!(
        first(&input, Shuangpin::Xiaohe, "nihc").as_deref(),
        Some("你好")
    );
    assert_eq!(
        first(&input, Shuangpin::Microsoft, "nihk").as_deref(),
        Some("你好")
    );
    assert_eq!(
        first(&input, Shuangpin::Ziranma, "nihk").as_deref(),
        Some("你好")
    );
    assert_eq!(
        first(&input, Shuangpin::Xiaohe, "vsgo").as_deref(),
        Some("中国")
    );
    assert_eq!(
        first(&input, Shuangpin::Microsoft, "b;").as_deref(),
        Some("并")
    );
    // Never read as Wubi: `wqvb` is 你好 in Wubi but not valid 小鹤双拼 here.
    assert_ne!(
        first(&input, Shuangpin::Xiaohe, "wqvb").as_deref(),
        Some("你好")
    );
    assert_eq!(first(&input, Shuangpin::Xiaohe, "bq"), None);
}

#[test]
fn odd_keys_and_partial_picks_keep_typed_keys() {
    let input = MixedInput::bundled();
    let learned = Learned::default();
    let options = options(Shuangpin::Xiaohe);
    let mut composer = Composer::default();
    // A trailing initial key: candidates of the complete syllables, leaving that key.
    composer.set_code(&input, "nihcu".into(), &options, &learned);
    let pick = composer.candidate_at(0).unwrap();
    assert_eq!(
        (pick.text.as_str(), pick.code.as_str(), pick.rest.as_str()),
        ("你好", "nihc", "u")
    );
    // Wubi auto-commit and fifth-letter rules are off: typing goes on as pinyin.
    let rules = LetterRules {
        push_fifth: true,
        auto_commit4: true,
    };
    let mut composer = Composer::default();
    for key in "uuru".chars() {
        assert!(
            composer
                .type_letter(&input, key, rules, &options, &learned)
                .is_empty()
        );
    }
    assert_eq!(composer.code, "uuru");
    assert_eq!(composer.candidate_at(0).unwrap().text, "输入");
}

#[test]
fn imported_pinyin_words_come_first_in_shuangpin() {
    let words = yiime_core::parse_text_export("'ni'hao 妮好\n");
    let (contents, _) = yiime_core::merge_user_words("", &words);
    let input = MixedInput::bundled().with_user_words(&contents);
    assert_eq!(
        first(&input, Shuangpin::Xiaohe, "nihc").as_deref(),
        Some("妮好")
    );
}
