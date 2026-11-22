//! Character-key state machine of the input box: decides what a typed character does from the
//! composition state and the switches, without touching any window. The platform layer only
//! carries out the returned [`KeyAction`] (Windows: `src/bin/windows_gui/key.rs`), so the rules are
//! unit-tested here.

use crate::engine::{LetterAction, classify_letter, paging_direction};
use crate::inline::is_literal_in_english;

/// State and switches the decision depends on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyContext {
    /// A code is being composed.
    pub composing: bool,
    pub caps_lock: bool,
    /// Shift is held (Shift+Enter commits the Chinese draft).
    pub shift: bool,
    /// Inside an inline English segment (Shift tap).
    pub english: bool,
    /// The key came from the numeric keypad.
    pub numpad: bool,
    /// What `;`/`'` select under [`crate::Flag::SemicolonSelect`] for this code (see
    /// [`crate::selection_index`]); None when the switch is off or the key does not select.
    pub selection: Option<usize>,
    pub comma_period_paging: bool,
    pub bracket_char_pick: bool,
    /// `;` is a code key here (微软双拼's -ing after an initial key).
    pub semicolon_is_code: bool,
}

/// What a character does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Let the edit control insert the character as typed.
    Pass,
    /// Commit the code (first candidates, segment by segment), then let the character through.
    FlushThenPass,
    /// Swallow the key.
    Ignore,
    /// Ctrl+Enter: commit the code, then submit the draft as Chinese.
    SubmitChinese,
    /// Enter outside composition: submit (translate, or Chinese with Shift).
    Submit {
        chinese: bool,
    },
    /// Esc outside composition: cancel the session.
    Cancel,
    /// A code letter for the engine (fifth-letter and unique-code rules live there).
    Compose(char),
    /// Insert the (full-width) punctuation for the character, committing the code first when
    /// `flush_first`; if there is no mapping, the character goes through as typed.
    Punctuation {
        flush_first: bool,
    },
    /// Pick candidate `index` on this page; if there is none, do `otherwise`.
    Select {
        index: usize,
        otherwise: Box<KeyAction>,
    },
    /// Pick candidate `index` on this page (nothing if there is none).
    Pick(usize),
    /// Space: the first candidate, or the raw letters if there is none.
    PickFirstOrRaw,
    /// Add a `'` syllable separator.
    Separator,
    /// Enter while composing: the raw letters.
    CommitRaw,
    /// Esc while composing: clear the code.
    ClearCode,
    /// Backspace while composing.
    Backspace,
    Page {
        forward: bool,
    },
    /// `[` `]`: first/last character of the first candidate; with a single character, do
    /// `otherwise`.
    CharOfWord {
        last: bool,
        otherwise: Box<KeyAction>,
    },
}

/// Decides what character `ch` does in the input box.
pub fn classify_char(ch: char, context: &KeyContext) -> KeyAction {
    if ch == '\n' {
        return KeyAction::SubmitChinese;
    }
    // Numpad digits never pick candidates.
    if context.numpad && ch.is_ascii_digit() {
        return KeyAction::FlushThenPass;
    }
    if context.english && !context.composing && is_literal_in_english(ch) {
        return KeyAction::Pass;
    }
    match classify_letter(ch, context.caps_lock, context.composing) {
        Some(LetterAction::Raw { flush_first: true }) => return KeyAction::FlushThenPass,
        Some(LetterAction::Raw { flush_first: false }) => return KeyAction::Pass,
        Some(LetterAction::Compose(letter)) => return KeyAction::Compose(letter),
        None => {}
    }
    if context.composing && ch == ';' && context.semicolon_is_code {
        return KeyAction::Compose(';');
    }
    if !context.composing {
        return match ch {
            '\r' => KeyAction::Submit {
                chinese: context.shift,
            },
            '\u{1b}' => KeyAction::Cancel,
            '\t' => KeyAction::Ignore,
            _ if !context.caps_lock => KeyAction::Punctuation { flush_first: false },
            _ => KeyAction::Pass,
        };
    }
    let composing = composing_action(ch, context);
    match context.selection {
        Some(index) => KeyAction::Select {
            index,
            otherwise: Box::new(composing),
        },
        None => composing,
    }
}

