//! Candidate engine: composition state (code, candidates, paging, Wubi hints) and Microsoft Wubi
//! key rules (fifth letter commits the first candidate, a unique four-letter code commits itself,
//! `;` `'` pick the 2nd/3rd candidate, Shift/Caps letters go through as typed). The platform layer
//! only feeds keys, writes committed text into the draft and redraws.

use crate::dict::{Candidate, MixedInput, Source};
use crate::learn::Learned;
use crate::shuangpin::Shuangpin;
use crate::text::to_traditional;
use std::collections::HashMap;

/// Each query fetches `page_size × CANDIDATE_PAGES` candidates.
pub const CANDIDATE_PAGES: usize = 10;
/// Longest code; extra letters are ignored.
use crate::MAX_CODE_LEN;

#[derive(Debug, Clone, Copy)]
pub struct ComposeOptions {
    pub page_size: usize,
    /// Output Traditional Chinese (Wubi hints still use the Simplified text).
    pub traditional: bool,
    pub learn: bool,
    /// Fuzzy pinyin (z/zh c/ch s/sh n/l an/ang en/eng in/ing); off by default.
    pub fuzzy: bool,
    /// Rank exact pinyin before exact Wubi and disable Wubi auto-commit rules.
    pub pinyin_first: bool,
    /// Hide rare characters and low-frequency initials matches (on by default in the IME).
    pub common_only: bool,
    /// Shuangpin layout: the code is double-pinyin keys, looked up as full pinyin only (no
    /// Wubi, no Wubi auto-commit rules). None = Wubi + full pinyin.
    pub shuangpin: Option<Shuangpin>,
}

#[derive(Debug, Clone, Copy)]
pub struct LetterRules {
    pub push_fifth: bool,
    pub auto_commit4: bool,
}

/// A picked candidate; the platform layer writes it into the draft and records it for learning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// Code consumed by this candidate (only the leading syllables for a partial match).
    pub code: String,
    pub text: String,
    /// Code still unconsumed: after committing `text`, keep composing it (Microsoft Pinyin picks
    /// syllable by syllable).
    pub rest: String,
}

/// One step of committing a whole code ([`Composer::flush_all`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flushed {
    /// The first candidate (record it for learning like a normal pick).
    Pick(Pick),
    /// Letters with no candidate, committed as typed.
    Raw(String),
}

/// Composition state of the input box.
#[derive(Debug, Default)]
pub struct Composer {
    pub code: String,
    pub candidates: Vec<Candidate>,
    pub page: usize,
    /// Candidates per page (re-read from settings on every refresh).
    pub page_size: usize,
    /// Display text → hint: remaining Wubi keys for a Wubi match, or a full-code `(trwu)` for others.
    pub hints: HashMap<String, String>,
}

