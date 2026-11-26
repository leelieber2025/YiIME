//! Shift tap mid-composition toggles an inline English segment (MS Pinyin / Sogou / Rime convention).
//!
//! - A tap is Shift pressed and released with no other key, within [`SHIFT_TAP_MAX`], with
//!   Ctrl/Alt/Win not held.
//! - Entering English commits the pending code as raw letters; letters, digits, spaces and
//!   punctuation then go into the draft as typed. Tap Shift again to return to Chinese.
//! - Enter still translates the mixed draft; Shift+Enter / Ctrl+Enter commit it as Chinese.

use std::time::{Duration, Instant};

/// Longest Shift hold that still counts as a tap (same as the global Chinese/English toggle in the
/// TSF DLL).
pub const SHIFT_TAP_MAX: Duration = Duration::from_millis(600);

pub fn is_shift_key(vk: u32) -> bool {
    matches!(vk, 0x10 | 0xA0 | 0xA1)
}

/// Numpad digits (VK_NUMPAD0..9) do not pick candidates while composing: commit the first
/// candidate, then type the digit (Microsoft Pinyin convention).
pub fn is_numpad_digit(vk: u32) -> bool {
    (0x60..=0x69).contains(&vk)
}

/// Detects Shift pressed and released on its own.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShiftTap {
    down: Option<Instant>,
}

impl ShiftTap {
    /// `other_modifier`: Ctrl/Alt/Win is held.
    pub fn key_down(&mut self, vk: u32, other_modifier: bool, now: Instant) {
        if !is_shift_key(vk) || other_modifier {
            self.down = None;
        } else if self.down.is_none() {
            // Auto-repeat does not reset the start time.
            self.down = Some(now);
        }
    }
    /// Returns whether this release completes a Shift tap.
    pub fn key_up(&mut self, vk: u32, now: Instant) -> bool {
        if !is_shift_key(vk) {
            return false;
        }
        self.down
            .take()
            .is_some_and(|start| now.saturating_duration_since(start) <= SHIFT_TAP_MAX)
    }
    pub fn cancel(&mut self) {
        self.down = None;
    }
}

/// Letters committed when switching to English: the code without `'` separators (None if empty).
pub fn literal_code(code: &str) -> Option<String> {
    let letters: String = code.chars().filter(|ch| *ch != '\'').collect();
    (!letters.is_empty()).then_some(letters)
}

/// Characters typed as is in an English segment: printable characters and space
/// (Enter/Esc/Backspace keep their usual handling).
pub fn is_literal_in_english(ch: char) -> bool {
    !ch.is_control()
}

