#![cfg(feature = "bundled")]
//! Golden test for pinyin tolerance (v13): typo correction, mixed full syllables and initials,
//! syllable segmentation with partial picks, whole sentences and fuzzy pinyin (off by default).
//! None of it changes Wubi codes or directly typed pinyin (Wubi first).

use std::sync::OnceLock;
use yiime_core::*;

const WORDS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/frost/InputTranslation-words.txt"
);

fn input() -> &'static MixedInput {
    static INPUT: OnceLock<MixedInput> = OnceLock::new();
    INPUT.get_or_init(|| {
        MixedInput::bundled().with_frequency_words(std::fs::read_to_string(WORDS).expect("大词库"))
    })
}

fn options(fuzzy: bool) -> ComposeOptions {
    ComposeOptions {
        page_size: 9,
        traditional: false,
        learn: false,
        fuzzy,
        pinyin_first: false,
        common_only: false,
    }
}

fn compose_with(code: &str, fuzzy: bool) -> Composer {
    let mut composer = Composer::default();
    composer.set_code(input(), code.into(), &options(fuzzy), &Learned::default());
    composer
}

fn compose(code: &str) -> Composer {
    compose_with(code, false)
}

fn first(code: &str) -> (String, Source) {
    let composer = compose(code);
    let c = composer.candidates.first().expect(code);
    (c.text.clone(), c.source)
}

fn position(composer: &Composer, text: &str) -> Option<usize> {
    composer.candidates.iter().position(|c| c.text == text)
}

#[test]
fn typo_correction_ranks_first_when_nothing_exact() {
    // Reported by a user: `zhonggou` (ou/uo swapped) used to have no candidates.
    let composer = compose("zhonggou");
    assert_eq!(composer.candidates[0].text, "中国");
    assert_eq!(composer.candidates[0].source, Source::Corrected);
    assert_eq!(composer.hint(&composer.candidates[0]), "(纠错 khlg)");
    for (typo, want) in [
        ("zhognguo", "中国"), // gn/ng
        ("nihoa", "你好"),    // ao/oa
        ("womne", "我们"),    // en/ne
        ("jintaintianqi", "今天天气"),
        ("zhnog", "中"),
        ("zhong'gou", "中国"), // Isolation notes are still corrected
    ] {
        assert_eq!(first(typo), (want.to_owned(), Source::Corrected), "{typo}");
    }
    // English words get no candidates; they stay as typed.
    // are.
    assert!(compose("hello").candidates.is_empty());
}

#[test]
fn partial_pick_keeps_remaining_syllables() {
    let mut composer = compose("zhonggou");
    // The whole sentence (中够) and leading-syllable words (zhong → 中/种/重…, then gou) come after typo
    // correction.
    let sentence = position(&composer, "中够").expect("整句");
    let zhong = position(&composer, "中").expect("前段字");
    assert!(sentence > 0 && zhong > sentence);
    assert_eq!(composer.candidates[zhong].source, Source::Partial);
    assert_eq!(composer.candidates[zhong].rest, "gou");
    let pick = composer.candidate_at(zhong).expect("在第一页");
    assert_eq!(
        pick,
        Pick {
            code: "zhong".into(),
            text: "中".into(),
            rest: "gou".into()
        }
    );
    composer.set_code(input(), pick.rest, &options(false), &Learned::default());
    assert_eq!(composer.code, "gou");
    assert_eq!(composer.candidates[0].text, "够");
    // With a separator, the leading word of `zhong'gou` leaves `gou`.
    let composer = compose("zhong'gou");
    let zhong = position(&composer, "中").unwrap();
    assert_eq!(composer.candidate_at(zhong).unwrap().code, "zhong");
}

#[test]
fn mixed_full_and_abbreviated_pinyin() {
    for (code, want) in [
        ("zhongg", "中国"),
        ("zhongguor", "中国人"),
        ("beijingdx", "北京大学"),
        ("zgren", "中国人"),
    ] {
        assert_eq!(first(code).0, want, "{code}");
    }
    // Pure initials `zhg` rank as in v11 (by frequency).
    assert_eq!(position(&compose("zhg"), "中国"), Some(1));
}

#[test]
fn multi_syllable_inputs_give_sentences() {
    for (code, want) in [
        ("woaini", "我爱你"),
        ("jintiantianqi", "今天天气"),
        ("zhongguoren", "中国人"),
        ("zhongwenshurufa", "中文输入法"),
        ("woxiangqubeijing", "我想去北京"),
        ("xi'an", "西安"),
    ] {
        assert_eq!(first(code).0, want, "{code}");
    }
}

#[test]
fn wubi_codes_stay_first() {
    assert_eq!(first("gngu"), ("一心一意".to_owned(), Source::Wubi));
    // `xain` is a Wubi code (and must not be treated as a typo of xian).
    assert_eq!(first("xain").1, Source::Wubi);
    assert!(
        compose("xain")
            .candidates
            .iter()
            .all(|c| c.source != Source::Corrected)
    );
    let yxyy = compose("yxyy");
    assert_eq!(yxyy.candidates[6].text, "一心一意");
}

#[test]
fn fuzzy_pinyin_is_off_by_default() {
    assert!(!Flag::FuzzyPinyin.default_on());
    assert_ne!(first("zongguo").0, "中国");
    let fuzzy = compose_with("zongguo", true);
    assert_eq!(fuzzy.candidates[0].text, "中国");
    let shanghai = compose_with("sanghai", true);
    assert!(position(&shanghai, "上海").is_some_and(|i| i < 9));
    assert!(position(&compose("sanghai"), "上海").is_none());
}

#[test]
fn fifth_letter_typo_is_not_cleared() {
    // Fifth-letter/empty-code rule: `zhno` is not a Wubi code, and `zhnog` may still be pinyin
    // after a swap, so it is not cleared.
    let learned = Learned::default();
    let rules = LetterRules {
        push_fifth: true,
        auto_commit4: true,
    };
    let mut composer = Composer::default();
    for letter in "zhnog".chars() {
        assert!(
            composer
                .type_letter(input(), letter, rules, &options(false), &learned)
                .is_empty()
        );
    }
    assert_eq!(composer.code, "zhnog");
    assert_eq!(composer.candidates[0].text, "中");
    // Wubi is unchanged: gngu then w commits 一心一意.
    let mut composer = Composer::default();
    let mut picks = Vec::new();
    for letter in "gnguw".chars() {
        picks.extend(composer.type_letter(input(), letter, rules, &options(false), &learned));
    }
    assert_eq!(picks[0].text, "一心一意");
    assert_eq!(composer.code, "w");
}
