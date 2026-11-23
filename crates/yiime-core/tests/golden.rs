#![allow(clippy::unwrap_used)] // Tests may unwrap.
#![cfg(feature = "bundled")]
//! Golden test: engine output must match v11 (the tested Windows release) line by line.
//!
//! golden/candidates-v11.tsv was generated from v11 (git tag v11) for 40 codes at 9/5/1 candidates
//! per page. Columns: page size, code, index, text, source, full code, Wubi hint; each code ends
//! with a `count` line (Wubi candidate count, whether pinyin can continue).

use std::sync::OnceLock;
use yiime_core::*;

const WORDS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/frost/YiIME-words.txt"
);

fn input() -> &'static MixedInput {
    static INPUT: OnceLock<MixedInput> = OnceLock::new();
    INPUT.get_or_init(|| {
        MixedInput::bundled().with_frequency_words(std::fs::read_to_string(WORDS).expect("大词库"))
    })
}

fn options(page_size: usize) -> ComposeOptions {
    ComposeOptions {
        page_size,
        traditional: false,
        learn: false,
        fuzzy: false,
        pinyin_first: false,
        common_only: false,
        shuangpin: None,
    }
}

fn compose(code: &str, page_size: usize) -> Composer {
    let mut composer = Composer::default();
    composer.set_code(
        input(),
        code.into(),
        &options(page_size),
        &Learned::default(),
    );
    composer
}

fn texts(composer: &Composer) -> Vec<&str> {
    composer
        .page_items()
        .iter()
        .map(|c| c.text.as_str())
        .collect()
}

const V13_FILLED: [&str; 1] = ["zhongg"];

#[test]
fn candidates_identical_to_v11() {
    let golden = include_str!("golden/candidates-v11.tsv");
    let mut codes: Vec<(usize, &str)> = Vec::new();
    for line in golden.lines() {
        let mut fields = line.split('\t');
        let size: usize = fields.next().unwrap().parse().unwrap();
        let code = fields.next().unwrap();
        if codes.last() != Some(&(size, code)) {
            codes.push((size, code));
        }
    }
    assert_eq!(codes.len(), 120);
    let input = input();
    let mut out = String::new();
    for (size, code) in codes {
        for (i, c) in input
            .candidates_with_page(code, size * CANDIDATE_PAGES, size)
            .iter()
            .enumerate()
        {
            out.push_str(&format!(
                "{size}\t{code}\t{i}\t{}\t{:?}\t{}\t{}\n",
                c.text,
                c.source,
                c.complete,
                input.wubi_hint(&c.text).unwrap_or_default()
            ));
        }
        out.push_str(&format!(
            "{size}\t{code}\tcount\t{}\t{}\n",
            input.wubi_count(code),
            input.pinyin_continues(code)
        ));
    }
    // Codes that had no candidates in v11 (neither Wubi code nor pinyin word) now get full pinyin,
    // initials, mixed, typo-corrected or segmented candidates since v13. Every other code must
    // match v11 line by line.
    let changed = |line: &str| V13_FILLED.contains(&line.split('\t').nth(1).unwrap_or(""));
    let want: Vec<&str> = golden.lines().filter(|l| !changed(l)).collect();
    let got: Vec<&str> = out.lines().filter(|l| !changed(l)).collect();
    if std::env::var_os("GOLDEN_DUMP").is_some() {
        std::fs::write("/tmp/golden-now.tsv", &out).unwrap();
    }
    for (line, (want, got)) in want.iter().zip(&got).enumerate() {
        assert_eq!(got, want, "第 {} 行", line + 1);
    }
    assert_eq!(got.len(), want.len());
    for code in V13_FILLED {
        let before = golden
            .lines()
            .filter(|l| l.split('\t').nth(1) == Some(code) && l.split('\t').nth(2) != Some("count"))
            .count();
        assert_eq!(before, 0, "{code} 在 v11 下本来就有候选");
    }
}

