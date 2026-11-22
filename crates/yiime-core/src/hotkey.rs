/// Default shortcut for plain Wubi mode (no translation).
pub const DEFAULT_PLAIN_TOGGLE: &str = "Ctrl+Shift+Space";

/// Shortcut: modifiers + Windows virtual-key code (used as a platform-neutral convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub vk: u16,
}

/// Parses shortcuts such as `Ctrl+Shift+Space` (case-insensitive, `+`-separated). Keys: Space, A–Z,
/// 0–9, F1–F12; except for F keys, Ctrl or Alt is required so ordinary typing is not taken.
/// windows/tsf/src/tsf.rs has a copy of these rules; change both together.
pub fn parse_hotkey(text: &str) -> Option<Hotkey> {
    let mut hotkey = Hotkey {
        ctrl: false,
        shift: false,
        alt: false,
        vk: 0,
    };
    for part in text.split('+').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => hotkey.ctrl = true,
            "shift" => hotkey.shift = true,
            "alt" => hotkey.alt = true,
            _ if hotkey.vk != 0 => return None,
            "space" => hotkey.vk = 0x20,
            _ if lower.len() == 1 && lower.as_bytes()[0].is_ascii_alphanumeric() => {
                hotkey.vk = lower.as_bytes()[0].to_ascii_uppercase() as u16;
            }
            _ => {
                let number: u16 = lower.strip_prefix('f')?.parse().ok()?;
                if !(1..=12).contains(&number) {
                    return None;
                }
                hotkey.vk = 0x6F + number;
            }
        }
    }
    let function_key = (0x70..=0x7B).contains(&hotkey.vk);
    (hotkey.vk != 0 && (hotkey.ctrl || hotkey.alt || function_key)).then_some(hotkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hotkeys_parse_with_modifiers() {
        assert_eq!(
            parse_hotkey(DEFAULT_PLAIN_TOGGLE),
            Some(Hotkey {
                ctrl: true,
                shift: true,
                alt: false,
                vk: 0x20
            })
        );
        assert_eq!(parse_hotkey("alt + p").map(|key| key.vk), Some(b'P' as u16));
        assert_eq!(parse_hotkey("F9").map(|key| key.vk), Some(0x78));
        assert_eq!(parse_hotkey("Shift+Space"), None);
        assert_eq!(parse_hotkey("Ctrl+A+B"), None);
        assert_eq!(parse_hotkey("Ctrl+F13"), None);
        assert_eq!(parse_hotkey("Ctrl"), None);
    }
}
