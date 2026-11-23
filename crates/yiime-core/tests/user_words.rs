#![allow(clippy::unwrap_used)] // Tests may unwrap.
//! User words: imported pinyin words lead their code in either order, and a running service
//! picks up an import without restarting.

use std::sync::Arc;
use yiime_core::{
    ComposeOptions, Composer, Learned, MixedInput, Source, UserWordsReloader, merge_user_words,
    parse_text_export,
};

fn first(input: &MixedInput, code: &str, pinyin_first: bool) -> (String, Source) {
    let candidate = input
        .candidates_filtered(code, 20, 9, false, pinyin_first, true, true)
        .into_iter()
        .next()
        .unwrap();
    (candidate.text, candidate.source)
}

#[test]
fn imported_pinyin_words_rank_first_in_both_orders() {
    let input = MixedInput::bundled();
    // Built-in words come first before the import.
    let before = first(&input, "shiyou", true).0;
    assert_ne!(before, "诗友");
    let (contents, added) = merge_user_words("", &parse_text_export("'shi'you 诗友\n"));
    assert_eq!(added, 1);
    let input = input.with_user_words(&contents);
    for pinyin_first in [true, false] {
        assert_eq!(
            first(&input, "shiyou", pinyin_first),
            ("诗友".to_owned(), Source::Pinyin),
            "pinyin_first = {pinyin_first}"
        );
    }
    // The built-in word is still there, right after.
    let all = input.candidates_filtered("shiyou", 20, 9, false, true, true, true);
    assert!(all.iter().skip(1).any(|candidate| candidate.text == before));
}

#[test]
fn user_wubi_words_rank_first_in_both_orders() {
    // `wqvb` is a Wubi code (你好); a user word under it leads in both orders.
    let input = MixedInput::bundled().with_user_words("拟号 wqvb\n");
    for pinyin_first in [true, false] {
        assert_eq!(
            first(&input, "wqvb", pinyin_first),
            ("拟号".to_owned(), Source::Wubi)
        );
    }
}

#[test]
fn import_reaches_a_running_service_without_restart() {
    let dir = std::env::temp_dir().join(format!("yiime-user-words-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("user_words.txt");
    std::fs::write(&path, "# 用户词库\n").unwrap();

    // Service start: the dictionary and a second snapshot (as with the large lexicon) share the
    // loaded user words.
    let (mut reloader, contents) = UserWordsReloader::load(Some(path.clone()));
    let input = Arc::new(MixedInput::bundled().with_user_words(&contents.unwrap()));
    let held = input.clone();
    assert_ne!(first(&input, "shiyou", true).0, "诗友");

    // The settings window (possibly another process) imports into user_words.txt.
    let existing = std::fs::read_to_string(&path).unwrap();
    let (merged, _) = merge_user_words(&existing, &parse_text_export("'shi'you 诗友\n"));
    assert!(yiime_core::write_atomically(&path, merged.as_bytes()));

    // Next session or code in the running service: reload in place.
    let changed = reloader.check().unwrap();
    input.set_user_words(&changed);
    assert_eq!(first(&held, "shiyou", true).0, "诗友");
    assert_eq!(first(&held, "shiyou", false).0, "诗友");
    assert_eq!(reloader.check(), None);

    // Removing the word from the file (e.g. in Notepad) takes it out again.
    std::fs::write(&path, "# 用户词库\n").unwrap();
    input.set_user_words(&reloader.check().unwrap());
    assert_ne!(first(&held, "shiyou", true).0, "诗友");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn learning_does_not_pass_user_words() {
    let bare = MixedInput::bundled();
    let pinyin = first(&bare, "shiyou", true).0;
    let wubi = first(&bare, "wqvb", false).0;
    let input = bare.with_user_words("诗友 shiyou\n拟号 wqvb\n");
    let mut learned = Learned::default();
    for _ in 0..3 {
        learned.record("shiyou", &pinyin);
        learned.record("wqvb", &wubi);
    }
    let options = ComposeOptions {
        page_size: 9,
        traditional: false,
        learn: true,
        fuzzy: false,
        pinyin_first: false,
        common_only: true,
        shuangpin: None,
    };
    let mut composer = Composer::default();
    for pinyin_first in [false, true] {
        let options = ComposeOptions {
            pinyin_first,
            ..options
        };
        composer.set_code(&input, "shiyou".into(), &options, &learned);
        assert_eq!(
            composer.candidates[0].text, "诗友",
            "pinyin_first = {pinyin_first}"
        );
        assert!(
            composer
                .candidates
                .iter()
                .any(|candidate| candidate.text == pinyin)
        );
        composer.set_code(&input, "wqvb".into(), &options, &learned);
        assert_eq!(
            composer.candidates[0].text, "拟号",
            "pinyin_first = {pinyin_first}"
        );
        assert!(
            composer
                .candidates
                .iter()
                .any(|candidate| candidate.text == wubi)
        );
    }
}
