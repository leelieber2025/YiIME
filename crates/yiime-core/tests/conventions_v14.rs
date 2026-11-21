#![cfg(feature = "bundled")]
//! Common input method conventions added in v14 (Microsoft Pinyin, Sogou, Rime): word
//! determination, comma and period page turning, deletion of learning records, automatic pairing of
//! brackets and quotes, small keyboard numbers, and default switches.

use yiime_core::*;

#[test]
fn bracket_picks_first_or_last_char_of_word() {
    let input = MixedInput::bundled();
    let mut composer = Composer::default();
    let options = ComposeOptions {
        page_size: 9,
        traditional: false,
        learn: false,
        fuzzy: false,
    };
    composer.set_code(&input, "zgr".into(), &options, &Learned::default());
    let first = &composer.candidates[0].text;
    assert_eq!(first, "中国人");
    assert_eq!(char_of_word(first, false).as_deref(), Some("中"));
    assert_eq!(char_of_word(first, true).as_deref(), Some("人"));
    // There is no definite word for a word: [ ] is still treated as punctuation.
    assert_eq!(char_of_word("仙", false), None);
    assert_eq!(char_of_word("", true), None);
}

#[test]
fn paging_keys() {
    assert_eq!(paging_direction('=', false), Some(true));
    assert_eq!(paging_direction('-', false), Some(false));
    // Commas and periods are turned off by default (Wubi uses them to top the screen).
    assert!(!Flag::CommaPeriodPaging.default_on());
    assert_eq!(paging_direction('.', false), None);
    assert_eq!(paging_direction(',', false), None);
    assert_eq!(paging_direction('.', true), Some(true));
    assert_eq!(paging_direction(',', true), Some(false));
}

#[test]
fn auto_pair_table_and_defaults() {
    assert!(!Flag::AutoPair.default_on());
    assert!(Flag::BracketCharPick.default_on());
    for (open, close) in [
        ("“", "”"),
        ("‘", "’"),
        ("（", "）"),
        ("《", "》"),
        ("【", "】"),
    ] {
        assert_eq!(closing_pair(open), Some(close));
        assert!(is_closing(close));
        assert!(!is_closing(open));
    }
    assert_eq!(closing_pair("，"), None);
    // Quotation marks alternate and match: the next quote after the opening quote is the closing
    // quote (it is only skipped if it is exactly after the cursor).
    let mut quotes = Quotes::default();
    assert_eq!(quotes.next('"'), Some("“"));
    assert_eq!(quotes.next('"'), Some("”"));
}

#[test]
fn numpad_digits_are_recognised() {
    for vk in 0x60..=0x69 {
        assert!(is_numpad_digit(vk));
    }
    assert!(!is_numpad_digit(0x31));
}
