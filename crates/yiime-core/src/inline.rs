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