impl Composer {
    /// Sets a new code (empty clears), re-queries and returns to the first page.
    pub fn set_code(
        &mut self,
        input: &MixedInput,
        code: String,
        options: &ComposeOptions,
        learned: &Learned,
    ) {
        let size = options.page_size;
        self.page_size = size;
        // Shuangpin with an odd key count ends in a bare initial: show the candidates of the
        // complete syllables, each leaving that key to compose on.
        let pending = match options.shuangpin {
            Some(_) if code.replace('\'', "").len() % 2 == 1 => code.chars().last(),
            _ => None,
        };
        let complete_keys = match options.shuangpin {
            Some(_) => &code[..code.len() - pending.map_or(0, char::len_utf8)],
            None => code.as_str(),
        };
        let lookup = match options.shuangpin {
            _ if code.is_empty() => None,
            Some(scheme) => {
                if complete_keys.replace('\'', "").is_empty() {
                    None
                } else {
                    scheme.expand(complete_keys)
                }
            }
            None => Some(code.clone()),
        };
        let mut candidates = match &lookup {
            None => Vec::new(),
            Some(lookup) => input.candidates_filtered(
                lookup,
                size * CANDIDATE_PAGES,
                size,
                options.fuzzy,
                options.pinyin_first || options.shuangpin.is_some(),
                options.common_only,
                // Traditional output converts every candidate anyway.
                options.common_only && !options.traditional,
            ),
        };
        // A partial match leaves full pinyin unconsumed; keep the matching typed keys instead.
        if let Some(scheme) = options.shuangpin {
            candidates.retain_mut(|candidate| {
                if candidate.rest.is_empty() {
                    return true;
                }
                match scheme.rest_keys(complete_keys, &candidate.rest) {
                    Some(keys) => {
                        candidate.rest = keys;
                        true
                    }
                    None => false,
                }
            });
            if let Some(key) = pending {
                for candidate in &mut candidates {
                    candidate.rest.push(key);
                    candidate.source = Source::Partial;
                }
            }
        }
        self.hints.clear();
        for candidate in &mut candidates {
            // Compute the hint from the Simplified text, then convert the display string.
            let hint = input.candidate_hint(&code, candidate);
            if options.traditional {
                candidate.text = to_traditional(&candidate.text);
            }
            if !hint.is_empty() {
                self.hints.insert(candidate.text.clone(), hint);
            }
        }
        if options.traditional {
            let mut seen = std::collections::HashSet::new();
            candidates.retain(|item| seen.insert((item.text.clone(), item.rest.clone())));
        }
        if options.learn {
            if options.pinyin_first {
                let exact_pinyin = candidates
                    .iter()
                    .take_while(|candidate| {
                        candidate.source == Source::Pinyin && candidate.complete
                    })
                    .count();
                let (pinyin, others) = candidates.split_at_mut(exact_pinyin);
                learned.reorder(&code, pinyin);
                learned.reorder(&code, others);
            } else {
                learned.reorder(&code, &mut candidates);
            }
        }
        self.candidates = candidates;
        self.code = code;
        self.page = 0;
    }
    pub fn is_composing(&self) -> bool {
        !self.code.is_empty()
    }
    pub fn page_count(&self) -> usize {
        self.candidates.len().div_ceil(self.page_size.max(1)).max(1)
    }
    pub fn page_items(&self) -> &[Candidate] {
        let size = self.page_size.max(1);
        let start = (self.page * size).min(self.candidates.len());
        let end = (start + size).min(self.candidates.len());
        &self.candidates[start..end]
    }
    pub fn page_label(&self) -> String {
        format!("‹ {}/{} ›", self.page + 1, self.page_count())
    }
    /// Wubi hint shown after a candidate (empty if none).
    pub fn hint(&self, candidate: &Candidate) -> String {
        self.hints.get(&candidate.text).cloned().unwrap_or_default()
    }
    pub fn turn_page(&mut self, forward: bool) {
        let pages = self.page_count();
        self.page = if forward {
            (self.page + 1).min(pages - 1)
        } else {
            self.page.saturating_sub(1)
        };
    }
    /// The `index`th candidate on this page; None past the end (e.g. 7 with 5 per page).
    pub fn candidate_at(&self, index: usize) -> Option<Pick> {
        self.page_items().get(index).map(|candidate| {
            let code = if candidate.rest.is_empty() {
                self.code.clone()
            } else {
                let letters = self.code.replace('\'', "");
                letters[..letters.len().saturating_sub(candidate.rest.len())].to_owned()
            };
            Pick {
                code,
                text: candidate.text.clone(),
                rest: candidate.rest.clone(),
            }
        })
    }
    /// Punctuation, Shift+letter, Enter-to-commit or ending the session: commits the whole code by
    /// picking the first candidate segment by segment (`zhongguox` → 中国, then the first
    /// candidate of `x`) until nothing is left, the raw letters when a segment has no candidate.
    /// Returns the steps in order; the code is empty afterwards. Both platforms use this so a
    /// leftover segment is never dropped and text that follows (punctuation) lands after it.
    pub fn flush_all(
        &mut self,
        input: &MixedInput,
        options: &ComposeOptions,
        learned: &Learned,
    ) -> Vec<Flushed> {
        let mut steps = Vec::new();
        // Every step consumes at least one letter; the bound is only a safety net.
        for _ in 0..=MAX_CODE_LEN {
            if self.code.is_empty() {
                break;
            }
            match self.candidate_at(0) {
                Some(pick) if pick.rest.len() < self.code.replace('\'', "").len() => {
                    self.set_code(input, pick.rest.clone(), options, learned);
                    steps.push(Flushed::Pick(pick));
                }
                _ => {
                    steps.push(Flushed::Raw(std::mem::take(&mut self.code)));
                    self.set_code(input, String::new(), options, learned);
                }
            }
        }
        if !self.code.is_empty() {
            steps.push(Flushed::Raw(std::mem::take(&mut self.code)));
            self.set_code(input, String::new(), options, learned);
        }
        steps
    }
    /// What space/punctuation/Shift+letter commits: the first candidate on this page, or the raw
    /// code (second item false) when there is none.
    pub fn first_or_raw(&self) -> (String, bool) {
        match self.candidate_at(0) {
            Some(pick) => (pick.text, true),
            None => (self.code.clone(), false),
        }
    }
    /// Types a code letter (lowercase). Returns candidates committed on the way (fifth-letter
    /// commit, unique four-letter code) in order; the platform layer writes them into the draft.
    /// Afterwards the state is updated (the code is empty after a unique four-letter commit).
    /// The code edit itself is [`edit_code`], shared with in-document composition.
    pub fn type_letter(
        &mut self,
        input: &MixedInput,
        letter: char,
        rules: LetterRules,
        options: &ComposeOptions,
        learned: &Learned,
    ) -> Vec<Pick> {
        let wubi_rules = !options.pinyin_first && options.shuangpin.is_none();
        let edit = edit_code(&self.code, letter, wubi_rules.then_some((input, rules)));
        if edit.ignored {
            return Vec::new();
        }
        let mut picks = Vec::new();
        if edit.fifth && input.wubi_count(&self.code) > 0 {
            if let Some(pick) = self.candidate_at(0) {
                picks.push(pick);
            }
        }
        self.set_code(input, edit.code, options, learned);
        if edit.auto
            && let Some(pick) = self.candidate_at(0)
        {
            picks.push(pick);
            self.set_code(input, String::new(), options, learned);
        }
        picks
    }
}