#[test]
fn named_orderings() {
    // Exact Wubi → full pinyin → initials (by frequency).
    assert_eq!(texts(&compose("gngu", 9))[0], "一心一意");
    let yxyy = compose("yxyy", 9);
    assert_eq!(yxyy.candidates[6].text, "一心一意"); // No. 7: The first 6 are Wubi
    assert_eq!(yxyy.candidates[6].source, Source::Abbreviation);
    assert_eq!(yxyy.hint(&yxyy.candidates[6]), "(gngu)");
    assert_eq!(yxyy.candidates[0].source, Source::Wubi);
    assert_eq!(
        texts(&compose("zgr", 9))[..4],
        ["中国人", "这个人", "整个人", "张国荣"]
    );
    assert_eq!(texts(&compose("zhgr", 9))[0], "中国人");
    assert_eq!(
        texts(&compose("wm", 9)),
        [
            "仙", "我们", "我妈", "文明", "完美", "外面", "外卖", "外贸", "我没"
        ]
    );
    let wm = compose("wm", 9);
    assert_eq!(wm.hint(&wm.candidates[0]), "h"); // 仙 is wmhh: remaining keys after wm
    assert_eq!(wm.hint(&wm.candidates[1]), "(trwu)");
    assert_eq!(texts(&compose("dsj", 9))[..3], ["碴", "大数据", "电视剧"]);
    assert_eq!(texts(&compose("yixinyiyi", 9))[0], "一心一意");
    // Wildcard `z` candidates show only the full code.
    let aaz = compose("aaz", 9);
    assert_eq!(texts(&aaz)[..3], ["工", "式", "戒"]);
    assert_eq!(aaz.candidates[1].source, Source::Wildcard);
    assert_eq!(aaz.hint(&aaz.candidates[1]), "(aad)");
    assert_eq!(texts(&compose("xi'an", 9))[0], "西安");
}

#[test]
fn page_size_paging_and_digits() {
    let mut wm = compose("wm", 5);
    assert_eq!(wm.page_items().len(), 5);
    assert_eq!(wm.candidate_at(6), None); // 7 does nothing with 5 per page.
    wm.turn_page(true);
    assert_eq!(wm.page_label(), "‹ 2/10 ›");
    assert_eq!(wm.candidate_at(1).unwrap().text, "外卖");
    wm.turn_page(false);
    wm.turn_page(false);
    assert_eq!(wm.candidate_at(0).unwrap().text, "仙");
    let mut yxyy = compose("yxyy", 5);
    yxyy.turn_page(true);
    assert_eq!(yxyy.candidate_at(1).unwrap().text, "一心一意");
    let one = compose("wm", 1);
    assert_eq!(texts(&one), ["仙"]);
    assert_eq!(one.candidate_at(1), None);
    let mut last = compose("wm", 9);
    for _ in 0..20 {
        last.turn_page(true);
    }
    assert_eq!(last.page, last.page_count() - 1);
    assert_eq!(last.candidates.len(), 90);
}

#[test]
fn traditional_output_keeps_simplified_hints() {
    let mut composer = Composer::default();
    let traditional = ComposeOptions {
        traditional: true,
        ..options(9)
    };
    composer.set_code(input(), "wm".into(), &traditional, &Learned::default());
    assert_eq!(texts(&composer)[..4], ["仙", "我們", "我媽", "文明"]);
    assert_eq!(composer.hint(&composer.candidates[1]), "(trwu)");
    composer.set_code(input(), "zgr".into(), &traditional, &Learned::default());
    assert_eq!(composer.candidates[0].text, "中國人");
    assert_eq!(to_traditional("头发后来发展了"), "頭髮後來發展了");
}

