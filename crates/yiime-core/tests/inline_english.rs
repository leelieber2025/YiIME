#![cfg(feature = "bundled")]
//! Shift tap mid-composition switches to inline English: simulates the Windows input box's key
//! handling.

use std::sync::OnceLock;
use std::time::{Duration, Instant};
use yiime_core::*;

fn input() -> &'static MixedInput {
    static INPUT: OnceLock<MixedInput> = OnceLock::new();
    INPUT.get_or_init(MixedInput::bundled)
}

/// Minimal input box: composer + draft + English-segment switch.
#[derive(Default)]
struct Session {
    composer: Composer,
    draft: Draft,
    english: bool,
    tap: ShiftTap,
}

const OPTIONS: ComposeOptions = ComposeOptions {
    page_size: 9,
    traditional: false,
    learn: false,
    fuzzy: false,
    pinyin_first: false,
    common_only: false,
    shuangpin: None,
};
const RULES: LetterRules = LetterRules {
    push_fifth: true,
    auto_commit4: true,
};

impl Session {
    fn shift_tap(&mut self) {
        let now = Instant::now();
        self.tap.key_down(0x10, false, now);
        assert!(self.tap.key_up(0x10, now + Duration::from_millis(80)));
        if !self.english {
            if let Some(letters) = literal_code(&self.composer.code) {
                self.draft.insert(&letters);
            }
            self.composer
                .set_code(input(), String::new(), &OPTIONS, &Learned::default());
        }
        self.english = !self.english;
    }
    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            if self.english && is_literal_in_english(ch) {
                self.draft.insert(&ch.to_string());
                continue;
            }
            match ch {
                ' ' => {
                    let (text, _) = self.composer.first_or_raw();
                    self.draft.insert(&text);
                    self.composer
                        .set_code(input(), String::new(), &OPTIONS, &Learned::default());
                }
                '\u{8}' if self.composer.code.is_empty() => self.draft.backspace(),
                _ => match classify_letter(ch, false, self.composer.is_composing()) {
                    Some(LetterAction::Compose(letter)) => {
                        for pick in self.composer.type_letter(
                            input(),
                            letter,
                            RULES,
                            &OPTIONS,
                            &Learned::default(),
                        ) {
                            self.draft.insert(&pick.text);
                        }
                    }
                    _ => self.draft.insert(&ch.to_string()),
                },
            }
        }
    }
}

#[test]
fn shift_tap_inserts_english_inside_one_chinese_sentence() {
    let mut session = Session::default();
    session.type_text("q ");
    assert_eq!(session.draft.text(), "我");
    session.shift_tap();
    session.type_text("iPhone 15");
    session.shift_tap();
    session.type_text("henhao ");
    assert_eq!(session.draft.text(), "我iPhone 15很好");
    // Spaces and punctuation are typed as is in English; backspace deletes across segments.
    session.shift_tap();
    session.type_text(" OK,");
    session.type_text("\u{8}");
    session.shift_tap();
    assert_eq!(session.draft.text(), "我iPhone 15很好 OK");
    session.type_text("\u{8}\u{8}\u{8}");
    assert_eq!(session.draft.text(), "我iPhone 15很好");
    session.type_text("\u{8}\u{8}");
    assert_eq!(session.draft.text(), "我iPhone 15");
}

#[test]
fn pending_code_is_committed_as_letters_when_switching() {
    // As in Microsoft Pinyin/Sogou/Rime: the unpicked code is committed as letters before English
    // starts.
    let mut session = Session::default();
    session.type_text("q ");
    session.type_text("zhong");
    assert!(session.composer.is_composing());
    session.shift_tap();
    assert!(!session.composer.is_composing());
    assert!(session.english);
    session.type_text("Shift");
    assert_eq!(session.draft.text(), "我zhongShift");
    // Back in Chinese, composition works as usual.
    session.shift_tap();
    session.type_text("wm ");
    assert_eq!(session.draft.text(), "我zhongShift仙");
}