/// How [`edit_code`] changes a code. Both the draft box and in-document composition use this,
/// then each looks up the candidate text in its own list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeEdit {
    /// The fifth-letter rule cleared the previous code before `code`.
    pub fifth: bool,
    /// Code after the letter, or the original code when [`Self::ignored`].
    pub code: String,
    /// The letter was not added: the code was already at [`MAX_CODE_LEN`].
    pub ignored: bool,
    /// `code` is a unique four-letter Wubi code and should be committed.
    pub auto: bool,
}

/// Fifth-letter and unique-four rules, then the new letter. `wubi` is None for pinyin-first and
/// Shuangpin, where those rules stay off. A letter past [`MAX_CODE_LEN`] sets [`CodeEdit::ignored`].
pub fn edit_code(code: &str, letter: char, wubi: Option<(&MixedInput, LetterRules)>) -> CodeEdit {
    let fifth =
        wubi.is_some_and(|(input, rules)| rules.push_fifth && pushes_fifth(input, code, letter));
    let mut next = if fifth {
        String::new()
    } else {
        code.to_owned()
    };
    let ignored = next.len() >= MAX_CODE_LEN;
    if ignored {
        next = code.to_owned();
    } else {
        next.push(letter);
    }
    let auto = !ignored
        && wubi.is_some_and(|(input, rules)| rules.auto_commit4 && auto_commits_four(input, &next));
    CodeEdit {
        fifth,
        code: next,
        ignored,
        auto,
    }
}

/// Fifth-letter rule: four letters already typed (not counting `'` and wildcard `z`) and this
/// letter cannot continue as pinyin.
///
/// If the four letters are not a Wubi code and the new letter only makes a typo of two swapped
/// letters (`zhnog`, `xina`), it may still be pinyin and is not cleared, so typo correction can
/// handle it.
pub fn pushes_fifth(input: &MixedInput, code: &str, letter: char) -> bool {
    let next = format!("{code}{letter}");
    code.len() == 4
        && !code.contains(['\'', 'z'])
        && !input.pinyin_continues(&next)
        && (input.wubi_count(code) > 0 || !input.pinyin_continues_after_swap(&next))
}