/// While composing, without `;`/`'` selection.
fn composing_action(ch: char, context: &KeyContext) -> KeyAction {
    let other = || {
        if (ch as u32) < 0x20 {
            KeyAction::Ignore
        } else if context.caps_lock {
            KeyAction::FlushThenPass
        } else {
            KeyAction::Punctuation { flush_first: true }
        }
    };
    match ch {
        '\'' => KeyAction::Separator,
        ' ' => KeyAction::PickFirstOrRaw,
        '1'..='9' => KeyAction::Pick(ch as usize - '1' as usize),
        '\r' => KeyAction::CommitRaw,
        '\u{1b}' => KeyAction::ClearCode,
        '\u{8}' => KeyAction::Backspace,
        _ if paging_direction(ch, context.comma_period_paging).is_some() => KeyAction::Page {
            forward: paging_direction(ch, context.comma_period_paging) == Some(true),
        },
        '[' | ']' if context.bracket_char_pick => KeyAction::CharOfWord {
            last: ch == ']',
            otherwise: Box::new(other()),
        },
        _ => other(),
    }
}

/// What `;`/`'` select while composing `code` (see [`KeyContext::selection`]): with a Shuangpin
/// layout `'` is never a separator, so it always picks the 3rd candidate, and 微软双拼's `;` after
/// an initial key is a code key instead.
pub fn shuangpin_selection(scheme: crate::Shuangpin, code: &str, ch: char) -> Option<usize> {
    match ch {
        ';' if semicolon_is_code(scheme, code) => None,
        ';' => Some(1),
        '\'' => Some(2),
        _ => None,
    }
}

/// Whether `;` typed after `code` is a code key: 微软双拼 with an odd number of keys.
pub fn semicolon_is_code(scheme: crate::Shuangpin, code: &str) -> bool {
    scheme.is_key(';') && code.replace('\'', "").len() % 2 == 1
}

