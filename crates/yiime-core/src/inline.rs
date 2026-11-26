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
        LetterAction, LetterRules, char_of_word, classify_letter, edit_code, paging_direction_vk,
        selection_index, shown_candidate,
    };
    use crate::keys::{
        KeyAction, KeyContext, classify_char, semicolon_is_code, shuangpin_selection,
        with_separator,
    };
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
        /// Drawn rectangles `(left, top, right, bottom)`. Clicks use these when present, so a
        /// repaint that changes the scale updates the hit targets to the same edges it draws.
        pub item_rects: Vec<(i32, i32, i32, i32)>,
        pub prev_rect: (i32, i32, i32, i32),
        pub next_rect: (i32, i32, i32, i32),
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
                item_rects: Vec::new(),
                prev_rect: (0, 0, 0, 0),
                next_rect: (0, 0, 0, 0),
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
                item_rects: Vec::new(),
                prev_rect: (0, 0, 0, 0),
                next_rect: (0, 0, 0, 0),
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
            self.item_rects.clear();
            self.prev_rect = (0, 0, 0, 0);
            self.next_rect = (0, 0, 0, 0);
        }

        /// Replace the click map with the rectangles just drawn.
        pub fn publish_click_rects(
            &mut self,
            items: &[(i32, i32, i32, i32)],
            prev: (i32, i32, i32, i32),
            next: (i32, i32, i32, i32),
            height: i32,
        ) {
            self.item_rects = items.to_vec();
            self.item_spans = items
                .iter()
                .map(|&(left, _, right, _)| (left, right))
                .collect();
            self.prev_rect = prev;
            self.next_rect = next;
            self.prev_span = (prev.0, prev.2);
            self.next_span = (next.0, next.2);
            self.bar_height = height;
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
            if !self.item_rects.is_empty() {
                for (idx, &(left, top, right, bottom)) in self.item_rects.iter().enumerate() {
                    if x >= left && x < right && y >= top && y < bottom {
                        return Some(HitTarget::Candidate(idx));
                    }
                }
                let (left, top, right, bottom) = self.prev_rect;
                if x >= left && x < right && y >= top && y < bottom {
                    return Some(HitTarget::PrevPage);
                }
                let (left, top, right, bottom) = self.next_rect;
                if x >= left && x < right && y >= top && y < bottom {
                    return Some(HitTarget::NextPage);
                }
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
        /// `;` picks the 2nd candidate and `'` the 3rd, except where `'` continues pinyin or
        /// 微软双拼 uses `;` as a code key. Off: `'` is still the pinyin separator.
        pub semicolon_select: bool,
        /// Dictionary for the pinyin-continues check on `'`. [`Self::wubi`]'s dictionary is used
        /// when this is None.
        pub select_input: Option<&'a MixedInput>,
        /// `[` `]` take one character from the first candidate (on by default).
        pub bracket_char_pick: bool,
        /// Code the drawn candidate page belongs to. When it differs from the logical code, a
        /// commit re-queries the logical code instead of using that page.
        pub page_code: Option<&'a str>,
    }

    /// Arrows, Home/End, PageUp/PageDown (when not paging) and Delete: kept while composing.
    pub fn is_navigation(key: usize) -> bool {
        matches!(key, 0x21..=0x28 | 0x2E)
    }

    /// Commits a whole code by taking the first candidate segment by segment, raw letters for a
    /// segment with none (same as [`crate::Composer::flush_all`] in the draft box).
    /// Punctuation commits the highlighted candidate on the current page, then any unconsumed
    /// tail by a fresh lookup (not another page of the old list).
    pub fn flush_visible(
        code: &str,
        page_code: Option<&str>,
        choice_at: &mut impl FnMut(usize) -> Option<(String, String)>,
        first_for: &dyn Fn(&str) -> Option<(String, String)>,
    ) -> String {
        let letters = |code: &str| code.replace('\'', "").len();
        let on_page = choice_at(0);
        let fresh = first_for(code);
        let mut out = String::new();
        let mut rest_code = code.to_owned();
        if let Some((text, rest)) = shown_candidate(page_code, code, on_page, fresh) {
            if letters(&rest) < letters(code) {
                out.push_str(&text);
                rest_code = rest;
            } else {
                out.push_str(code);
                return out;
            }
        }
        if !rest_code.is_empty() {
            out.push_str(&flush_code(&rest_code, first_for));
        }
        out
    }

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
                        0x41..=0x5a
                            | 0x08
                            | 0x1b
                            | 0x0d
                            | 0x20
                            | 0x31..=0x39
                            | 0xBA
                            | 0xDE
                            | 0xDB
                            | 0xDD
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
            choice_at: impl FnMut(usize) -> Option<(String, String)>,
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
                    semicolon_select: false,
                    select_input: None,
                    bracket_char_pick: false,
                    page_code: None,
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
            mut choice_at: impl FnMut(usize) -> Option<(String, String)>,
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
                semicolon_select,
                select_input,
                bracket_char_pick,
                page_code,
            } = input;
            let Some(code) = self.code(context).map(str::to_owned) else {
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
                        Some(LetterAction::Compose(c)) => self.compose_letter(
                            context, &code, c, wubi, page_code, choice_at, first_for,
                        ),
                        _ => InlineKeyOutcome::NotHandled,
                    }
                }
                0xBA if !shift
                    && shuangpin.is_some_and(|scheme| semicolon_is_code(scheme, &code)) =>
                {
                    self.compose_letter(context, &code, ';', None, page_code, choice_at, first_for)
                }
                _ => {
                    if let Some(outcome) = self.box_key(
                        context,
                        &code,
                        key,
                        shift,
                        comma_period_paging,
                        &mut choice_at,
                    ) {
                        return outcome;
                    }
                    if let Some(outcome) = self.mark_key(
                        context,
                        &code,
                        key,
                        shift,
                        shuangpin,
                        semicolon_select,
                        select_input.or(wubi.map(|(input, _)| input)),
                        &mut choice_at,
                    ) {
                        return outcome;
                    }
                    if let Some(outcome) = self.bracket_key(
                        context,
                        &code,
                        key,
                        shift,
                        bracket_char_pick,
                        &mut choice_at,
                    ) {
                        return outcome;
                    }
                    if let Some(forward) = u16::try_from(key)
                        .ok()
                        .and_then(|vk| paging_direction_vk(vk, shift, comma_period_paging))
                    {
                        return InlineKeyOutcome::Page(forward);
                    }
                    if let Some(text) = typed {
                        let committed = flush_visible(&code, page_code, &mut choice_at, first_for);
                        self.active = None;
                        return InlineKeyOutcome::Finish(committed + &text);
                    }
                    if is_navigation(key) {
                        return InlineKeyOutcome::Swallow;
                    }
                    InlineKeyOutcome::NotHandled
                }
            }
        }

        /// Fifth-letter and unique-four edits from [`edit_code`] (the same steps as
        /// [`crate::Composer::type_letter`]). The candidate text still comes from this
        /// composition's own list.
        #[allow(clippy::too_many_arguments)]
        fn compose_letter(
            &mut self,
            context: &C,
            code: &str,
            letter: char,
            wubi: Option<(&MixedInput, LetterRules)>,
            page_code: Option<&str>,
            choice_at: impl FnOnce(usize) -> Option<(String, String)>,
            first_for: &dyn Fn(&str) -> Option<(String, String)>,
        ) -> InlineKeyOutcome {
            let edit = edit_code(code, letter, wubi);
            if edit.ignored {
                return InlineKeyOutcome::Swallow;
            }
            let mut committed = String::new();
            if edit.fifth && wubi.is_some_and(|(input, _)| input.wubi_count(code) > 0) {
                let on_page = choice_at(0);
                let fresh = first_for(code);
                if let Some((text, _)) = shown_candidate(page_code, code, on_page, fresh) {
                    committed = text;
                }
            }
            if edit.auto
                && let Some((text, _)) = first_for(&edit.code)
            {
                self.active = None;
                return InlineKeyOutcome::Finish(committed + &text);
            }
            self.active = Some((context.clone(), edit.code.clone()));
            if committed.is_empty() {
                InlineKeyOutcome::Update(edit.code)
            } else {
                InlineKeyOutcome::PickPartial {
                    committed,
                    rest: edit.code,
                }
            }
        }

        /// Space, digits, Enter, Esc, Backspace and the character paging keys. The decision is
        /// [`classify_char`], the same function the draft box uses.
        fn box_key(
            &mut self,
            context: &C,
            code: &str,
            key: usize,
            shift: bool,
            comma_period_paging: bool,
            choice_at: &mut impl FnMut(usize) -> Option<(String, String)>,
        ) -> Option<InlineKeyOutcome> {
            let ch = match key {
                0x08 => '\u{8}',
                0x1b => '\u{1b}',
                0x0d => '\r',
                0x20 => ' ',
                0x31..=0x39 if !shift => key as u8 as char,
                0xBD if !shift => '-',
                0xBB if !shift => '=',
                0xBC if !shift && comma_period_paging => ',',
                0xBE if !shift && comma_period_paging => '.',
                _ => return None,
            };
            let action = classify_char(
                ch,
                &KeyContext {
                    composing: true,
                    shift,
                    comma_period_paging,
                    ..KeyContext::default()
                },
            );
            Some(match action {
                KeyAction::Backspace => {
                    let mut next = code.to_owned();
                    next.pop();
                    if next.is_empty() {
                        self.active = None;
                        InlineKeyOutcome::Finish(String::new())
                    } else {
                        self.active = Some((context.clone(), next.clone()));
                        InlineKeyOutcome::Update(next)
                    }
                }
                KeyAction::ClearCode => {
                    self.active = None;
                    InlineKeyOutcome::Finish(String::new())
                }
                KeyAction::CommitRaw => {
                    self.active = None;
                    InlineKeyOutcome::Finish(code.to_owned())
                }
                KeyAction::PickFirstOrRaw => {
                    if let Some((text, rest)) = choice_at(0) {
                        self.apply_choice(context, text, rest)
                    } else {
                        self.active = None;
                        InlineKeyOutcome::Finish(code.to_owned())
                    }
                }
                KeyAction::Pick(index) => {
                    if let Some((text, rest)) = choice_at(index) {
                        self.apply_choice(context, text, rest)
                    } else {
                        InlineKeyOutcome::Swallow
                    }
                }
                KeyAction::Page { forward } => InlineKeyOutcome::Page(forward),
                _ => return None,
            })
        }

        /// `'` continues a pinyin code (`xi'an`). `;` / `'` select the 2nd / 3rd candidate when
        /// that switch is on. 微软双拼's code-key `;` is handled before this. A `;` that selects
        /// nothing falls through to punctuation; a `'` that selects nothing is still a separator.
        #[allow(clippy::too_many_arguments)]
        fn mark_key(
            &mut self,
            context: &C,
            code: &str,
            key: usize,
            shift: bool,
            shuangpin: Option<Shuangpin>,
            semicolon_select: bool,
            input: Option<&MixedInput>,
            choice_at: &mut impl FnMut(usize) -> Option<(String, String)>,
        ) -> Option<InlineKeyOutcome> {
            let ch = match (key, shift) {
                (0xBA, false) => ';',
                (0xDE, false) => '\'',
                _ => return None,
            };
            let selection = if semicolon_select {
                match shuangpin {
                    Some(scheme) => shuangpin_selection(scheme, code, ch),
                    None if ch == ';' => Some(1),
                    None => input.and_then(|input| selection_index(input, code, ch)),
                }
            } else {
                None
            };
            if let Some(index) = selection
                && let Some((text, rest)) = choice_at(index)
            {
                return Some(self.apply_choice(context, text, rest));
            }
            if ch == '\'' {
                return Some(match with_separator(code, MAX_CODE_LEN) {
                    Some(next) => {
                        self.active = Some((context.clone(), next.clone()));
                        InlineKeyOutcome::Update(next)
                    }
                    None => InlineKeyOutcome::Swallow,
                });
            }
            None
        }

        /// `[` `]` take the first or last character of the highlighted candidate. A single
        /// character, or the switch being off, leaves the key to ordinary punctuation.
        fn bracket_key(
            &mut self,
            context: &C,
            _code: &str,
            key: usize,
            shift: bool,
            bracket_char_pick: bool,
            choice_at: &mut impl FnMut(usize) -> Option<(String, String)>,
        ) -> Option<InlineKeyOutcome> {
            if shift || !bracket_char_pick {
                return None;
            }
            let last = match key {
                0xDB => false,
                0xDD => true,
                _ => return None,
            };
            let (text, rest) = choice_at(0)?;
            let ch = char_of_word(&text, last)?;
            Some(if rest.is_empty() {
                self.active = None;
                InlineKeyOutcome::Finish(ch)
            } else {
                self.active = Some((context.clone(), rest.clone()));
                InlineKeyOutcome::PickPartial {
                    committed: ch,
                    rest,
                }
            })
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct BarSlot {
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    /// One page of the in-document candidate bar. Items wrap so the page controls stay inside
    /// `max_width`. A single candidate wider than the row is narrowed to that row.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CandidateBarLayout {
        pub width: i32,
        pub height: i32,
        pub items: Vec<BarSlot>,
        pub prev: BarSlot,
        pub next: BarSlot,
        pub menu: BarSlot,
        pub divider_x: i32,
        pub row_h: i32,
    }

    pub fn layout_candidate_bar(
        item_widths: &[i32],
        row_h: i32,
        gap: i32,
        pad_x: i32,
        slot: i32,
        before_buttons: i32,
        max_width: i32,
    ) -> CandidateBarLayout {
        let row_h = row_h.max(1);
        let gap = gap.max(0);
        let pad_x = pad_x.max(0);
        let slot = slot.max(1);
        let before_buttons = before_buttons.max(0);
        // From the last item's right edge: the gap before the buttons, three slots, right pad.
        let trail = before_buttons + slot * 3 + pad_x;
        let max_width = max_width.max(pad_x + trail + 1);
        let item_limit = (max_width - pad_x - trail).max(1);
        let mut rows: Vec<Vec<(usize, i32)>> = Vec::new();
        let mut current = Vec::new();
        let mut used = pad_x;
        for (index, &natural) in item_widths.iter().enumerate() {
            let w = natural.max(1).min(item_limit);
            let next = if current.is_empty() {
                pad_x + w
            } else {
                used + gap + w
            };
            if !current.is_empty() && next + trail > max_width {
                rows.push(std::mem::take(&mut current));
                current.push((index, w));
                used = pad_x + w;
            } else {
                current.push((index, w));
                used = next;
            }
        }
        rows.push(current);
        let mut items = Vec::with_capacity(item_widths.len());
        let mut width = pad_x + trail;
        for (row_index, row) in rows.iter().enumerate() {
            let y = row_index as i32 * row_h;
            let mut x = pad_x;
            for &(_index, w) in row {
                items.push(BarSlot { x, y, w, h: row_h });
                x += w + gap;
            }
            let content_right = if row.is_empty() { pad_x } else { x - gap };
            width = width.max(content_right + trail).min(max_width);
        }
        let last_y = rows.len().saturating_sub(1) as i32 * row_h;
        let last = rows.last().map(Vec::as_slice).unwrap_or(&[]);
        let mut x = pad_x;
        for &(_, w) in last {
            x += w + gap;
        }
        let content_right = if last.is_empty() { pad_x } else { x - gap };
        let prev_x = content_right + before_buttons;
        let next_x = prev_x + slot;
        let menu_x = next_x + slot;
        CandidateBarLayout {
            width,
            height: rows.len().max(1) as i32 * row_h,
            items,
            prev: BarSlot {
                x: prev_x,
                y: last_y,
                w: slot,
                h: row_h,
            },
            next: BarSlot {
                x: next_x,
                y: last_y,
                w: slot,
                h: row_h,
            },
            menu: BarSlot {
                x: menu_x,
                y: last_y,
                w: slot,
                h: row_h,
            },
            divider_x: content_right + gap / 2,
            row_h,
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
    use crate::{LetterRules, MixedInput, Shuangpin};

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
            semicolon_select: false,
            select_input: None,
            bracket_char_pick: false,
            page_code: None,
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
    fn draft_box_and_inline_share_wubi_letter_edits() {
        use crate::{ComposeOptions, Composer, Learned, LetterRules};
        let input =
            MixedInput::from_rime("...\n王\tgggg\t100\n工\taaaa\t100\n式\taaaa\t90\n", "...\n");
        let options = ComposeOptions {
            page_size: 5,
            traditional: false,
            learn: false,
            fuzzy: false,
            pinyin_first: false,
            common_only: false,
            shuangpin: None,
        };
        let rules = LetterRules {
            push_fifth: true,
            auto_commit4: true,
        };
        let learned = Learned::default();
        let ctx = 1u8;
        let first = |code: &str| {
            input
                .candidates(code, 1)
                .into_iter()
                .next()
                .map(|row| (row.text, row.rest))
        };
        let key = |wubi| InlineKey {
            key: b'G' as usize,
            shift: false,
            caps_lock: false,
            comma_period_paging: false,
            shuangpin: None,
            wubi,
            typed: None,
            semicolon_select: false,
            select_input: None,
            bracket_char_pick: false,
            page_code: None,
        };
        let mut composer = Composer::default();
        for ch in ['g', 'g', 'g'] {
            assert!(
                composer
                    .type_letter(&input, ch, rules, &options, &learned)
                    .is_empty()
            );
        }
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, &composer.code);
        let picks = composer.type_letter(&input, 'g', rules, &options, &learned);
        let outcome = logic.handle_key_with(&ctx, key(Some((&input, rules))), |_| None, &first);
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].text, "王");
        assert!(composer.code.is_empty());
        assert_eq!(outcome, InlineKeyOutcome::Finish("王".into()));
        assert!(!logic.active());

        let mut composer = Composer::default();
        for ch in ['a', 'a', 'a', 'a'] {
            assert!(
                composer
                    .type_letter(&input, ch, rules, &options, &learned)
                    .is_empty()
            );
        }
        assert_eq!(composer.code, "aaaa");
        let shown = composer.candidate_at(0).map(|pick| (pick.text, pick.rest));
        logic.sync_from_composition(&ctx, "aaaa");
        let picks = composer.type_letter(&input, 'g', rules, &options, &learned);
        let outcome = logic.handle_key_with(
            &ctx,
            key(Some((&input, rules))),
            |index| (index == 0).then(|| shown.clone()).flatten(),
            &first,
        );
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].text, "工");
        assert_eq!(composer.code, "g");
        assert_eq!(
            outcome,
            InlineKeyOutcome::PickPartial {
                committed: "工".into(),
                rest: "g".into(),
            }
        );

        let off = LetterRules {
            push_fifth: false,
            auto_commit4: false,
        };
        let mut composer = Composer::default();
        for ch in ['g', 'g', 'g', 'g'] {
            assert!(
                composer
                    .type_letter(&input, ch, off, &options, &learned)
                    .is_empty()
            );
        }
        logic.sync_from_composition(&ctx, "ggg");
        let outcome = logic.handle_key_with(&ctx, key(Some((&input, off))), |_| None, &first);
        assert_eq!(composer.code, "gggg");
        assert_eq!(outcome, InlineKeyOutcome::Update("gggg".into()));
    }

    #[test]
    fn shared_composing_keys_follow_the_draft_box() {
        let ctx = 1u8;
        let key = |vk| InlineKey {
            key: vk,
            shift: false,
            caps_lock: false,
            comma_period_paging: true,
            shuangpin: None,
            wubi: None,
            typed: None,
            semicolon_select: false,
            select_input: None,
            bracket_char_pick: false,
            page_code: None,
        };
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key(0x20),
                |_| Some(("你".into(), String::new())),
                &|_| None
            ),
            InlineKeyOutcome::Finish("你".into())
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x20), |_| None, &|_| None),
            InlineKeyOutcome::Finish("ni".into())
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x0d), |_| None, &|_| None),
            InlineKeyOutcome::Finish("ni".into())
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x1b), |_| None, &|_| None),
            InlineKeyOutcome::Finish(String::new())
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(&ctx, key(0x08), |_| None, &|_| None),
            InlineKeyOutcome::Update("n".into())
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(&ctx, key(0xBD), |_| None, &|_| None),
            InlineKeyOutcome::Page(false)
        );
        logic.sync_from_composition(&ctx, "ni");
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key(0x32),
                |index| (index == 1).then_some(("好".into(), String::new())),
                &|_| None
            ),
            InlineKeyOutcome::Finish("好".into())
        );
    }

    fn sample_key(vk: usize) -> InlineKey<'static> {
        InlineKey {
            key: vk,
            shift: false,
            caps_lock: false,
            comma_period_paging: false,
            shuangpin: None,
            wubi: None,
            typed: None,
            semicolon_select: false,
            select_input: None,
            bracket_char_pick: false,
            page_code: None,
        }
    }

    #[test]
    fn apostrophe_continues_pinyin_semicolon_selects_and_shuangpin_keeps_its_code_key() {
        let ctx = 1u8;
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "xi");
        assert!(logic.wants(&ctx, 0xDE, false, false, None));
        let mut key = sample_key(0xDE);
        key.typed = Some("‘".into());
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("西".into(), String::new())), &|_| {
                None
            }),
            InlineKeyOutcome::Update("xi'".into())
        );
        assert_eq!(logic.code(&ctx), Some("xi'"));
        assert_eq!(
            logic.handle_key_with(&ctx, sample_key(b'A' as usize), |_| None, &|_| None),
            InlineKeyOutcome::Update("xi'a".into())
        );
        assert_eq!(
            logic.handle_key_with(&ctx, sample_key(b'N' as usize), |_| None, &|_| None),
            InlineKeyOutcome::Update("xi'an".into())
        );

        logic.sync_from_composition(&ctx, "ggg");
        let mut semi = sample_key(0xBA);
        semi.semicolon_select = true;
        semi.typed = Some("；".into());
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                semi,
                |index| (index == 1).then_some(("工".into(), String::new())),
                &|_| None
            ),
            InlineKeyOutcome::Finish("工".into())
        );

        logic.sync_from_composition(&ctx, "b");
        let mut code_key = sample_key(0xBA);
        code_key.shuangpin = Some(Shuangpin::Microsoft);
        code_key.semicolon_select = true;
        code_key.typed = Some("；".into());
        assert!(logic.wants(&ctx, 0xBA, false, false, Some(Shuangpin::Microsoft)));
        assert_eq!(
            logic.handle_key_with(&ctx, code_key, |_| None, &|_| None),
            InlineKeyOutcome::Update("b;".into())
        );
    }

    #[test]
    fn punctuation_commits_the_current_page_then_the_rest() {
        let ctx = 1u8;
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "xian");
        let mut key = sample_key(0x31);
        key.shift = true;
        key.typed = Some("！".into());
        key.page_code = Some("xian");
        let first = |code: &str| match code {
            "xian" => Some(("先".into(), String::new())),
            "an" => Some(("安".into(), String::new())),
            _ => None,
        };
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("西安".into(), String::new())), &first),
            InlineKeyOutcome::Finish("西安！".into())
        );

        logic.sync_from_composition(&ctx, "xian");
        let mut key = sample_key(0x31);
        key.shift = true;
        key.typed = Some("！".into());
        key.page_code = Some("xian");
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("西".into(), "an".into())), &first),
            InlineKeyOutcome::Finish("西安！".into())
        );
    }

    #[test]
    fn fifth_letter_uses_the_page_only_when_it_shows_that_code() {
        let ctx = 1u8;
        let input =
            MixedInput::from_rime("...\n王\tgggg\t100\n工\taaaa\t100\n式\taaaa\t90\n", "...\n");
        let rules = LetterRules {
            push_fifth: true,
            auto_commit4: false,
        };
        let fresh = |code: &str| {
            input
                .candidates(code, 1)
                .into_iter()
                .next()
                .map(|row| (row.text, row.rest))
        };
        let mut logic = LogicalInline::default();
        // Fourth key is in the logical code; the candidate page is still the third key.
        logic.sync_from_composition(&ctx, "gggg");
        let mut key = sample_key(b'G' as usize);
        key.wubi = Some((&input, rules));
        key.page_code = Some("ggg");
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("工".into(), String::new())), &fresh),
            InlineKeyOutcome::PickPartial {
                committed: "王".into(),
                rest: "g".into(),
            }
        );
        assert_eq!(logic.code(&ctx), Some("g"));

        // The page caught up but has no rows yet: still look up the four letters.
        logic.sync_from_composition(&ctx, "gggg");
        let mut key = sample_key(b'G' as usize);
        key.wubi = Some((&input, rules));
        key.page_code = Some("gggg");
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| None, &fresh),
            InlineKeyOutcome::PickPartial {
                committed: "王".into(),
                rest: "g".into(),
            }
        );

        // A page that really is the four-letter code keeps its highlighted candidate.
        logic.sync_from_composition(&ctx, "aaaa");
        let mut key = sample_key(b'G' as usize);
        key.wubi = Some((&input, rules));
        key.page_code = Some("aaaa");
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("式".into(), String::new())), &fresh),
            InlineKeyOutcome::PickPartial {
                committed: "式".into(),
                rest: "g".into(),
            }
        );
    }

    #[test]
    fn brackets_take_a_character_unless_the_candidate_is_one_or_the_switch_is_off() {
        let ctx = 1u8;
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "nihao");
        let mut key = sample_key(0xDD);
        key.bracket_char_pick = true;
        key.typed = Some("】".into());
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key,
                |_| Some(("你好".into(), String::new())),
                &|_| None
            ),
            InlineKeyOutcome::Finish("好".into())
        );

        logic.sync_from_composition(&ctx, "ni");
        let mut key = sample_key(0xDD);
        key.bracket_char_pick = true;
        key.typed = Some("】".into());
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key,
                |_| Some(("你".into(), String::new())),
                &|code: &str| (code == "ni").then_some(("你".into(), String::new()))
            ),
            InlineKeyOutcome::Finish("你】".into())
        );

        logic.sync_from_composition(&ctx, "nihao");
        let mut key = sample_key(0xDD);
        key.bracket_char_pick = false;
        key.typed = Some("】".into());
        assert_eq!(
            logic.handle_key_with(
                &ctx,
                key,
                |_| Some(("你好".into(), String::new())),
                &|code: &str| (code == "nihao").then_some(("你好".into(), String::new()))
            ),
            InlineKeyOutcome::Finish("你好】".into())
        );
    }

    #[test]
    fn english_punctuation_uses_the_layout_character_once() {
        assert_eq!(crate::typed_from_layout(Some('@'), 0x32, true), Some('@'));
        assert_eq!(crate::typed_from_layout(Some('"'), 0x32, true), Some('"'));
        assert_eq!(crate::typed_from_layout(None, 0x32, true), Some('@'));

        let ctx = 1u8;
        let mut logic = LogicalInline::default();
        logic.sync_from_composition(&ctx, "ni");
        let mut key = sample_key(0x32);
        key.shift = true;
        key.typed = Some("\"".into());
        assert_eq!(
            logic.handle_key_with(&ctx, key, |_| Some(("你".into(), String::new())), &|_| {
                Some(("你".into(), String::new()))
            }),
            InlineKeyOutcome::Finish("你\"".into())
        );
        assert!(!logic.active());
    }

    #[test]
    fn a_wide_page_keeps_the_last_candidate_and_the_page_buttons_on_screen() {
        let layout = layout_candidate_bar(&[80, 80, 80, 80], 28, 16, 8, 18, 12, 160);
        assert!(layout.width <= 160);
        assert!(layout.items.len() == 4);
        assert!(layout.height > 28);
        let last = layout.items.last().unwrap();
        assert!(last.x + last.w <= layout.width);
        assert!(layout.next.x + layout.next.w <= layout.width);
        assert!(layout.prev.x + layout.prev.w <= layout.width);
        assert_ne!(layout.items[0].y, layout.items[3].y);

        let one_row = layout_candidate_bar(&[40, 40], 28, 16, 8, 18, 12, 400);
        assert_eq!(one_row.height, 28);
        assert_eq!(one_row.items[0].y, one_row.items[1].y);
        assert!(one_row.next.x > one_row.items[1].x);
    }

    #[test]
    fn redraw_publishes_the_click_rects_it_draws() {
        let mut view = CandidateView::empty(5);
        view.bar_height = 28;
        view.item_spans = vec![(8, 40), (48, 80)];
        let drawn = [(8, 0, 72, 56), (88, 0, 160, 56)];
        view.publish_click_rects(&drawn, (170, 0, 188, 56), (188, 0, 206, 56), 56);
        assert_eq!(view.hit_test(100, 10), Some(HitTarget::Candidate(1)));
        assert_eq!(view.hit_test(20, 10), Some(HitTarget::Candidate(0)));
        assert_eq!(view.hit_test(100, 60), None);
        // The 100% span ended at 80, so this point only belongs to the redrawn second item.
        assert!(view.item_spans[1].1 > 80);
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