/// Unique four-letter rule: the full four-letter Wubi code has exactly one candidate and cannot be
/// pinyin.
pub fn auto_commits_four(input: &MixedInput, code: &str) -> bool {
    code.len() == 4
        && !code.contains(['\'', 'z'])
        && input.wubi_count(code) == 1
        && !input.pinyin_continues(code)
}

/// Pick a character from a word (`[` `]` in Microsoft Pinyin and Sogou): the first or last
/// character; None for a single character (the key is then plain punctuation).
pub fn char_of_word(text: &str, last: bool) -> Option<String> {
    let mut chars = text.chars();
    let first = chars.next()?;
    let tail = chars.last()?;
    Some(if last { tail } else { first }.to_string())
}

/// Paging keys: `-` `=` always, `,` `.` when comma/period paging is on. Some(true) = next page,
/// Some(false) = previous.
pub fn paging_direction(ch: char, comma_period: bool) -> Option<bool> {
    match ch {
        '=' => Some(true),
        '-' => Some(false),
        '.' if comma_period => Some(true),
        ',' if comma_period => Some(false),
        _ => None,
    }
}

/// Virtual-key paging on a US layout (the keys the README lists): `-` `=` and PageUp/PageDown
/// always, `,` `.` when `comma_period` is on. Shifted keys are not paging (`+` `_` `<` `>`).
/// Some(true) = next page, Some(false) = previous.
pub fn paging_direction_vk(vk: u16, shift: bool, comma_period: bool) -> Option<bool> {
    if shift {
        return None;
    }
    match vk {
        0xBB | 0x22 => Some(true),           // `=` / PageDown (VK_NEXT)
        0xBD | 0x21 => Some(false),          // `-` / PageUp (VK_PRIOR)
        0xBE if comma_period => Some(true),  // `.`
        0xBC if comma_period => Some(false), // `,`
        _ => None,
    }
}

/// Selection keys: `;` picks the 2nd candidate, `'` the 3rd (unless the code may be pinyin or
/// already has a separator, where `'` separates syllables).
pub fn selection_index(input: &MixedInput, code: &str, ch: char) -> Option<usize> {
    match ch {
        ';' => Some(1),
        '\'' if !code.contains('\'') && !input.pinyin_continues(code) => Some(2),
        _ => None,
    }
}

/// How a letter key is handled in the input box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetterAction {
    /// Code letter (lowercase).
    Compose(char),
    /// Caps Lock or Shift+letter: typed as is (as in Microsoft Wubi); while composing, the first
    /// candidate or raw code is committed first.
    Raw { flush_first: bool },
}

/// How an ASCII letter is handled; None for other characters.
pub fn classify_letter(ch: char, caps_lock: bool, composing: bool) -> Option<LetterAction> {
    if !ch.is_ascii_alphabetic() {
        return None;
    }
    Some(if caps_lock || ch.is_ascii_uppercase() {
        LetterAction::Raw {
            flush_first: composing,
        }
    } else {
        LetterAction::Compose(ch.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuangpin_odd_key_partial_candidate_keeps_complete_rest_plus_pending() {
        let pinyin = "...\n中国\tzhong guo\t100\n人\tren\t90\n";
        let input = crate::MixedInput::from_rime("...\n", pinyin);
        let options = ComposeOptions {
            page_size: 5,
            traditional: false,
            learn: false,
            fuzzy: false,
            pinyin_first: true,
            common_only: false,
            shuangpin: Some(crate::Shuangpin::Xiaohe),
        };
        let learned = Learned::default();
        let mut composer = Composer::default();
        // Xiaohe: vs=zhong, go=guo, rf=ren, u=trailing odd initial.
        composer.set_code(&input, "vsgorfu".into(), &options, &learned);
        let idx = composer
            .candidates
            .iter()
            .position(|c| c.text == "中国")
            .expect("partial 中国 candidate present");
        assert_eq!(composer.candidates[idx].rest, "rfu");

        let pick = composer.candidate_at(idx).unwrap();
        assert_eq!(pick.text, "中国");
        assert_eq!(pick.rest, "rfu");
        composer.set_code(&input, pick.rest, &options, &learned);
        assert_eq!(composer.code, "rfu");
    }
}
