pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Simplified → Traditional (OpenCC STPhrases first, then the first STCharacters candidate). Tables
/// are built on first use.
#[cfg(feature = "engine")]
pub fn to_traditional(text: &str) -> String {
    use std::collections::HashMap;
    struct Tables {
        chars: HashMap<char, char>,
        phrases: HashMap<String, String>,
        longest: usize,
    }
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        let rows = |contents: &'static str| {
            contents
                .lines()
                .filter(|line| !line.starts_with('#'))
                .filter_map(|line| {
                    let (from, to) = line.split_once('\t')?;
                    Some((from, to.split(' ').next()?))
                })
        };
        let chars = rows(include_str!("../../../data/opencc/STCharacters.txt"))
            .filter_map(|(from, to)| Some((from.chars().next()?, to.chars().next()?)))
            .collect();
        let phrases: HashMap<String, String> =
            rows(include_str!("../../../data/opencc/STPhrases.txt"))
                .map(|(from, to)| (from.to_owned(), to.to_owned()))
                .collect();
        let longest = phrases
            .keys()
            .map(|key| key.chars().count())
            .max()
            .unwrap_or(1);
        Tables {
            chars,
            phrases,
            longest,
        }
    });
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let matched = (2..=tables.longest.min(chars.len() - at))
            .rev()
            .find_map(|len| {
                let piece: String = chars[at..at + len].iter().collect();
                tables.phrases.get(&piece).map(|to| (len, to))
            });
        if let Some((len, to)) = matched {
            out.push_str(to);
            at += len;
        } else {
            out.push(tables.chars.get(&chars[at]).copied().unwrap_or(chars[at]));
            at += 1;
        }
    }
    out
}

/// Full-width punctuation in Chinese mode; `.` and `:` after a digit stay half-width (`3.5`,
/// `10:30`). Quotes are handled by [`Quotes`].
pub fn full_width(ch: char, after_digit: bool) -> Option<&'static str> {
    Some(match ch {
        ',' => "，",
        '.' if !after_digit => "。",
        '?' => "？",
        '!' => "！",
        ':' if !after_digit => "：",
        ';' => "；",
        '(' => "（",
        ')' => "）",
        '\\' => "、",
        '<' => "《",
        '>' => "》",
        '[' => "【",
        ']' => "】",
        '$' => "￥",
        '^' => "……",
        '_' => "——",
        '~' => "～",
        '`' => "·",
        _ => return None,
    })
}

/// Auto-close: the closing mark (quote, bracket, title mark) for an opening one.
pub fn closing_pair(open: &str) -> Option<&'static str> {
    Some(match open {
        "“" => "”",
        "‘" => "’",
        "（" => "）",
        "《" => "》",
        "【" => "】",
        "「" => "」",
        _ => return None,
    })
}

/// Whether `text` is a closing mark from [`closing_pair`] (right before the cursor it is skipped
/// over instead of typed again).
pub fn is_closing(text: &str) -> bool {
    matches!(text, "”" | "’" | "）" | "》" | "】" | "」")
}

/// Open/closed state of Chinese quotes (single and double tracked separately).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Quotes {
    single_open: bool,
    double_open: bool,
}

impl Quotes {
    /// The quote that would be typed next, without changing state; None if `ch` is not a quote.
    pub fn peek(&self, ch: char) -> Option<&'static str> {
        match ch {
            '\'' => Some(if self.single_open { "’" } else { "‘" }),
            '"' => Some(if self.double_open { "”" } else { "“" }),
            _ => None,
        }
    }
    /// Types a quote and flips its state.
    pub fn next(&mut self, ch: char) -> Option<&'static str> {
        let quote = self.peek(ch)?;
        if ch == '\'' {
            self.single_open = !self.single_open;
        } else {
            self.double_open = !self.double_open;
        }
        Some(quote)
    }
    /// Chinese-mode punctuation for `ch`: alternating quotes, otherwise [`full_width`].
    pub fn punctuate(&mut self, ch: char, after_digit: bool) -> Option<&'static str> {
        self.next(ch).or_else(|| full_width(ch, after_digit))
    }
}

/// ASCII punctuation or digit produced by a virtual key (+Shift) on a US layout; None for letters
/// and other keys. TSF uses it to type [`full_width`] punctuation when the input box is closed.
pub fn us_key_char(vk: u16, shift: bool) -> Option<char> {
    const SHIFTED_DIGITS: &[u8; 10] = b")!@#$%^&*(";
    let (plain, shifted) = match vk {
        0x30..=0x39 => {
            let index = (vk - 0x30) as usize;
            ((b'0' + index as u8) as char, SHIFTED_DIGITS[index] as char)
        }
        0xBA => (';', ':'),
        0xBB => ('=', '+'),
        0xBC => (',', '<'),
        0xBD => ('-', '_'),
        0xBE => ('.', '>'),
        0xBF => ('/', '?'),
        0xC0 => ('`', '~'),
        0xDB => ('[', '{'),
        0xDC => ('\\', '|'),
        0xDD => (']', '}'),
        0xDE => ('\'', '"'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commit_normalizes_windows_newlines_once() {
        assert_eq!(
            single_line("Hello\r\nWorld\rNext\nLine"),
            "Hello World Next Line"
        );
        assert_eq!(single_line("段一\n\n段二"), "段一 段二");
    }

    #[test]
    fn punctuation_and_quotes() {
        assert_eq!(full_width('.', false), Some("。"));
        assert_eq!(full_width('.', true), None);
        assert_eq!(full_width(':', true), None);
        assert_eq!(full_width('\\', false), Some("、"));
        assert_eq!(full_width('a', false), None);
        let mut quotes = Quotes::default();
        let out: Vec<_> = "'\"'\""
            .chars()
            .map(|ch| quotes.punctuate(ch, false).unwrap())
            .collect();
        assert_eq!(out, ["‘", "“", "’", "”"]);
        assert_eq!(quotes, Quotes::default());
    }

    /// The TSF punctuation table (without the input box) matches the core mapping key by key.
    #[test]
    fn tsf_virtual_key_punctuation_matches_v11_table() {
        let v11 = |key: u16, shift: bool, after_digit: bool| -> Option<&'static str> {
            Some(match (key, shift) {
                (0xBC, false) => "，",
                (0xBC, true) => "《",
                (0xBE, false) if !after_digit => "。",
                (0xBE, true) => "》",
                (0xBF, true) => "？",
                (0xBA, false) => "；",
                (0xBA, true) if !after_digit => "：",
                (0xDB, false) => "【",
                (0xDD, false) => "】",
                (0xDC, false) => "、",
                (0xC0, false) => "·",
                (0xC0, true) => "～",
                (0xBD, true) => "——",
                (0x31, true) => "！",
                (0x34, true) => "￥",
                (0x36, true) => "……",
                (0x39, true) => "（",
                (0x30, true) => "）",
                _ => return None,
            })
        };
        for key in 0u16..=0xFF {
            if key == 0xDE {
                continue; // quotes alternate via `Quotes`
            }
            for shift in [false, true] {
                for after_digit in [false, true] {
                    assert_eq!(
                        us_key_char(key, shift).and_then(|ch| full_width(ch, after_digit)),
                        v11(key, shift, after_digit),
                        "vk {key:#x} shift {shift} after_digit {after_digit}"
                    );
                }
            }
        }
    }
}
