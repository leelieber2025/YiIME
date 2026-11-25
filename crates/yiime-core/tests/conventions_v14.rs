#![allow(clippy::unwrap_used)] // Tests may unwrap.
#![cfg(feature = "bundled")]
//! Common input-method conventions (Microsoft Pinyin, Sogou, Rime): picking a character from a
//! word, comma/period paging, forgetting learned picks, auto-closing brackets and quotes, numpad
//! digits, and default switches.

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
        pinyin_first: false,
        common_only: false,
        shuangpin: None,
    };
    composer.set_code(&input, "zgr".into(), &options, &Learned::default());
    let first = &composer.candidates[0].text;
    assert_eq!(first, "中国人");
    assert_eq!(char_of_word(first, false).as_deref(), Some("中"));
    assert_eq!(char_of_word(first, true).as_deref(), Some("人"));
    // A single character has no character to pick: `[` `]` stay punctuation.
    assert_eq!(char_of_word("仙", false), None);
    assert_eq!(char_of_word("", true), None);
}

#[test]
fn paging_keys() {
    assert_eq!(paging_direction('=', false), Some(true));
    assert_eq!(paging_direction('-', false), Some(false));
    // Comma/period paging is off by default (in Wubi they commit the first candidate).
    assert!(!Flag::CommaPeriodPaging.default_on());
    assert_eq!(paging_direction('.', false), None);
    assert_eq!(paging_direction(',', false), None);
    assert_eq!(paging_direction('.', true), Some(true));
    assert_eq!(paging_direction(',', true), Some(false));
    // TSF key sink uses virtual keys (US layout). PageUp/PageDown always page; `,` `.` follow the switch.
    assert_eq!(paging_direction_vk(0xBB, false, false), Some(true));
    assert_eq!(paging_direction_vk(0xBD, false, false), Some(false));
    assert_eq!(paging_direction_vk(0x22, false, false), Some(true));
    assert_eq!(paging_direction_vk(0x21, false, false), Some(false));
    assert_eq!(paging_direction_vk(0xBE, false, false), None);
    assert_eq!(paging_direction_vk(0xBC, false, false), None);
    assert_eq!(paging_direction_vk(0xBE, false, true), Some(true));
    assert_eq!(paging_direction_vk(0xBC, false, true), Some(false));
    assert_eq!(paging_direction_vk(0xBB, true, true), None);
    assert_eq!(paging_direction_vk(0xBE, true, true), None);
    assert_eq!(paging_direction_vk(0x21, true, true), None);
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
    // Quotes alternate: after an opening quote comes the closing one (skipped over only when it is
    // right before the cursor).
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
