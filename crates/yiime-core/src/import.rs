//! Imports user dictionaries exported from other input methods into YiIME's user words
//! (`word code` per line, see [`crate::USER_WORDS_FILE`]); the code is the full pinyin without
//! separators, so the words come first under that pinyin.
//!
//! Supported: Sogou Pinyin text exports (`'ni'hao 你好`), QQ Pinyin text exports
//! (`ni'hao 你好 1`), Rime-style tables (`你好\tni hao\t1`), and Sogou cell dictionaries (.scel).

/// One imported word: (word, pinyin code).
pub type ImportedWord = (String, String);

fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF)
        || ch == '〇'
}

/// Pinyin code from an exported spelling (`'ni'hao`, `ni hao`, `lü`): letters only, ü as v.
fn pinyin_code(spelling: &str) -> Option<String> {
    let code: String = spelling
        .chars()
        .filter(|ch| !matches!(ch, '\'' | ' ' | '-'))
        .map(|ch| match ch {
            'ü' | 'Ü' => 'v',
            ch => ch.to_ascii_lowercase(),
        })
        .collect();
    (!code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_lowercase()))
        .then_some(code)
}

/// Words from a text export: per line, one Chinese word and its pinyin (in either order,
/// separated by spaces, tabs or `,`); counts and comments are ignored, lines without both are
/// skipped. A UTF-16 export (as Sogou writes) must be decoded first, see [`decode_text`].
pub fn parse_text_export(text: &str) -> Vec<ImportedWord> {
    let mut words = Vec::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let fields: Vec<&str> = line
            .split(['\t', ',', '，'])
            .flat_map(|field| {
                // Space separates word from pinyin in Sogou/QQ exports, but also syllables in
                // Rime tables: keep a run of pinyin syllables together.
                let field = field.trim();
                if field.chars().any(is_han) {
                    field.split(' ').collect::<Vec<_>>()
                } else {
                    vec![field]
                }
            })
            .filter(|field| !field.is_empty())
            .collect();
        let word = fields
            .iter()
            .find(|field| field.chars().all(is_han))
            .map(|word| word.to_string());
        let code = fields
            .iter()
            .filter(|field| field.chars().any(|ch| ch.is_ascii_alphabetic()))
            .find_map(|field| pinyin_code(field));
        if let (Some(word), Some(code)) = (word, code) {
            words.push((word, code));
        }
    }
    words
}

/// Decodes an exported text file: UTF-16 (LE/BE with BOM, as Sogou exports), UTF-8 or GB18030
/// is not supported (re-save it as UTF-8). None if it is not valid text.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    let utf16 = |little: bool, data: &[u8]| {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(true, rest),
        [0xFE, 0xFF, rest @ ..] => utf16(false, rest),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

const SCEL_PINYIN_TABLE: usize = 0x1540;
const SCEL_WORDS: usize = 0x2628;

/// Words from a Sogou cell dictionary (.scel). Errors describe a malformed file.
pub fn parse_scel(bytes: &[u8]) -> Result<Vec<ImportedWord>, &'static str> {
    if bytes.len() < SCEL_WORDS || bytes[4..8] != [0x44, 0x43, 0x53, 0x01] {
        return Err("不是搜狗细胞词库（.scel）文件");
    }
    let mut at = SCEL_PINYIN_TABLE + 4;
    let u16_at = |at: usize| -> Result<usize, &'static str> {
        bytes
            .get(at..at + 2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]) as usize)
            .ok_or("词库文件不完整")
    };
    let text_at = |at: usize, len: usize| -> Result<String, &'static str> {
        let data = bytes.get(at..at + len).ok_or("词库文件不完整")?;
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| "词库文件编码错误")
    };
    let mut syllables: Vec<String> = Vec::new();
    while at < SCEL_WORDS {
        let index = u16_at(at)?;
        let len = u16_at(at + 2)?;
        if len == 0 {
            break;
        }
        let syllable = text_at(at + 4, len)?;
        if syllables.len() <= index {
            syllables.resize(index + 1, String::new());
        }
        syllables[index] = syllable;
        at += 4 + len;
    }
    let mut words = Vec::new();
    let mut at = SCEL_WORDS;
    while at + 4 <= bytes.len() {
        let same = u16_at(at)?;
        let pinyin_len = u16_at(at + 2)?;
        at += 4;
        let mut code = String::new();
        for slot in 0..pinyin_len / 2 {
            let index = u16_at(at + slot * 2)?;
            code.push_str(syllables.get(index).ok_or("拼音索引越界")?);
        }
        at += pinyin_len;
        for _ in 0..same {
            let word_len = u16_at(at)?;
            let word = text_at(at + 2, word_len)?;
            at += 2 + word_len;
            let extra = u16_at(at)?;
            at += 2 + extra;
            if let Some(code) = pinyin_code(&code) {
                words.push((word, code));
            }
        }
    }
    Ok(words)
}

