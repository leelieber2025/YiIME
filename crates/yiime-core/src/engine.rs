//! Candidate engine: composition state (code, candidates, paging, Wubi hints) and Microsoft Wubi
//! key rules (fifth letter commits the first candidate, a unique four-letter code commits itself,
//! `;` `'` pick the 2nd/3rd candidate, Shift/Caps letters go through as typed). The platform layer
//! only feeds keys, writes committed text into the draft and redraws.

use crate::dict::{Candidate, MixedInput, Source};
use crate::learn::Learned;
use crate::text::to_traditional;
use std::collections::HashMap;

/// Each query fetches `page_size × CANDIDATE_PAGES` candidates.
pub const CANDIDATE_PAGES: usize = 10;
/// Longest code; extra letters are ignored.
pub const MAX_CODE_LEN: usize = 32;

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

/// Composition state of the input box.
#[derive(Debug, Default)]
pub struct Composer {
    pub code: String,
    pub candidates: Vec<Candidate>,
    pub page: usize,
    /// Candidates per page (re-read from settings on every refresh).
    pub page_size: usize,
    /// Display text → Wubi hint such as `(trwu)`; candidates matched by Wubi code get none.
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
        let mut candidates = if code.is_empty() {
            Vec::new()
        } else {
            input.candidates_filtered(
                &code,
                size * CANDIDATE_PAGES,
                size,
                options.fuzzy,
                options.pinyin_first,
                options.common_only,
            )
        };
        self.hints.clear();
        for candidate in &mut candidates {
            let hint = if candidate.source == Source::Wubi {
                None
            } else {
                input.wubi_hint(&candidate.text)
            };
            if options.traditional {
                candidate.text = to_traditional(&candidate.text);
            }
            if let Some(hint) = hint {
                // Wildcard candidates show only the full code, to keep the bar short.
                let hint = if candidate.source == Source::Wildcard {
                    hint.rsplit('/').next().unwrap_or(&hint).to_owned()
                } else {
                    hint
                };
                let hint = if candidate.source == Source::Corrected {
                    format!("(纠错 {hint})")
                } else {
                    format!("({hint})")
                };
                self.hints.insert(candidate.text.clone(), hint);
            } else if candidate.source == Source::Corrected {
                self.hints
                    .insert(candidate.text.clone(), "(纠错)".to_owned());
            }
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
    pub fn type_letter(
        &mut self,
        input: &MixedInput,
        letter: char,
        rules: LetterRules,
        options: &ComposeOptions,
        learned: &Learned,
    ) -> Vec<Pick> {
        let mut picks = Vec::new();
        let mut code = self.code.clone();
        // Fifth letter: after four letters that cannot continue as pinyin, commit the first
        // candidate, or drop the four letters if there is none; the new letter starts the next
        // character.
        if !options.pinyin_first && rules.push_fifth && pushes_fifth(input, &code, letter) {
            if let Some(pick) = self.candidate_at(0).filter(|_| input.wubi_count(&code) > 0) {
                picks.push(pick);
                self.set_code(input, String::new(), options, learned);
            }
            code.clear();
        }
        if code.len() < MAX_CODE_LEN {
            code.push(letter);
            self.set_code(input, code.clone(), options, learned);
        }
        // A unique four-letter Wubi code commits itself.
        let auto = !options.pinyin_first && rules.auto_commit4 && auto_commits_four(input, &code);
        if let Some(pick) = self.candidate_at(0).filter(|_| auto) {
            picks.push(pick);
            self.set_code(input, String::new(), options, learned);
        }
        picks
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
