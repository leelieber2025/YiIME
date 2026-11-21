//! Candidate engine: word grouping status (encoding, candidate, paging, Wubi prompt) and Microsoft
//! Wubi style key rules (five-digit top word, four-digit unique word automatically on the screen,
//! semicolon/single quote weight selection, Shift/Caps letters straight out). The platform layer is
//! only responsible for handing over the keys, writing the text on the screen into the draft, and
//! redrawing it.

use crate::dict::{Candidate, MixedInput, Source};
use crate::learn::Learned;
use crate::text::to_traditional;
use std::collections::HashMap;

/// Each query takes `page_size × CANDIDATE_PAGES` candidates.
pub const CANDIDATE_PAGES: usize = 10;
/// The maximum number of characters to encode (exceeding letters are ignored).
pub const MAX_CODE_LEN: usize = 32;

/// Settings used when refreshing candidates.
#[derive(Debug, Clone, Copy)]
pub struct ComposeOptions {
    pub page_size: usize,
    /// Traditional Chinese output: The candidate text is converted to Traditional Chinese (the Wubi
    /// prompt is still checked according to the original simplified text).
    pub traditional: bool,
    /// Self-learning sorting.
    pub learn: bool,
    /// Fuzzy sound (z/zh c/ch s/sh n/l an/ang en/eng in/ing), the default is off.
    pub fuzzy: bool,
}

/// Letter key rule switch.
#[derive(Debug, Clone, Copy)]
pub struct LetterRules {
    pub push_fifth: bool,
    pub auto_commit4: bool,
}

/// Selected candidates: coding and text (the platform layer writes a draft accordingly and records
/// learning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    /// The encoding used by this candidate (the previous candidate is just the first few
    /// syllables).
    pub code: String,
    pub text: String,
    /// The code that has not been used yet: After `text` is displayed on the screen when it is not
    /// empty, it should be used to continue composing characters (Microsoft Pinyin style character
    /// selection paragraph by paragraph).
    pub rest: String,
}

/// The word grouping status maintained by the input method in the floating box.
#[derive(Debug, Default)]
pub struct Composer {
    pub code: String,
    pub candidates: Vec<Candidate>,
    pub page: usize,
    /// Number of candidates per page (read from settings each time candidates are refreshed).
    pub page_size: usize,
    /// Display text → Wubi coding prompt, such as `(trwu)`; candidates matched by Wubi are not
    /// marked.
    pub hints: HashMap<String, String>,
}

impl Composer {
    /// Change to a new code and check the candidates again (an empty code means it will be
    /// cleared), and return to the first page.
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
            input.candidates_with_options(&code, size * CANDIDATE_PAGES, size, options.fuzzy)
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
                // Universal key candidates only mark the complete code, and the candidate bar is
                // shorter.
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
            learned.reorder(&code, &mut candidates);
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
    /// The Wubi coding prompt behind the candidate on the candidate bar (empty if not available).
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
    /// The `index`th candidate on the current page (None when exceeding this page, for example,
    /// press 7 when there are 5 candidates per page).
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
    /// The content on the screen when using spaces/punctuation/Shift+letters: the current page is
    /// preferred; when there are no candidates, the original encoding is used (the second item is
    /// false).
    pub fn first_or_raw(&self) -> (String, bool) {
        match self.candidate_at(0) {
            Some(pick) => (pick.text, true),
            None => (self.code.clone(), false),
        }
    }
    /// Enter a coded letter (lowercase). During the return period, the candidates selected on the
    /// screen (five-code top word, four-code unique) are in order; the platform layer writes them
    /// into the draft in sequence. The group word status has been updated (the code is empty after
    /// the only four codes are displayed on the screen).
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
        // Five-code top word/empty code clearing: When a letter is typed after four codes and it
        // cannot be pinyin, if there are candidates, the first choice will be displayed on the
        // screen. If there are no candidates (blank codes), the four codes will be discarded, and
        // the new letter will start the next character.
        if rules.push_fifth && pushes_fifth(input, &code, letter) {
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
        // The only four codes automatically appear on the screen.
        let auto = rules.auto_commit4 && auto_commits_four(input, &code);
        if let Some(pick) = self.candidate_at(0).filter(|_| auto) {
            picks.push(pick);
            self.set_code(input, String::new(), options, learned);
        }
        picks
    }
}

/// Five-code top word rules: There are already four codes (excluding diaphragm and universal key
/// z), and adding this letter cannot continue to be Pinyin.
///
/// When the fourth code is not the Wubi code, if it is just a misspelling after adding this letter
/// (two adjacent letters are swapped, such as zhnog, xina), it may still be pinyin, and it should
/// not be cleared, so that the misspelling correction can take over.
pub fn pushes_fifth(input: &MixedInput, code: &str, letter: char) -> bool {
    let next = format!("{code}{letter}");
    code.len() == 4
        && !code.contains(['\'', 'z'])
        && !input.pinyin_continues(&next)
        && (input.wubi_count(code) > 0 || !input.pinyin_continues_after_swap(&next))
}

/// The only automatic screen rule for four codes: There is only one candidate for the complete Wubi
/// code of four codes, and it cannot be Pinyin.
pub fn auto_commits_four(input: &MixedInput, code: &str) -> bool {
    code.len() == 4
        && !code.contains(['\'', 'z'])
        && input.wubi_count(code) == 1
        && !input.pinyin_continues(code)
}

/// Determine the word by word (Microsoft Pinyin, Sogou's `[` `]`): get the first or last character
/// of the word; when it is a single character, it is None (processed according to the original
/// punctuation).
pub fn char_of_word(text: &str, last: bool) -> Option<String> {
    let mut chars = text.chars();
    let first = chars.next()?;
    let tail = chars.last()?;
    Some(if last { tail } else { first }.to_string())
}

/// Page turning keys: `-` `=` always; `,` `.` when "comma period page turning" is turned on
/// (optional for Sogou and Microsoft Pinyin). Returns Some(true) for the next page and Some(false)
/// for the previous page.
pub fn paging_direction(ch: char, comma_period: bool) -> Option<bool> {
    match ch {
        '=' => Some(true),
        '-' => Some(false),
        '.' if comma_period => Some(true),
        ',' if comma_period => Some(false),
        _ => None,
    }
}

/// Select the repeated key: the semicolon selects the second candidate; the single quote selects
/// the third candidate (when the encoding may be Pinyin or contains diaphragms, the single quote
/// still serves as the delimiter).
pub fn selection_index(input: &MixedInput, code: &str, ch: char) -> Option<usize> {
    match ch {
        ';' => Some(1),
        '\'' if !code.contains('\'') && !input.pinyin_continues(code) => Some(2),
        _ => None,
    }
}

/// How letter keys are handled in the draft box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetterAction {
    /// as coded letters (lowercase).
    Compose(char),
    /// Caps Lock or Shift+letters: Enter letters as they are (consistent with Microsoft Wubi); when
    /// composing words, the preferred/original encoding is displayed on the screen first.
    Raw { flush_first: bool },
}

/// The processing method is given when `ch` is an ASCII letter, otherwise None.
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