/// Imports a file's bytes (.scel by its header, otherwise a text export).
pub fn import_words(bytes: &[u8]) -> Result<Vec<ImportedWord>, &'static str> {
    if bytes.len() >= 8 && bytes[4..8] == [0x44, 0x43, 0x53, 0x01] {
        return parse_scel(bytes);
    }
    let text = decode_text(bytes).ok_or("无法识别文件编码（请另存为 UTF-8 或 UTF-16）")?;
    let words = parse_text_export(&text);
    if words.is_empty() {
        return Err("文件里没有找到「词 拼音」格式的词条");
    }
    Ok(words)
}

/// Appends imported words to the user words file contents, skipping pairs already there.
/// Returns the new contents and how many words were added.
pub fn merge_user_words(existing: &str, words: &[ImportedWord]) -> (String, usize) {
    let mut seen: std::collections::HashSet<(String, String)> = existing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.to_owned(),
                fields.next()?.to_ascii_lowercase(),
            ))
        })
        .collect();
    let mut contents = existing.to_owned();
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push_str("\r\n");
    }
    let mut added = 0;
    for (word, code) in words {
        if seen.insert((word.clone(), code.clone())) {
            contents.push_str(&format!("{word} {code}\r\n"));
            added += 1;
        }
    }
    (contents, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(words: &[(&str, &str)]) -> Vec<ImportedWord> {
        words
            .iter()
            .map(|(word, code)| (word.to_string(), code.to_string()))
            .collect()
    }

    #[test]
    fn sogou_qq_and_rime_text_exports() {
        let sogou = "'ni'hao 你好\r\n'lü'xing 旅行\r\n";
        assert_eq!(
            parse_text_export(sogou),
            pairs(&[("你好", "nihao"), ("旅行", "lvxing")])
        );
        let qq = "# QQ\nzhong'guo 中国 12\nbad line\n";
        assert_eq!(parse_text_export(qq), pairs(&[("中国", "zhongguo")]));
        let rime = "输入法\tshu ru fa\t100\n";
        assert_eq!(parse_text_export(rime), pairs(&[("输入法", "shurufa")]));
    }

    #[test]
    fn utf16_exports_decode() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "'ni'hao 你好".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        assert_eq!(import_words(&bytes).unwrap(), pairs(&[("你好", "nihao")]));
        assert!(import_words(b"hello world").is_err());
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn scel_cell_dictionary() {
        let mut bytes = vec![0u8; SCEL_PINYIN_TABLE];
        bytes[..12].copy_from_slice(&[0x40, 0x15, 0, 0, 0x44, 0x43, 0x53, 0x01, 1, 0, 0, 0]);
        bytes.extend([3, 0, 0, 0]);
        for (index, syllable) in ["ni", "hao", "lv"].iter().enumerate() {
            let text = utf16(syllable);
            bytes.extend((index as u16).to_le_bytes());
            bytes.extend((text.len() as u16).to_le_bytes());
            bytes.extend(text);
        }
        bytes.resize(SCEL_WORDS, 0);
        // Two homophones under ni'hao, one word under lv.
        bytes.extend([2, 0, 4, 0, 0, 0, 1, 0]);
        for word in ["你好", "拟好"] {
            let text = utf16(word);
            bytes.extend((text.len() as u16).to_le_bytes());
            bytes.extend(text);
            bytes.extend([10, 0]);
            bytes.extend([0; 10]);
        }
        bytes.extend([1, 0, 2, 0, 2, 0]);
        let text = utf16("绿");
        bytes.extend((text.len() as u16).to_le_bytes());
        bytes.extend(text);
        bytes.extend([10, 0]);
        bytes.extend([0; 10]);
        assert_eq!(
            import_words(&bytes).unwrap(),
            pairs(&[("你好", "nihao"), ("拟好", "nihao"), ("绿", "lv")])
        );
        assert!(parse_scel(&bytes[..SCEL_WORDS + 6]).is_err());
    }

    #[test]
    fn merging_skips_existing_pairs() {
        let (contents, added) = merge_user_words(
            "# header\n你好 nihao\n",
            &pairs(&[("你好", "nihao"), ("中国", "zhongguo")]),
        );
        assert_eq!(added, 1);
        assert_eq!(contents, "# header\n你好 nihao\n中国 zhongguo\r\n");
        let input = crate::MixedInput::bundled().with_user_words(&contents);
        assert_eq!(input.candidates("zhongguo", 1)[0].text, "中国");
    }
}