#[test]
fn letter_rules_and_shift_letter_in_draft() {
    let input = input();
    let learned = Learned::default();
    let rules = LetterRules {
        push_fifth: true,
        auto_commit4: true,
    };
    let opts = options(9);
    let mut composer = Composer::default();
    let mut draft = Draft::default();
    let type_text = |composer: &mut Composer, draft: &mut Draft, text: &str| {
        for ch in text.chars() {
            match classify_letter(ch, false, composer.is_composing()).unwrap() {
                LetterAction::Compose(letter) => {
                    for pick in composer.type_letter(input, letter, rules, &opts, &learned) {
                        draft.insert(&pick.text);
                    }
                }
                LetterAction::Raw { flush_first } => {
                    if flush_first {
                        draft.insert(&composer.first_or_raw().0);
                        composer.set_code(input, String::new(), &opts, &learned);
                    }
                    draft.insert(&ch.to_string());
                }
            }
        }
    };
    // Unique four-letter code commits itself: bnhn has only 也.
    type_text(&mut composer, &mut draft, "bnhn");
    assert_eq!(draft.text(), "也");
    assert!(!composer.is_composing());
    // Fifth letter: gngu has two candidates (no auto-commit); typing w commits the first and starts
    // a new code.
    type_text(&mut composer, &mut draft, "gnguw");
    assert_eq!(draft.text(), "也一心一意");
    assert_eq!(composer.code, "w");
    // Shift+letter: commit the first candidate (wm → 仙), then type the capital letter.
    type_text(&mut composer, &mut draft, "mA");
    assert_eq!(draft.text(), "也一心一意仙A");
    assert!(!composer.is_composing());
    // Without candidates, Shift+letter commits the raw code.
    type_text(&mut composer, &mut draft, "qwerB");
    assert_eq!(draft.text(), "也一心一意仙AqwerB");
    // Caps Lock: no composition, and nothing to flush without the input box.
    assert_eq!(
        classify_letter('a', true, false),
        Some(LetterAction::Raw { flush_first: false })
    );
    assert_eq!(classify_letter('1', false, true), None);
    // Empty code: qwer has no candidates and is dropped when the next letter is typed.
    let mut empty = Composer::default();
    for letter in "qwer".chars() {
        assert!(
            empty
                .type_letter(input, letter, rules, &opts, &learned)
                .is_empty()
        );
    }
    assert!(
        empty
            .type_letter(input, 'w', rules, &opts, &learned)
            .is_empty()
    );
    assert_eq!(empty.code, "w");
    // With the rules off, four letters do not auto-commit and a fifth keeps accumulating.
    let off = LetterRules {
        push_fifth: false,
        auto_commit4: false,
    };
    let mut plain = Composer::default();
    for letter in "bnhnw".chars() {
        assert!(
            plain
                .type_letter(input, letter, off, &opts, &learned)
                .is_empty()
        );
    }
    assert_eq!(plain.code, "bnhnw");
}

#[test]
fn selection_keys() {
    let input = input();
    assert_eq!(selection_index(input, "wm", ';'), Some(1));
    assert_eq!(selection_index(input, "wm", '\''), Some(2));
    // `xi` may be pinyin, so `'` is a syllable separator.
    assert_eq!(selection_index(input, "xi", '\''), None);
    assert_eq!(selection_index(input, "xi'a", '\''), None);
    assert_eq!(selection_index(input, "wm", ','), None);
}

#[test]
fn learning_reorders_like_v11() {
    let mut learned = Learned::default();
    learned.record("wm", "外卖");
    learned.record("wm", "外卖");
    learned.record("wm", "仙");
    let mut composer = Composer::default();
    let learn = ComposeOptions {
        learn: true,
        ..options(5)
    };
    composer.set_code(input(), "wm".into(), &learn, &learned);
    assert_eq!(texts(&composer), ["外卖", "仙", "我们", "我妈", "文明"]);
    composer.set_code(input(), "wm".into(), &options(5), &learned);
    assert_eq!(texts(&composer)[0], "仙"); // No adjustment when learning is off
}

#[test]
fn pinyin_first_ranks_pinyin_and_disables_wubi_automatic_commits() {
    let input = MixedInput::from_rime(
        "...\n二\tfan\t9\n甲\taaaa\t9\n乙\tbbbb\t9\n",
        "...\n翻\tfan\t9\n",
    );
    let options = ComposeOptions {
        pinyin_first: true,
        common_only: false,
        shuangpin: None,
        ..options(9)
    };
    let learned = Learned::default();
    let mut composer = Composer::default();
    composer.set_code(&input, "fan".into(), &options, &learned);
    assert_eq!(composer.candidates[0].text, "翻");
    assert_eq!(composer.candidates[0].source, Source::Pinyin);
    assert_eq!(composer.candidates[1].text, "二");

    composer.set_code(&input, String::new(), &options, &learned);
    let rules = LetterRules {
        push_fifth: true,
        auto_commit4: true,
    };
    for letter in "aaaaa".chars() {
        assert!(
            composer
                .type_letter(&input, letter, rules, &options, &learned)
                .is_empty()
        );
    }
    assert_eq!(composer.code, "aaaaa");
}