/// Code after typing a `'` separator: none twice in a row, and within `max_len`.
pub fn with_separator(code: &str, max_len: usize) -> Option<String> {
    (!code.ends_with('\'') && code.len() < max_len).then(|| format!("{code}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composing() -> KeyContext {
        KeyContext {
            composing: true,
            bracket_char_pick: true,
            ..KeyContext::default()
        }
    }

    #[test]
    fn idle_box_keys() {
        let idle = KeyContext::default();
        assert_eq!(classify_char('\n', &idle), KeyAction::SubmitChinese);
        assert_eq!(
            classify_char('\r', &idle),
            KeyAction::Submit { chinese: false }
        );
        let shift = KeyContext {
            shift: true,
            ..idle
        };
        assert_eq!(
            classify_char('\r', &shift),
            KeyAction::Submit { chinese: true }
        );
        assert_eq!(classify_char('\u{1b}', &idle), KeyAction::Cancel);
        assert_eq!(classify_char('\t', &idle), KeyAction::Ignore);
        assert_eq!(classify_char('a', &idle), KeyAction::Compose('a'));
        assert_eq!(classify_char('A', &idle), KeyAction::Pass);
        assert_eq!(
            classify_char(',', &idle),
            KeyAction::Punctuation { flush_first: false }
        );
        let caps = KeyContext {
            caps_lock: true,
            ..idle
        };
        assert_eq!(classify_char('a', &caps), KeyAction::Pass);
        assert_eq!(classify_char(',', &caps), KeyAction::Pass);
        // Inline English: everything printable is typed as is.
        let english = KeyContext {
            english: true,
            ..idle
        };
        assert_eq!(classify_char('a', &english), KeyAction::Pass);
        assert_eq!(classify_char(',', &english), KeyAction::Pass);
        assert_eq!(
            classify_char('\r', &english),
            KeyAction::Submit { chinese: false }
        );
        // Numpad digits never pick.
        let numpad = KeyContext {
            numpad: true,
            ..composing()
        };
        assert_eq!(classify_char('1', &numpad), KeyAction::FlushThenPass);
    }

    #[test]
    fn composing_keys() {
        let context = composing();
        assert_eq!(classify_char('b', &context), KeyAction::Compose('b'));
        assert_eq!(classify_char('B', &context), KeyAction::FlushThenPass);
        assert_eq!(classify_char(' ', &context), KeyAction::PickFirstOrRaw);
        assert_eq!(classify_char('3', &context), KeyAction::Pick(2));
        assert_eq!(
            classify_char('0', &context),
            KeyAction::Punctuation { flush_first: true }
        );
        assert_eq!(classify_char('\r', &context), KeyAction::CommitRaw);
        assert_eq!(classify_char('\u{1b}', &context), KeyAction::ClearCode);
        assert_eq!(classify_char('\u{8}', &context), KeyAction::Backspace);
        assert_eq!(classify_char('\'', &context), KeyAction::Separator);
        assert_eq!(
            classify_char('=', &context),
            KeyAction::Page { forward: true }
        );
        assert_eq!(
            classify_char('-', &context),
            KeyAction::Page { forward: false }
        );
        assert_eq!(
            classify_char('.', &context),
            KeyAction::Punctuation { flush_first: true }
        );
        let paging = KeyContext {
            comma_period_paging: true,
            ..context
        };
        assert_eq!(
            classify_char('.', &paging),
            KeyAction::Page { forward: true }
        );
        assert_eq!(
            classify_char(',', &paging),
            KeyAction::Page { forward: false }
        );
        assert_eq!(
            classify_char(']', &context),
            KeyAction::CharOfWord {
                last: true,
                otherwise: Box::new(KeyAction::Punctuation { flush_first: true })
            }
        );
        let no_brackets = KeyContext {
            bracket_char_pick: false,
            ..context
        };
        assert_eq!(
            classify_char('[', &no_brackets),
            KeyAction::Punctuation { flush_first: true }
        );
        assert_eq!(classify_char('\u{7}', &context), KeyAction::Ignore);
        let caps = KeyContext {
            caps_lock: true,
            ..context
        };
        assert_eq!(classify_char('a', &caps), KeyAction::FlushThenPass);
        assert_eq!(classify_char('!', &caps), KeyAction::FlushThenPass);
    }

    #[test]
    fn semicolon_and_apostrophe_selection_fall_back() {
        let select = KeyContext {
            selection: Some(1),
            ..composing()
        };
        assert_eq!(
            classify_char(';', &select),
            KeyAction::Select {
                index: 1,
                otherwise: Box::new(KeyAction::Punctuation { flush_first: true })
            }
        );
        let third = KeyContext {
            selection: Some(2),
            ..composing()
        };
        assert_eq!(
            classify_char('\'', &third),
            KeyAction::Select {
                index: 2,
                otherwise: Box::new(KeyAction::Separator)
            }
        );
        let microsoft = KeyContext {
            selection: Some(1),
            semicolon_is_code: true,
            ..composing()
        };
        assert_eq!(classify_char(';', &microsoft), KeyAction::Compose(';'));
        assert_eq!(with_separator("xi", 32).as_deref(), Some("xi'"));
        assert_eq!(with_separator("xi'", 32), None);
        assert_eq!(with_separator("abc", 3), None);
    }

    #[test]
    fn selection_index_with_real_dictionary() {
        let input = crate::MixedInput::bundled();
        // `'` separates syllables when the code may be pinyin, else picks the 3rd candidate.
        assert_eq!(crate::selection_index(&input, "xi", '\''), None);
        assert_eq!(crate::selection_index(&input, "ggll", '\''), Some(2));
        assert_eq!(crate::selection_index(&input, "xi", ';'), Some(1));
    }
}
