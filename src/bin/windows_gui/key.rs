use super::*;
use yiime_core::KeyAction;

pub(super) unsafe fn set_code(hwnd: HWND, state: &State, code: String) {
    state.composition.borrow_mut().set_code(
        &state.input.borrow(),
        code,
        &compose_options(),
        &state.learned.borrow(),
    );
    unsafe {
        relayout(hwnd, state);
    }
}
pub(super) unsafe fn insert_text(state: &State, text: &str) {
    unsafe {
        SendMessageW(state.edit, EM_REPLACESEL, 1, wide(text).as_ptr() as isize);
    }
}
/// Writes the picked candidate or raw letters into the draft and ends this composition.
unsafe fn commit_into_draft(hwnd: HWND, state: &State, text: &str) {
    unsafe {
        set_code(hwnd, state, String::new());
        insert_text(state, text);
    }
}
/// Picks candidate `index` on this page; false if there is none. A leading-syllable candidate
/// (中 in zhonggou) leaves the remaining code for the next pick.
pub(super) unsafe fn pick(hwnd: HWND, state: &State, index: usize) -> bool {
    let pick = state.composition.borrow().candidate_at(index);
    match pick {
        Some(yiime_core::Pick { code, text, rest }) => {
            remember_pick(state, &code, &text);
            unsafe {
                if rest.is_empty() {
                    commit_into_draft(hwnd, state, &text);
                } else {
                    insert_text(state, &text);
                    set_code(hwnd, state, rest);
                }
            }
            true
        }
        None => false,
    }
}
/// `[` `]`: commit the first/last character of the first candidate (a partial candidate leaves
/// the remaining syllables).
unsafe fn pick_char_of_word(hwnd: HWND, state: &State, last: bool) -> bool {
    let pick = state.composition.borrow().candidate_at(0);
    let Some(pick) = pick else {
        return false;
    };
    let Some(text) = yiime_core::char_of_word(&pick.text, last) else {
        return false;
    };
    unsafe {
        if pick.rest.is_empty() {
            commit_into_draft(hwnd, state, &text);
        } else {
            insert_text(state, &text);
            set_code(hwnd, state, pick.rest);
        }
    }
    true
}
/// Space: pick the first candidate (a partial candidate leaves the remaining syllables), or
/// commit the raw letters if there is none.
unsafe fn pick_first_or_raw(hwnd: HWND, state: &State) {
    if unsafe { !pick(hwnd, state, 0) } {
        let raw = state.composition.borrow().code.clone();
        unsafe {
            commit_into_draft(hwnd, state, &raw);
        }
    }
}
/// Punctuation, Shift+letter, Ctrl+Enter: commit the whole code, picking the first candidate
/// segment by segment until it is used up (shared with macOS: [`yiime_core::Composer::flush_all`]).
unsafe fn flush_composition(hwnd: HWND, state: &State) {
    if state.composition.borrow().code.is_empty() {
        return;
    }
    let steps = state.composition.borrow_mut().flush_all(
        &state.input.borrow(),
        &compose_options(),
        &state.learned.borrow(),
    );
    for step in steps {
        match step {
            yiime_core::Flushed::Pick(pick) => {
                remember_pick(state, &pick.code, &pick.text);
                unsafe { insert_text(state, &pick.text) };
            }
            yiime_core::Flushed::Raw(raw) => unsafe { insert_text(state, &raw) },
        }
    }
    unsafe { set_code(hwnd, state, String::new()) };
}
unsafe fn turn_page(hwnd: HWND, state: &State, forward: bool) {
    state.composition.borrow_mut().turn_page(forward);
    unsafe {
        relayout(hwnd, state);
    }
}
/// Character before the caret, to keep `.` `:` half-width after digits (`3.5`, `10:30`).
unsafe fn char_before_caret(edit: HWND) -> Option<u16> {
    let mut start = 0u32;
    unsafe {
        SendMessageW(edit, EM_GETSEL, &mut start as *mut u32 as usize, 0);
    }
    let text: Vec<u16> = unsafe { get_text(edit) }.encode_utf16().collect();
    (start as usize)
        .checked_sub(1)
        .and_then(|index| text.get(index).copied())
}
/// Character after the caret, to skip over a closing mark when auto-closing.
unsafe fn char_after_caret(edit: HWND) -> Option<u16> {
    let mut end = 0u32;
    unsafe {
        SendMessageW(edit, EM_GETSEL, 0, &mut end as *mut u32 as isize);
    }
    let text: Vec<u16> = unsafe { get_text(edit) }.encode_utf16().collect();
    text.get(end as usize).copied()
}
/// Moves the caret by `delta` UTF-16 units (unchecked).
unsafe fn move_caret(edit: HWND, delta: isize) {
    let mut end = 0u32;
    unsafe {
        SendMessageW(edit, EM_GETSEL, 0, &mut end as *mut u32 as isize);
        let len = GetWindowTextLengthW(edit) as isize;
        let at = (end as isize + delta).clamp(0, len) as usize;
        SendMessageW(edit, EM_SETSEL, at, at as isize);
    }
}
unsafe fn insert_punctuation(state: &State, ch: char) -> bool {
    if option(Flag::PunctEnglish) {
        return false;
    }
    let mut quotes = state.quotes.get();
    let after_digit = || {
        unsafe { char_before_caret(state.edit) }
            .is_some_and(|unit| (b'0' as u16..=b'9' as u16).contains(&unit))
    };
    let text = quotes
        .next(ch)
        .or_else(|| yiime_core::full_width(ch, after_digit()));
    state.quotes.set(quotes);
    let Some(text) = text else {
        return false;
    };
    if option(Flag::AutoPair) {
        // Auto-close: insert the opening and closing marks with the caret between them; a
        // closing mark right after the caret is skipped over.
        if yiime_core::is_closing(text)
            && unsafe { char_after_caret(state.edit) } == text.encode_utf16().next()
        {
            unsafe {
                move_caret(state.edit, 1);
            }
            return true;
        }
        if let Some(close) = yiime_core::closing_pair(text) {
            unsafe {
                insert_text(state, &format!("{text}{close}"));
                move_caret(state.edit, -(close.encode_utf16().count() as isize));
            }
            return true;
        }
    }
    unsafe {
        insert_text(state, text);
    }
    true
}
/// Character input in the box. True means handled, so the EDIT control does not see it. The
/// decision is made by the platform-independent [`yiime_core::classify_char`]; this only carries
/// it out.
pub(super) unsafe fn handle_char(hwnd: HWND, state: &State, unit: u32) -> bool {
    let Some(ch) = char::from_u32(unit) else {
        return false;
    };
    // The numpad flag belongs to this one character (Ctrl+Enter does not consume it).
    let numpad = ch != '\n' && state.numpad.replace(false);
    let code = state.composition.borrow().code.clone();
    let composing = !code.is_empty();
    if !composing {
        // A new code: pick up user words imported or edited since the last one.
        refresh_user_words(state);
    }
    let shuangpin = OPTIONS.shuangpin();
    let selection = match shuangpin {
        _ if !composing || !option(Flag::SemicolonSelect) => None,
        Some(scheme) => yiime_core::shuangpin_selection(scheme, &code, ch),
        None => selection_index(&state.input.borrow(), &code, ch),
    };
    let context = yiime_core::KeyContext {
        composing,
        caps_lock: unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0,
        shift: unsafe { GetKeyState(VK_SHIFT as i32) } < 0,
        english: state.english.get(),
        numpad,
        selection,
        comma_period_paging: option(Flag::CommaPeriodPaging),
        bracket_char_pick: option(Flag::BracketCharPick),
        semicolon_is_code: composing
            && shuangpin.is_some_and(|scheme| yiime_core::semicolon_is_code(scheme, &code)),
    };
    unsafe { run_key_action(hwnd, state, ch, yiime_core::classify_char(ch, &context)) }
}
/// Carries out one [`KeyAction`]; true means handled.
unsafe fn run_key_action(hwnd: HWND, state: &State, ch: char, action: KeyAction) -> bool {
    match action {
        KeyAction::Pass => false,
        KeyAction::FlushThenPass => {
            unsafe { flush_composition(hwnd, state) };
            false
        }
        KeyAction::Ignore => true,
        KeyAction::SubmitChinese => {
            // A pending code first puts its first candidate (or raw letters) into the draft. The
            // box never inserts a newline.
            unsafe {
                flush_composition(hwnd, state);
                submit(hwnd, state, true);
            }
            true
        }
        KeyAction::Submit { chinese } => {
            unsafe { submit(hwnd, state, chinese) };
            true
        }
        KeyAction::Cancel => {
            unsafe { cancel_session(hwnd, state) };
            true
        }
        // Code letter. Fifth-letter, empty-code and unique-four-letter rules live in the core.
        // In plain mode a commit produced here goes out together with the next one (flushing
        // now would end the session and lose the new letter).
        KeyAction::Compose(letter) => {
            let rules = LetterRules {
                push_fifth: option(Flag::PushFifth),
                auto_commit4: option(Flag::AutoCommit4),
            };
            let picks = {
                let learned = state.learned.borrow();
                state.composition.borrow_mut().type_letter(
                    &state.input.borrow(),
                    letter,
                    rules,
                    &compose_options(),
                    &learned,
                )
            };
            for pick in picks {
                remember_pick(state, &pick.code, &pick.text);
                unsafe {
                    insert_text(state, &pick.text);
                }
            }
            unsafe {
                relayout(hwnd, state);
            }
            true
        }
        // Other punctuation: commit the first candidate, then type the (full-width) mark, as
        // common Chinese IMEs do.
        KeyAction::Punctuation { flush_first } => {
            if flush_first {
                unsafe { flush_composition(hwnd, state) };
            }
            unsafe { insert_punctuation(state, ch) }
        }
        // `;` picks the 2nd candidate and `'` the 3rd (Microsoft Wubi).
        KeyAction::Select { index, otherwise } => unsafe {
            pick(hwnd, state, index) || run_key_action(hwnd, state, ch, *otherwise)
        },
        KeyAction::Pick(index) => {
            unsafe { pick(hwnd, state, index) };
            true
        }
        KeyAction::PickFirstOrRaw => {
            unsafe { pick_first_or_raw(hwnd, state) };
            true
        }
        KeyAction::Separator => {
            let code = state.composition.borrow().code.clone();
            if let Some(code) = yiime_core::with_separator(&code, MAX_CODE_LEN) {
                unsafe { set_code(hwnd, state, code) };
            }
            true
        }
        KeyAction::CommitRaw => {
            let raw = state.composition.borrow().code.clone();
            unsafe { commit_into_draft(hwnd, state, &raw) };
            true
        }
        KeyAction::ClearCode => {
            unsafe { set_code(hwnd, state, String::new()) };
            true
        }
        KeyAction::Backspace => {
            let mut code = state.composition.borrow().code.clone();
            code.pop();
            unsafe { set_code(hwnd, state, code) };
            true
        }
        KeyAction::Page { forward } => {
            unsafe { turn_page(hwnd, state, forward) };
            true
        }
        // `[` first character, `]` last character of the first candidate; with a
        // single-character candidate they are plain punctuation.
        KeyAction::CharOfWord { last, otherwise } => unsafe {
            pick_char_of_word(hwnd, state, last) || run_key_action(hwnd, state, ch, *otherwise)
        },
    }
}
pub(super) unsafe extern "system" fn edit_proc(
    edit: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let previous = unsafe { GetWindowLongPtrW(edit, GWLP_USERDATA) };
    let call_previous = || unsafe {
        CallWindowProcW(
            std::mem::transmute::<isize, WNDPROC>(previous),
            edit,
            msg,
            w,
            l,
        )
    };
    let hwnd = unsafe { GetParent(edit) };
    let Some(state) = (unsafe { state_of(hwnd) }) else {
        return call_previous();
    };
    // Shift tap (pressed and released alone) toggles the English segment.
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        state.numpad.set(yiime_core::is_numpad_digit(w as u32));
        let held = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        let mut tap = state.shift_tap.get();
        tap.key_down(
            w as u32,
            held(VK_CONTROL) || held(VK_MENU) || held(VK_LWIN) || held(VK_RWIN),
            std::time::Instant::now(),
        );
        state.shift_tap.set(tap);
    } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
        let mut tap = state.shift_tap.get();
        let tapped = tap.key_up(w as u32, std::time::Instant::now());
        state.shift_tap.set(tap);
        if tapped && !state.pending.get() {
            let action = yiime_core::shift_tap_action(
                state.tsf_service && !state.tsf_receiver.get().is_null(),
                state.composition.borrow().code.is_empty(),
                unsafe { GetWindowTextLengthW(state.edit) } == 0,
                state.english.get(),
            );
            unsafe {
                match action {
                    yiime_core::ShiftTapAction::ToggleSegment => toggle_inline_english(hwnd, state),
                    yiime_core::ShiftTapAction::SwitchToEnglish => switch_to_english(hwnd, state),
                }
            }
        }
    }
    match msg {
        WM_GETDLGCODE => return call_previous() | DLGC_WANTALLKEYS as isize,
        WM_CHAR => {
            if state.swallow_char.replace(false) && w <= 0x20 {
                return 0;
            }
            if unsafe { handle_char(hwnd, state, w as u32) } {
                unsafe {
                    flush_plain(hwnd, state);
                }
                return 0;
            }
            let result = call_previous();
            unsafe {
                flush_plain(hwnd, state);
            }
            return result;
        }
        WM_KEYDOWN
            if unsafe { GetKeyState(VK_CONTROL as i32) } < 0
                && unsafe { GetKeyState(VK_MENU as i32) } >= 0
                && (w == 0xBE
                    || (w == b'F' as usize && unsafe { GetKeyState(VK_SHIFT as i32) } < 0)) =>
        {
            // Ctrl+.: Chinese/English punctuation; Ctrl+Shift+F: Simplified/Traditional output
            // (Microsoft Wubi shortcuts).
            let (option_key, on_text, off_text) = if w == 0xBE {
                (Flag::PunctEnglish, "英文标点", "中文标点")
            } else {
                (Flag::Traditional, "繁体输出", "简体输出")
            };
            let on = !option(option_key);
            set_option(option_key, on);
            state.swallow_char.set(true);
            let code = state.composition.borrow().code.clone();
            unsafe {
                set_code(hwnd, state, code);
                set_status(
                    hwnd,
                    state,
                    Some(((if on { on_text } else { off_text }).to_owned(), false)),
                );
            }
            return 0;
        }
        // Ctrl+Shift+digit: forget the learned pick of that candidate under the current code
        // (Microsoft Pinyin convention).
        WM_KEYDOWN
            if (0x31..=0x39).contains(&w)
                && unsafe { GetKeyState(VK_CONTROL as i32) } < 0
                && unsafe { GetKeyState(VK_SHIFT as i32) } < 0
                && unsafe { GetKeyState(VK_MENU as i32) } >= 0
                && !state.composition.borrow().code.is_empty() =>
        {
            state.swallow_char.set(true);
            let pick = state.composition.borrow().candidate_at(w - 0x31);
            if let Some(pick) = pick {
                let forgot = state.learned.borrow_mut().forget(&pick.code, &pick.text);
                let code = state.composition.borrow().code.clone();
                let text = if forgot {
                    format!("已删除“{}”的学习记录", pick.text)
                } else {
                    format!("“{}”没有学习记录（词库词不删除）", pick.text)
                };
                unsafe {
                    set_code(hwnd, state, code);
                    set_status(hwnd, state, Some((text, false)));
                }
            }
            return 0;
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if is_plain_toggle(w as u16) => {
            // The mode can also be toggled inside the box; the code and draft stay and the hint
            // line updates.
            set_plain_mode(!plain_mode());
            // Only Ctrl combinations (without Alt) produce a following WM_CHAR (space or
            // control character).
            state.swallow_char.set(unsafe {
                GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_MENU as i32) >= 0
            });
            unsafe {
                relayout(hwnd, state);
                flush_plain(hwnd, state);
            }
            return 0;
        }
        // With no code and an empty draft (or a recalled entry unchanged): ↑/PgUp shows older
        // history, ↓/PgDn newer.
        WM_KEYDOWN
            if state.composition.borrow().code.is_empty()
                && [VK_UP, VK_DOWN, VK_PRIOR, VK_NEXT].contains(&(w as u16))
                && option(Flag::HistoryEnabled) =>
        {
            let draft = unsafe { get_text(state.edit) };
            let current = state.history_pos.get();
            let untouched = draft.is_empty()
                || current.is_some_and(|index| {
                    load_history()
                        .get(index)
                        .is_some_and(|entry| entry.chinese == draft)
                });
            if untouched {
                let older = [VK_UP, VK_PRIOR].contains(&(w as u16));
                let next = match (current, older) {
                    (None, true) => Some(0),
                    (None, false) => None,
                    (Some(index), true) => Some(index + 1),
                    (Some(index), false) => index.checked_sub(1),
                };
                if let Some(index) = next
                    && unsafe { show_history(hwnd, state, index) }
                {
                    state.recall_session.set(true);
                }
                return 0;
            }
        }
        WM_KEYDOWN if !state.composition.borrow().code.is_empty() => {
            let key = w as u16;
            if key == VK_PRIOR || key == VK_NEXT {
                unsafe {
                    turn_page(hwnd, state, key == VK_NEXT);
                }
                return 0;
            }
            if [
                VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_HOME, VK_END, VK_DELETE,
            ]
            .contains(&key)
            {
                return 0;
            }
        }
        _ => {}
    }
    call_previous()
}