#[cfg(feature = "engine")]
pub mod ui {
    use crate::MAX_CODE_LEN;
    use crate::dict::{MixedInput, Source};
    use crate::engine::{
        LetterAction, LetterRules, auto_commits_four, classify_letter, paging_direction_vk,
        pushes_fifth,
    };
    use crate::keys::semicolon_is_code;
    use crate::shuangpin::Shuangpin;
    use crate::text::to_traditional;
    use std::collections::HashSet;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DisplayCandidate {
        pub text: String,
        pub hint: String,
        pub complete: bool,
        pub rest: String,
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_candidates(
        input: &MixedInput,
        code: &str,
        size: usize,
        fuzzy: bool,
        traditional: bool,
        common_only: bool,
        pinyin_first: bool,
        shuangpin: Option<Shuangpin>,
    ) -> Vec<DisplayCandidate> {
        let pinyin_first = pinyin_first || shuangpin.is_some();
        let pending = match shuangpin {
            Some(_) if code.replace('\'', "").len() % 2 == 1 => code.chars().last(),
            _ => None,
        };
        let complete_keys = match shuangpin {
            Some(_) => &code[..code.len() - pending.map_or(0, char::len_utf8)],
            None => code,
        };
        let lookup = match shuangpin {
            _ if code.is_empty() => None,
            Some(scheme) => {
                if complete_keys.replace('\'', "").is_empty() {
                    None
                } else {
                    scheme.expand(complete_keys)
                }
            }
            None => Some(code.to_owned()),
        };
        let mut items = match &lookup {
            None => Vec::new(),
            Some(lookup) => input.candidates_filtered(
                lookup,
                size.max(1) * crate::CANDIDATE_PAGES,
                size.max(1),
                fuzzy,
                pinyin_first,
                common_only,
                common_only && !traditional,
            ),
        };
        if let Some(scheme) = shuangpin {
            items.retain_mut(|candidate| {
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
                for candidate in &mut items {
                    candidate.rest.push(key);
                    candidate.source = Source::Partial;
                }
            }
        }
        let mut rows: Vec<DisplayCandidate> = items
            .into_iter()
            .map(|mut candidate| {
                let hint = input.candidate_hint(code, &candidate);
                if traditional {
                    candidate.text = to_traditional(&candidate.text);
                }
                DisplayCandidate {
                    text: candidate.text,
                    hint,
                    complete: candidate.complete,
                    rest: candidate.rest,
                }
            })
            .collect();
        if traditional {
            let mut seen = HashSet::new();
            rows.retain(|item| seen.insert((item.text.clone(), item.rest.clone())));
        }
        rows
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum HitTarget {
        Candidate(usize),
        PrevPage,
        NextPage,
    }

    #[derive(Debug, Clone)]
    pub struct CandidateView {
        pub code: String,
        pub rows: Vec<DisplayCandidate>,
        pub size: usize,
        pub page: usize,
        pub page_starts: Vec<usize>,
        pub item_spans: Vec<(i32, i32)>,
        pub prev_span: (i32, i32),
        pub next_span: (i32, i32),
        pub bar_height: i32,
    }

    impl CandidateView {
        pub fn empty(size: usize) -> Self {
            Self {
                code: String::new(),
                rows: Vec::new(),
                size: size.max(1),
                page: 0,
                page_starts: vec![0],
                item_spans: Vec::new(),
                prev_span: (0, 0),
                next_span: (0, 0),
                bar_height: 0,
            }
        }

        pub fn with_rows(
            code: String,
            rows: Vec<DisplayCandidate>,
            size: usize,
            page: usize,
        ) -> Self {
            let size = size.max(1);
            let mut page_starts: Vec<usize> = (0..rows.len()).step_by(size).collect();
            if page_starts.is_empty() {
                page_starts.push(0);
            }
            let last = page_starts.len().saturating_sub(1);
            Self {
                code,
                rows,
                size,
                page: page.min(last),
                page_starts,
                item_spans: Vec::new(),
                prev_span: (0, 0),
                next_span: (0, 0),
                bar_height: 0,
            }
        }

        pub fn reset(&mut self) {
            self.code.clear();
            self.rows.clear();
            self.page = 0;
            self.page_starts = vec![0];
            self.item_spans.clear();
            self.prev_span = (0, 0);
            self.next_span = (0, 0);
            self.bar_height = 0;
        }

        pub fn set_page_starts(&mut self, starts: &[u32]) -> bool {
            if starts.is_empty() || starts[0] != 0 {
                return false;
            }
            let limit = self.rows.len();
            let mut parsed = Vec::with_capacity(starts.len());
            for (i, &start) in starts.iter().enumerate() {
                let idx = start as usize;
                if i > 0 && (idx <= parsed[i - 1] || idx >= limit) {
                    return false;
                }
                parsed.push(idx);
            }
            self.page_starts = parsed;
            self.page = self.page.min(self.page_starts.len().saturating_sub(1));
            true
        }

        pub fn page_count(&self) -> usize {
            self.page_starts.len().max(1)
        }

        pub fn current_page_start(&self) -> usize {
            self.page_starts
                .get(self.page)
                .copied()
                .unwrap_or(0)
                .min(self.rows.len())
        }

        pub fn page_items(&self) -> &[DisplayCandidate] {
            let start = self.current_page_start();
            let end = self
                .page_starts
                .get(self.page + 1)
                .copied()
                .unwrap_or(self.rows.len())
                .min(self.rows.len());
            if start >= end {
                &[]
            } else {
                &self.rows[start..end]
            }
        }

        pub fn turn_page(&mut self, forward: bool) -> bool {
            let last = self.page_count().saturating_sub(1);
            let next = if forward {
                self.page.saturating_add(1).min(last)
            } else {
                self.page.saturating_sub(1)
            };
            if next == self.page {
                false
            } else {
                self.page = next;
                true
            }
        }

        pub fn choice(&self, index: usize) -> Option<(String, String)> {
            self.page_items()
                .get(index)
                .map(|item| (item.text.clone(), item.rest.clone()))
        }

        pub fn hit_test(&self, x: i32, y: i32) -> Option<HitTarget> {
            if y < 0 || y >= self.bar_height {
                return None;
            }
            for (idx, &(left, right)) in self.item_spans.iter().enumerate() {
                if x >= left && x < right {
                    return Some(HitTarget::Candidate(idx));
                }
            }
            if x >= self.prev_span.0 && x < self.prev_span.1 {
                return Some(HitTarget::PrevPage);
            }
            if x >= self.next_span.0 && x < self.next_span.1 {
                return Some(HitTarget::NextPage);
            }
            None
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum InlineKeyOutcome {
        Start(String),
        Update(String),
        Finish(String),
        PickPartial { committed: String, rest: String },
        Page(bool),
        Swallow,
        NotHandled,
    }

    /// One key for [`LogicalInline::handle_key_with`].
    pub struct InlineKey<'a> {
        pub key: usize,
        pub shift: bool,
        pub caps_lock: bool,
        pub comma_period_paging: bool,
        pub shuangpin: Option<Shuangpin>,
        /// Microsoft Wubi letter rules (fifth letter, unique four-letter code); None with
        /// pinyin-first or Shuangpin.
        pub wubi: Option<(&'a MixedInput, LetterRules)>,
        /// What the key types when it is not a composition key (Chinese or ASCII punctuation, a
        /// digit): the composition is committed and this follows it.
        pub typed: Option<String>,
    }

    /// Arrows, Home/End, PageUp/PageDown (when not paging) and Delete: kept while composing.
    pub fn is_navigation(key: usize) -> bool {
        matches!(key, 0x21..=0x28 | 0x2E)
    }

    /// Commits a whole code by taking the first candidate segment by segment, raw letters for a
    /// segment with none (same as [`crate::Composer::flush_all`] in the draft box).
    pub fn flush_code(code: &str, first_for: &dyn Fn(&str) -> Option<(String, String)>) -> String {
        let letters = |code: &str| code.replace('\'', "").len();
        let mut out = String::new();
        let mut code = code.to_owned();
        for _ in 0..=MAX_CODE_LEN {
            if code.is_empty() {
                break;
            }
            match first_for(&code) {
                Some((text, rest)) if letters(&rest) < letters(&code) => {
                    out.push_str(&text);
                    code = rest;
                }
                _ => {
                    out.push_str(&code);
                    code.clear();
                }
            }
        }
        out.push_str(&code);
        out
    }

    #[derive(Debug, Clone)]
    pub struct LogicalInline<C> {
        active: Option<(C, String)>,
    }

    impl<C> Default for LogicalInline<C> {
        fn default() -> Self {
            Self { active: None }
        }
    }

    impl<C: PartialEq + Clone> LogicalInline<C> {
        pub fn active(&self) -> bool {
            self.active.is_some()
        }

        pub fn same_context(&self, context: &C) -> bool {
            self.active.as_ref().is_some_and(|(ctx, _)| ctx == context)
        }

        pub fn code(&self, context: &C) -> Option<&str> {
            self.active
                .as_ref()
                .filter(|(ctx, _)| ctx == context)
                .map(|(_, code)| code.as_str())
        }

        pub fn sync_from_composition(&mut self, context: &C, code: &str) {
            self.active = Some((context.clone(), code.to_owned()));
        }

        pub fn clear(&mut self) {
            self.active = None;
        }

        pub fn wants(
            &self,
            context: &C,
            key: usize,
            shift: bool,
            comma_period_paging: bool,
            shuangpin: Option<Shuangpin>,
        ) -> bool {
            self.wants_with(context, key, shift, comma_period_paging, shuangpin, false)
        }

        /// `types_text`: the key types a character (punctuation, digit, numpad); while composing
        /// it commits the composition and then types that character instead of landing inside
        /// the composition in the host (SampleIME `_HandleCompositionPunctuation`). Navigation
        /// keys are kept while composing so the caret cannot leave the composition.
        pub fn wants_with(
            &self,
            context: &C,
            key: usize,
            shift: bool,
            comma_period_paging: bool,
            shuangpin: Option<Shuangpin>,
            types_text: bool,
        ) -> bool {
            if let Some(code) = self.code(context) {
                if types_text || is_navigation(key) {
                    return true;
                }
                let paging = u16::try_from(key).ok().is_some_and(|vk| {
                    paging_direction_vk(vk, shift, comma_period_paging).is_some()
                });
                let shuangpin_semi = key == 0xBA
                    && !shift
                    && shuangpin.is_some_and(|scheme| semicolon_is_code(scheme, code));
                paging
                    || shuangpin_semi
                    || matches!(
                        key,
                        0x41..=0x5a | 0x08 | 0x1b | 0x0d | 0x20 | 0x31..=0x39
                    )
            } else {
                (0x41..=0x5a).contains(&key)
            }
        }

        #[allow(clippy::too_many_arguments)]
        pub fn handle_key(
            &mut self,
            context: &C,
            key: usize,
            shift: bool,
            caps_lock: bool,
            comma_period_paging: bool,
            shuangpin: Option<Shuangpin>,
            choice_at: impl FnOnce(usize) -> Option<(String, String)>,
        ) -> InlineKeyOutcome {
            self.handle_key_with(
                context,
                InlineKey {
                    key,
                    shift,
                    caps_lock,
                    comma_period_paging,
                    shuangpin,
                    wubi: None,
                    typed: None,
                },
                choice_at,
                &|_| None,
            )
        }

        /// `first_for(code)`: the first candidate of any code (text, unconsumed rest), used by the
        /// unique-four-letter rule and to commit the whole code before typed punctuation.
        pub fn handle_key_with(
            &mut self,
            context: &C,
            input: InlineKey<'_>,
            choice_at: impl FnOnce(usize) -> Option<(String, String)>,
            first_for: &dyn Fn(&str) -> Option<(String, String)>,
        ) -> InlineKeyOutcome {
            let InlineKey {
                key,
                shift,
                caps_lock,
                comma_period_paging,
                shuangpin,
                wubi,
                typed,
            } = input;
            let Some(mut code) = self.code(context).map(str::to_owned) else {
                if !(0x41..=0x5a).contains(&key) {
                    return InlineKeyOutcome::NotHandled;
                }
                let ch = (key as u8 as char).to_ascii_lowercase().to_string();
                self.active = Some((context.clone(), ch.clone()));
                return InlineKeyOutcome::Start(ch);
            };
            match key {
                0x41..=0x5a => {
                    let base = (key as u8 as char).to_ascii_lowercase();
                    let typed = if shift {
                        base.to_ascii_uppercase()
                    } else {
                        base
                    };
                    match classify_letter(typed, caps_lock, true) {
                        Some(LetterAction::Raw { flush_first: true }) => {
                            let out = format!("{code}{typed}");
                            self.active = None;
                            InlineKeyOutcome::Finish(out)
                        }
                        Some(LetterAction::Compose(c)) => {
                            // Microsoft Wubi rules, as in the draft box (Composer::type_letter):
                            // a fifth letter commits the first candidate (or drops four letters
                            // that have none) and starts the next character.
                            let mut committed = String::new();
                            let fifth = wubi.filter(|(input, rules)| {
                                rules.push_fifth && pushes_fifth(input, &code, c)
                            });
                            if let Some((input, _)) = fifth {
                                if input.wubi_count(&code) > 0 {
                                    if let Some((text, _)) = choice_at(0) {
                                        committed = text;
                                    }
                                }
                                code.clear();
                            }
                            if code.len() >= MAX_CODE_LEN {
                                return InlineKeyOutcome::Swallow;
                            }
                            code.push(c);
                            let unique = wubi.is_some_and(|(input, rules)| {
                                rules.auto_commit4 && auto_commits_four(input, &code)
                            });
                            if let Some((text, _)) = unique.then(|| first_for(&code)).flatten() {
                                self.active = None;
                                return InlineKeyOutcome::Finish(committed + &text);
                            }
                            self.active = Some((context.clone(), code.clone()));
                            if committed.is_empty() {
                                InlineKeyOutcome::Update(code)
                            } else {
                                InlineKeyOutcome::PickPartial {
                                    committed,
                                    rest: code,
                                }
                            }
                        }
                        _ => InlineKeyOutcome::NotHandled,
                    }
                }
                0xBA if !shift
                    && shuangpin.is_some_and(|scheme| semicolon_is_code(scheme, &code)) =>
                {
                    if code.len() >= MAX_CODE_LEN {
                        return InlineKeyOutcome::Swallow;
                    }
                    code.push(';');
                    self.active = Some((context.clone(), code.clone()));
                    InlineKeyOutcome::Update(code)
                }
                0x08 => {
                    code.pop();
                    if code.is_empty() {
                        self.active = None;
                        InlineKeyOutcome::Finish(String::new())
                    } else {
                        self.active = Some((context.clone(), code.clone()));
                        InlineKeyOutcome::Update(code)
                    }
                }
                0x1b => {
                    self.active = None;
                    InlineKeyOutcome::Finish(String::new())
                }
                0x0d => {
                    self.active = None;
                    InlineKeyOutcome::Finish(code)
                }
                0x20 => {
                    if let Some((text, rest)) = choice_at(0) {
                        self.apply_choice(context, text, rest)
                    } else {
                        self.active = None;
                        InlineKeyOutcome::Finish(code)
                    }
                }
                0x31..=0x39 if !shift => {
                    if let Some((text, rest)) = choice_at(key - 0x31) {
                        self.apply_choice(context, text, rest)
                    } else {
                        // No candidate at that number: keep the digit out of the composition.
                        InlineKeyOutcome::Swallow
                    }
                }
                _ => {
                    if let Some(forward) = u16::try_from(key)
                        .ok()
                        .and_then(|vk| paging_direction_vk(vk, shift, comma_period_paging))
                    {
                        return InlineKeyOutcome::Page(forward);
                    }
                    if let Some(text) = typed {
                        self.active = None;
                        return InlineKeyOutcome::Finish(flush_code(&code, first_for) + &text);
                    }
                    if is_navigation(key) {
                        return InlineKeyOutcome::Swallow;
                    }
                    InlineKeyOutcome::NotHandled
                }
            }
        }

        /// The active composition's context.
        pub fn context(&self) -> Option<&C> {
            self.active.as_ref().map(|(context, _)| context)
        }

        pub fn pick_choice(
            &mut self,
            context: &C,
            choice: Option<(String, String)>,
        ) -> Option<InlineKeyOutcome> {
            self.code(context)?;
            let (text, rest) = choice?;
            Some(self.apply_choice(context, text, rest))
        }

        fn apply_choice(&mut self, context: &C, text: String, rest: String) -> InlineKeyOutcome {
            if rest.is_empty() {
                self.active = None;
                InlineKeyOutcome::Finish(text)
            } else {
                self.active = Some((context.clone(), rest.clone()));
                InlineKeyOutcome::PickPartial {
                    committed: text,
                    rest,
                }
            }
        }

        pub fn commit_raw(&mut self, context: &C) -> Option<String> {
            let code = self.code(context)?.to_owned();
            self.active = None;
            Some(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_tap_rules() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut tap = ShiftTap::default();
        tap.key_down(0x10, false, t0);
        assert!(tap.key_up(0x10, t0 + ms(120)));
        // Held too long.
        tap.key_down(0xA0, false, t0);
        assert!(!tap.key_up(0xA0, t0 + ms(900)));
        // Auto-repeat keeps the original start.
        tap.key_down(0x10, false, t0);
        tap.key_down(0x10, false, t0 + ms(500));
        assert!(!tap.key_up(0x10, t0 + ms(700)));
        // Shift+letter and Shift+Enter are not taps.
        tap.key_down(0x10, false, t0);
        tap.key_down(b'A' as u32, false, t0 + ms(50));
        assert!(!tap.key_up(0x10, t0 + ms(100)));
        tap.key_down(0x10, false, t0);
        tap.key_down(0x0D, false, t0 + ms(50));
        assert!(!tap.key_up(0x10, t0 + ms(100)));
        // Not a tap while Ctrl is held (Ctrl+Shift).
        tap.key_down(0x10, true, t0);
        assert!(!tap.key_up(0x10, t0 + ms(100)));
        // Releasing another key is not a tap.
        assert!(!tap.key_up(b'A' as u32, t0));
    }

    #[test]
    fn literal_code_and_chars() {
        assert_eq!(literal_code("zhong'guo"), Some("zhongguo".to_owned()));
        assert_eq!(literal_code(""), None);
        assert!(is_literal_in_english(' '));
        assert!(is_literal_in_english('A'));
        assert!(is_literal_in_english(','));
        assert!(!is_literal_in_english('\r'));
        assert!(!is_literal_in_english('\u{8}'));
    }
}

#[cfg(all(test, feature = "engine"))]
mod ui_tests {
    use super::ui::*;
    use crate::{MixedInput, Shuangpin};

    #[test]
    fn build_candidates_handles_shuangpin_odd_key_partial_and_traditional_dedup() {
        let pinyin = "...\n中国\tzhong guo\t100\n人\tren\t90\n头发\ttou fa\t80\n頭髮\ttou fa\t70\n";
        let input = MixedInput::from_rime("...\n", pinyin);
        let rows = build_candidates(
            &input,
            "vsgorfu",
            5,
            false,
            false,
            false,
            true,
            Some(Shuangpin::Xiaohe),
        );
        let partial = rows.iter().find(|r| r.text == "中国").unwrap();
        assert_eq!(partial.rest, "rfu");

        let trad = build_candidates(&input, "toufa", 5, false, true, false, true, None);
        assert_eq!(trad.iter().filter(|r| r.text == "頭髮").count(), 1);
    }

    #[test]
    fn logical_inline_handles_rapid_keys_enter_partial_semicolon_shift_and_max_len() {
        let ctx = 1usize;
        let mut logic = LogicalInline::default();

        // Rapid async keys: Start('n'), Update('ni'), Update('nih') before first callback completes.
        assert_eq!(
            logic.handle_key(&ctx, b'N' as usize, false, false, true, None, |_| None),
            InlineKeyOutcome::Start("n".into())
        );
        assert!(logic.active());
        assert_eq!(
            logic.handle_key(&ctx, b'I' as usize, false, false, true, None, |_| None),
            InlineKeyOutcome::Update("ni".into())
        );
        assert_eq!(
            logic.handle_key(&ctx, b'H' as usize, false, false, true, None, |_| None),
            InlineKeyOutcome::Update("nih".into())
        );
        assert_eq!(logic.code(&ctx), Some("nih"));

        // Enter commits raw code, Space picks candidate 0.
        assert_eq!(
            logic.handle_key(&ctx, 0x0d, false, false, true, None, |_| Some((
                "你".into(),
                String::new()
            ))),
            InlineKeyOutcome::Finish("nih".into())
        );
        assert!(!logic.active());

        // Partial candidate pick keeps `rest` active.
        assert_eq!(
            logic.handle_key(&ctx, b'X' as usize, false, false, true, None, |_| None),
            InlineKeyOutcome::Start("x".into())
        );
        assert_eq!(
            logic.handle_key(&ctx, 0x20, false, false, true, None, |_| Some((
                "西".into(),
                "an".into()
            ))),
            InlineKeyOutcome::PickPartial {
                committed: "西".into(),
                rest: "an".into(),
            }
        );
        assert_eq!(logic.code(&ctx), Some("an"));

        // Shift+letter flushes pending code + uppercase letter once.
        assert_eq!(
            logic.handle_key(&ctx, b'A' as usize, true, false, true, None, |_| None),
            InlineKeyOutcome::Finish("anA".into())
        );
        assert!(!logic.active());

        // Microsoft Shuangpin `;` is accepted during active composition as final `ing`.
        assert_eq!(
            logic.handle_key(
                &ctx,
                b'B' as usize,
                false,
                false,
                true,
                Some(Shuangpin::Microsoft),
                |_| None
            ),
            InlineKeyOutcome::Start("b".into())
        );
        assert!(logic.wants(&ctx, 0xBA, false, true, Some(Shuangpin::Microsoft)));
        assert_eq!(
            logic.handle_key(
                &ctx,
                0xBA,
                false,
                false,
                true,
                Some(Shuangpin::Microsoft),
                |_| None
            ),
            InlineKeyOutcome::Update("b;".into())
        );

        // MAX_CODE_LEN (32) swallows 33rd char, backspace pops.
        for _ in 0..30 {
            let _ = logic.handle_key(&ctx, b'A' as usize, false, false, true, None, |_| None);
        }
        assert_eq!(logic.code(&ctx).unwrap().len(), 32);
        assert_eq!(
            logic.handle_key(&ctx, b'A' as usize, false, false, true, None, |_| None),
            InlineKeyOutcome::Swallow
        );
        assert_eq!(logic.code(&ctx).unwrap().len(), 32);
        assert_eq!(
            logic.handle_key(&ctx, 0x08, false, false, true, None, |_| None),
            InlineKeyOutcome::Update(logic.code(&ctx).unwrap()[..31].to_owned())
        );
    }

    #[test]
    fn inline_commits_before_typed_keys_keeps_navigation_and_applies_wubi_rules() {
        use crate::LetterRules;
        let ctx = 1usize;
        let key = |key: usize, wubi, typed: Option<&str>| InlineKey {
            key,
            shift: false,
            caps_lock: false,
            comma_period_paging: false,
            shuangpin: None,
            wubi,
            typed: typed.map(str::to_owned),
        };
        let first = |code: &str| match code {
            "ni" => Some(("你".to_owned(), String::new())),
            "nihao" => Some(("你".to_owned(), "hao".to_owned())),
            "hao" => Some(("好".to_owned(), String::new())),
            "gggg" => Some(("王".to_owned(), String::new())),
            _ => None,
        };
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "ni");
        // Punctuation (or a numpad digit) commits the composition, then its own character.
        assert!(logic.wants_with(&ctx, 0xBC, false, false, None, true));
        assert_eq!(
            logic.handle_key_with(&ctx, key(0xBC, None, Some("，")), |_| None, &first),
            InlineKeyOutcome::Finish("你，".into())
        );
        assert!(!logic.active());
        // The whole code is committed segment by segment; letters with no candidate stay.
        assert_eq!(flush_code("nihao", &first), "你好");
        assert_eq!(flush_code("xq", &first), "xq");
        // Arrows and Delete stay out of the host while composing; an empty number slot too.
        logic.sync_from_composition(&ctx, "ni");
        assert!(logic.wants_with(&ctx, 0x25, false, false, None, false));
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x25, None, None), |_| None, &first),
            InlineKeyOutcome::Swallow
        );
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x37, None, None), |_| None, &first),
            InlineKeyOutcome::Swallow
        );
        assert_eq!(logic.code(&ctx), Some("ni"));
        // Not composing: arrows and punctuation belong to the host.
        logic.clear();
        assert!(!logic.wants_with(&ctx, 0x25, false, false, None, true));

        // Unique four-letter Wubi code commits itself; a fifth letter commits the first one.
        let wubi =
            MixedInput::from_rime("...\n王\tgggg\t100\n工\taaaa\t100\n式\taaaa\t90\n", "...\n");
        let rules = LetterRules {
            push_fifth: true,
            auto_commit4: true,
        };
        logic.sync_from_composition(&ctx, "ggg");
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key(b'G' as usize, Some((&wubi, rules)), None),
                |_| None,
                &first
            ),
            InlineKeyOutcome::Finish("王".into())
        );
        logic.sync_from_composition(&ctx, "aaaa");
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key(b'G' as usize, Some((&wubi, rules)), None),
                |_| Some(("工".into(), String::new())),
                &first
            ),
            InlineKeyOutcome::PickPartial {
                committed: "工".into(),
                rest: "g".into(),
            }
        );
        // Rules off (the settings): four letters keep composing.
        logic.sync_from_composition(&ctx, "ggg");
        let off = LetterRules {
            push_fifth: false,
            auto_commit4: false,
        };
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key(b'G' as usize, Some((&wubi, off)), None),
                |_| None,
                &first
            ),
            InlineKeyOutcome::Update("gggg".into())
        );
        assert_eq!(logic.context(), Some(&ctx));
    }

    #[test]
    fn candidate_view_supports_variable_page_starts_reset_and_hit_testing() {
        let rows: Vec<_> = (0..12)
            .map(|n| DisplayCandidate {
                text: n.to_string(),
                hint: String::new(),
                complete: true,
                rest: String::new(),
            })
            .collect();
        let mut view = CandidateView::with_rows("x".into(), rows, 5, 1);
        assert_eq!(view.page_starts, vec![0, 5, 10]);
        assert!(view.set_page_starts(&[0, 3, 8]));
        assert_eq!(view.page_count(), 3);
        assert_eq!(view.current_page_start(), 3);
        assert_eq!(
            view.page_items()
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>(),
            vec!["3", "4", "5", "6", "7"]
        );
        assert!(view.turn_page(true));
        assert_eq!(view.current_page_start(), 8);
        assert_eq!(
            view.page_items()
                .iter()
                .map(|r| r.text.as_str())
                .collect::<Vec<_>>(),
            vec!["8", "9", "10", "11"]
        );
        assert!(!view.set_page_starts(&[1, 3]));
        assert!(!view.set_page_starts(&[0, 5, 5]));
        assert!(!view.set_page_starts(&[0, 12]));

        view.bar_height = 28;
        view.item_spans = vec![(8, 30), (46, 68)];
        view.prev_span = (80, 98);
        view.next_span = (98, 116);
        assert_eq!(view.hit_test(15, 10), Some(HitTarget::Candidate(0)));
        assert_eq!(view.hit_test(50, 10), Some(HitTarget::Candidate(1)));
        assert_eq!(view.hit_test(85, 10), Some(HitTarget::PrevPage));
        assert_eq!(view.hit_test(105, 10), Some(HitTarget::NextPage));
        assert_eq!(view.hit_test(105, 30), None);

        view.reset();
        assert!(view.code.is_empty());
        assert_eq!(view.page, 0);
        assert!(view.rows.is_empty());
    }
}
