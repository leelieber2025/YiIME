use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Wubi,
    Pinyin,
    /// Candidates matched by the Wubi code containing the universal key z (same as Microsoft Wubi,
    /// z replaces any code).
    Wildcard,
    /// The first letter of Pinyin is simplified (yxyy → single-minded, zgr → Chinese), and the full
    /// Pinyin and simplified version are mixed (zhongg → Chinese).
    Abbreviation,
    /// The pinyin candidate (zhonggou → China) resulting from the corrected misspelling is ranked
    /// after the accurate result.
    Corrected,
    /// When the whole string is not in the lexicon, the first part of the word is cut out according
    /// to syllables (after selecting, [`Candidate::rest`] remains in the encoding to continue
    /// selecting).
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub source: Source,
    pub complete: bool,
    /// Codes that have not been used after selecting this candidate (only [`Source::Partial`] is
    /// not empty).
    pub rest: String,
}

#[derive(Clone)]
struct Entry {
    /// Shared string: The words in the large dictionary are hung on both Pinyin and Wubi tables at
    /// the same time, and only one copy is saved.
    text: Arc<str>,
    weight: u64,
    /// Pinyin original encoding (syllables separated by spaces, such as `xi an`), used for matching
    /// `'` diaphragms; Wubi is empty.
    syllables: Box<str>,
}

pub struct MixedInput {
    wubi: BTreeMap<String, Vec<Entry>>,
    pinyin: BTreeMap<String, Vec<Entry>>,
    /// Single character → (shortest Wubi code, longest Wubi code) is used to mark Wubi codes for
    /// pinyin candidates; the coding of words is derived from the single character full code
    /// according to the standard coding rules and checked in the Wubi table. No additional index is
    /// created to save memory.
    wubi_codes: HashMap<char, (Box<str>, Box<str>)>,
    frequency: Option<FrequencyWords>,
    /// Jianpin index: The first input may be the encoding of Jianpin (it will not slow down the
    /// startup).
    abbreviations: OnceLock<Abbreviations>,
    /// Legal syllable table: Created when syllables need to be segmented for the first time.
    syllable_table: OnceLock<spell::SyllableTable>,
}

#[path = "spell.rs"]
mod spell;

/// Simplified spelling index: The keys are sorted in byte order, and a continuous interval is
/// bifurcated during the query. No separate table is created for each prefix; the key here is the
/// "first letter of each syllable" (zh/ch/sh is folded into z/c/s), and the tongue information is
/// recorded separately (capital Z/C/S). Use it to filter when the query writes zh/ch/sh.
struct Abbreviations {
    /// Multi-syllable words in Rime Pinyin dictionary: (initial consonant string, raised tongue
    /// capital), words, word frequency, syllables, sorted by folded lowercase keys.
    rime: Vec<RimeAbbreviation>,
    /// Large lexicon line number, sorted by the folded first letter key (words with the same key
    /// with the highest frequency come first).
    frost: Vec<u32>,
}

/// One item in the simplified spelling index: (initial consonant string, capitalized with raised
/// tongue), word, word frequency, syllable.
type RimeAbbreviation = (Box<str>, Arc<str>, u64, Box<str>);

/// Syllable string (space separated) → the first letter of each syllable; zh/ch/sh is written as
/// capital Z/C/S.
fn syllable_initials(syllables: &str) -> String {
    syllables
        .split(' ')
        .filter(|syllable| !syllable.is_empty())
        .filter_map(|syllable| {
            let mut bytes = syllable.bytes();
            let first = bytes.next()?;
            Some(
                if matches!(first, b'z' | b'c' | b's') && bytes.next() == Some(b'h') {
                    first.to_ascii_uppercase() as char
                } else {
                    first as char
                },
            )
        })
        .collect()
}

/// The collapsed initial letter key (zh/ch/sh → z/c/s) does not allocate memory.
fn folded_initials(syllables: &str) -> impl Iterator<Item = u8> + '_ {
    syllables
        .split(' ')
        .filter_map(|syllable| syllable.bytes().next())
}

/// Divide the simplified pinin input into the first letters of syllables: zh/ch/sh can be used as
/// either an initial consonant or two initial letters (zh → font size). Return (collapse key,
/// per-bit requirement), the requirement is 0, which means no flat tongue is allowed, otherwise it
/// is required uppercase Z/C/S.
fn abbreviation_parses(code: &str) -> Vec<(String, Vec<u8>)> {
    fn walk(rest: &[u8], key: &mut String, need: &mut Vec<u8>, out: &mut Vec<(String, Vec<u8>)>) {
        let Some(&first) = rest.first() else {
            out.push((key.clone(), need.clone()));
            return;
        };
        if out.len() >= 16 {
            return;
        }
        key.push(first as char);
        need.push(0);
        walk(&rest[1..], key, need, out);
        need.pop();
        if matches!(first, b'z' | b'c' | b's') && rest.get(1) == Some(&b'h') {
            need.push(first.to_ascii_uppercase());
            walk(&rest[2..], key, need, out);
            need.pop();
        }
        key.pop();
    }
    let mut out = Vec::new();
    walk(
        code.as_bytes(),
        &mut String::new(),
        &mut Vec::new(),
        &mut out,
    );
    out.retain(|(key, _)| key.len() >= 2);
    out
}

fn initials_match(initials: &str, need: &[u8]) -> bool {
    initials.len() == need.len()
        && initials
            .bytes()
            .zip(need)
            .all(|(have, want)| *want == 0 || have == *want)
}

/// Compact index of large thesaurus at runtime: line range + line number sorted by Pinyin key
/// (remove spaces)/Wubi code, with words with the highest frequency in the same key first.
struct FrequencyWords {
    text: String,
    lines: Vec<(u32, u32)>,
    by_pinyin: Vec<u32>,
    by_code: Vec<u32>,
}

struct FrequencyLine<'a> {
    text: &'a str,
    pinyin: &'a str,
    weight: u64,
    code: &'a str,
}

impl FrequencyWords {
    fn new(text: String) -> Self {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches(['\r', '\n']);
            if !body.starts_with('#')
                && body.split('\t').count() >= 4
                && text.len() < u32::MAX as usize
            {
                lines.push((offset as u32, (offset + body.len()) as u32));
            }
            offset += line.len();
        }
        let mut words = Self {
            text,
            lines,
            by_pinyin: Vec::new(),
            by_code: Vec::new(),
        };
        // First sort by word frequency in descending order, and then by key for stable sorting.
        // Within the same key, keep words with higher frequency first; use cache keys to avoid
        // repeatedly splitting lines.
        let mut by_weight: Vec<u32> = (0..words.lines.len() as u32).collect();
        by_weight.sort_by_cached_key(|i| std::cmp::Reverse(words.line(*i).weight));
        let mut by_pinyin = by_weight.clone();
        by_pinyin.sort_by_cached_key(|i| pinyin_key(words.line(*i).pinyin).collect::<Vec<u8>>());
        let mut by_code = by_weight;
        by_code.sort_by_cached_key(|i| words.line(*i).code.to_owned());
        words.by_pinyin = by_pinyin;
        words.by_code = by_code;
        words
    }

    /// All line numbers, in descending order of word frequency.
    fn by_weight_order(&self) -> Vec<u32> {
        let mut order: Vec<u32> = (0..self.lines.len() as u32).collect();
        order.sort_by_cached_key(|i| std::cmp::Reverse(self.line(*i).weight));
        order
    }

    fn line(&self, index: u32) -> FrequencyLine<'_> {
        let (start, end) = self.lines[index as usize];
        let mut fields = self.text[start as usize..end as usize].split('\t');
        FrequencyLine {
            text: fields.next().unwrap_or(""),
            pinyin: fields.next().unwrap_or(""),
            weight: fields.next().and_then(|w| w.parse().ok()).unwrap_or(0),
            code: fields.next().unwrap_or(""),
        }
    }

    fn pinyin(&self, key: &str) -> impl Iterator<Item = FrequencyLine<'_>> {
        let from = self
            .by_pinyin
            .partition_point(|i| pinyin_key(self.line(*i).pinyin).lt(key.bytes()));
        self.by_pinyin[from..]
            .iter()
            .map(|i| self.line(*i))
            .take_while(move |line| pinyin_key(line.pinyin).eq(key.bytes()))
    }

    fn has_pinyin_prefix(&self, prefix: &str) -> bool {
        let from = self
            .by_pinyin
            .partition_point(|i| pinyin_key(self.line(*i).pinyin).lt(prefix.bytes()));
        self.by_pinyin.get(from).is_some_and(|i| {
            let key: Vec<u8> = pinyin_key(self.line(*i).pinyin).collect();
            key.starts_with(prefix.as_bytes())
        })
    }

    fn code(&self, code: &str) -> impl Iterator<Item = FrequencyLine<'_>> {
        let from = self.by_code.partition_point(|i| self.line(*i).code < code);
        self.by_code[from..]
            .iter()
            .map(|i| self.line(*i))
            .take_while(move |line| line.code == code)
    }
}

/// Pinyin key: byte sequence after removing the spaces between syllables.
fn pinyin_key(pinyin: &str) -> std::iter::Filter<std::str::Bytes<'_>, fn(&u8) -> bool> {
    pinyin.bytes().filter(|b| *b != b' ')
}

impl MixedInput {
    /// Directly read the text of Rime's Wubi and Pinyin dictionaries; each line is "word <TAB>
    /// encoding <TAB> word frequency".
    pub fn from_rime(wubi: &str, pinyin: &str) -> Self {
        let wubi = parse_dictionary(wubi, false);
        let wubi_codes = code_index(&wubi);
        Self {
            wubi,
            pinyin: parse_dictionary(pinyin, true),
            wubi_codes,
            frequency: None,
            abbreviations: OnceLock::new(),
            syllable_table: OnceLock::new(),
        }
    }

    /// Thesaurus packaged with the program: Wubi 86 + Pinyin + Supplementary Idioms (programmed
    /// into binary). Large lexicon and user lexicon are loaded separately at runtime.
    #[cfg(feature = "bundled")]
    pub fn bundled() -> Self {
        Self::from_rime(
            include_str!("../../../data/wubi/wubi86.dict.yaml"),
            include_str!("../../../data/pinyin/pinyin_simp.dict.yaml"),
        )
        .with_extra_wubi(include_str!(
            "../../../data/wubi_extra/wubi86_extra.dict.yaml"
        ))
    }

    fn abbreviation_index(&self) -> &Abbreviations {
        self.abbreviations.get_or_init(|| {
            let mut rime: Vec<RimeAbbreviation> = self
                .pinyin
                .values()
                .flatten()
                .filter(|entry| entry.syllables.contains(' '))
                .map(|entry| {
                    (
                        syllable_initials(&entry.syllables).into_boxed_str(),
                        entry.text.clone(),
                        entry.weight,
                        entry.syllables.clone(),
                    )
                })
                .collect();
            rime.sort_by_cached_key(|(initials, _, weight, _)| {
                (initials.to_ascii_lowercase(), std::cmp::Reverse(*weight))
            });
            let frost = self
                .frequency
                .as_ref()
                .map(|words| {
                    // by_pinyin is already "the most frequent words with the same key come first".
                    // After the stable sorting, the keys with the same initial letter will still be
                    // roughly ranked according to word frequency.
                    let mut order: Vec<u32> = words
                        .by_weight_order()
                        .into_iter()
                        .filter(|i| words.line(*i).pinyin.contains(' '))
                        .collect();
                    order.sort_by_cached_key(|i| {
                        folded_initials(words.line(*i).pinyin).collect::<Vec<u8>>()
                    });
                    order
                })
                .unwrap_or_default();
            Abbreviations { rime, frost }
        })
    }

    /// The simplified pinyin index and syllable table are pre-built (can be called in the
    /// background thread to avoid getting stuck when typing for the first time).
    pub fn warm_up(&self) {
        let _ = self.abbreviation_index();
        let _ = self.syllables();
    }

    fn syllables(&self) -> &spell::SyllableTable {
        self.syllable_table.get_or_init(|| {
            spell::SyllableTable::new(
                self.pinyin
                    .values()
                    .flatten()
                    .flat_map(|entry| entry.syllables.split(' ')),
            )
        })
    }

    /// Candidates with the same pinyin key: original pinyin dictionary + large thesaurus, uniformly
    /// sorted by word frequency.
    fn homophones(&self, key: &str) -> Vec<(Arc<str>, u64, Box<str>)> {
        let mut homophones: Vec<(Arc<str>, u64, Box<str>)> = self
            .pinyin
            .get(key)
            .into_iter()
            .flatten()
            .map(|entry| (entry.text.clone(), entry.weight, entry.syllables.clone()))
            .collect();
        if let Some(words) = &self.frequency {
            homophones.extend(
                words
                    .pinyin(key)
                    .map(|line| (Arc::from(line.text), line.weight, Box::from(line.pinyin))),
            );
        }
        homophones.sort_by_key(|item| std::cmp::Reverse(item.1));
        homophones
    }

    /// Candidates for simplified spelling (at least two initial letters), only words with exactly
    /// the same number of syllables, sorted by word frequency.
    fn abbreviation(&self, code: &str, limit: usize) -> Vec<Arc<str>> {
        if code.len() < 2 || code.len() > 12 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }
        let index = self.abbreviation_index();
        let mut found: Vec<(Arc<str>, u64)> = Vec::new();
        for (key, need) in abbreviation_parses(code) {
            let start = index
                .rime
                .partition_point(|(initials, _, _, _)| initials.to_ascii_lowercase() < key);
            for (initials, text, weight, _) in index.rime[start..]
                .iter()
                .take_while(|(initials, _, _, _)| initials.eq_ignore_ascii_case(&key))
            {
                if initials_match(initials, &need) {
                    found.push((text.clone(), *weight));
                }
            }
            if let Some(words) = &self.frequency {
                let start = index
                    .frost
                    .partition_point(|i| folded_initials(words.line(*i).pinyin).lt(key.bytes()));
                for i in &index.frost[start..] {
                    let line = words.line(*i);
                    if !folded_initials(line.pinyin).eq(key.bytes()) {
                        break;
                    }
                    if initials_match(&syllable_initials(line.pinyin), &need) {
                        found.push((Arc::from(line.text), line.weight));
                    }
                }
            }
        }
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut seen = HashSet::new();
        found
            .into_iter()
            .filter(|(text, _)| seen.insert(text.clone()))
            .take(limit)
            .map(|(text, _)| text)
            .collect()
    }

    /// Merged into the supplementary Wubi Dictionary (existing words under the same code are not
    /// repeated); rearranged according to word frequency, the original dictionary has high word
    /// frequency and is ranked first.
    pub fn with_extra_wubi(mut self, extra: &str) -> Self {
        for (code, entries) in parse_dictionary(extra, false) {
            let slot = self.wubi.entry(code).or_default();
            for entry in entries {
                if !slot.iter().any(|existing| existing.text == entry.text) {
                    slot.push(entry);
                }
            }
            slot.sort_by_key(|entry| std::cmp::Reverse(entry.weight));
        }
        self.wubi_codes = code_index(&self.wubi);
        self
    }

    /// Hang up the runtime large vocabulary (derived from rime-frost, each line "Word <TAB> Pinyin
    /// <TAB> Word frequency <TAB> Wubi word code"). The original text is saved as a whole block,
    /// and only two row number indexes (about 10MB + 2.4MB) are built based on encoding. They are
    /// not expanded into tables, saving memory and fast startup. Pinyin: sorted by word frequency
    /// together with the homophones in the original Pinyin dictionary; Wubi: sorted after the
    /// original words (by word frequency).
    pub fn with_frequency_words(mut self, contents: String) -> Self {
        self.frequency = Some(FrequencyWords::new(contents));
        self
    }

    /// Universal key: z in the code matches any code (only Wubi codes of the same length are looked
    /// at), sorted by word frequency.
    fn wildcard(&self, code: &str, limit: usize) -> Vec<Arc<str>> {
        let prefix: String = code.chars().take_while(|ch| *ch != 'z').collect();
        let mut found: Vec<&Entry> = self
            .wubi
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .filter(|(key, _)| {
                key.len() == code.len()
                    && key
                        .bytes()
                        .zip(code.bytes())
                        .all(|(have, want)| want == b'z' || have == want)
            })
            .flat_map(|(_, entries)| entries.iter())
            .collect();
        found.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.text.cmp(&b.text)));
        found
            .into_iter()
            .take(limit)
            .map(|entry| entry.text.clone())
            .collect()
    }

    /// Incorporate into the user vocabulary (each line of "word coding", separated by white space,
    /// starting with # is a comment): ranked at the top of the Wubi code.
    pub fn with_user_words(mut self, contents: &str) -> Self {
        for line in contents.lines() {
            let mut fields = line.split_whitespace();
            let (Some(text), Some(code)) = (fields.next(), fields.next()) else {
                continue;
            };
            let code = code.to_ascii_lowercase();
            if text.starts_with('#') || !code.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            let slot = self.wubi.entry(code).or_default();
            slot.retain(|entry| &*entry.text != text);
            slot.insert(
                0,
                Entry {
                    text: text.into(),
                    weight: u64::MAX,
                    syllables: "".into(),
                },
            );
        }
        self
    }

    /// The number of different candidates under the complete Wubi code (including large
    /// vocabulary).
    pub fn wubi_count(&self, code: &str) -> usize {
        let mut texts: HashSet<&str> = self
            .wubi
            .get(code)
            .into_iter()
            .flatten()
            .map(|entry| &*entry.text)
            .collect();
        if let Some(words) = &self.frequency {
            texts.extend(words.code(code).map(|line| line.text));
        }
        texts.len()
    }

    /// Is there a Pinyin code starting with `code` (including equality), that is, it may still be
    /// Pinyin if you continue typing.
    pub fn pinyin_continues(&self, code: &str) -> bool {
        self.pinyin
            .range(code.to_owned()..)
            .next()
            .is_some_and(|(key, _)| key.starts_with(code))
            || self
                .frequency
                .as_ref()
                .is_some_and(|words| words.has_pinyin_prefix(code))
    }

    /// Is it possible that the pinyin is still possible after swapping two adjacent letters (for
    /// correction of misspellings).
    pub fn pinyin_continues_after_swap(&self, code: &str) -> bool {
        let bytes = code.as_bytes();
        (0..bytes.len().saturating_sub(1)).any(|i| {
            bytes[i] != bytes[i + 1] && {
                let mut swapped = bytes.to_vec();
                swapped.swap(i, i + 1);
                String::from_utf8(swapped).is_ok_and(|text| self.pinyin_continues(&text))
            }
        })
    }

    /// Tips for Wubi encoding of characters or words: When there is only one length, it is the
    /// complete code (such as `toln`), and when there is a short code, it is the "shortest short
    /// code/complete code" (such as `r/rqyy`). Returns None if it is not in the dictionary.
    pub fn wubi_hint(&self, text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        if let [single] = chars[..] {
            let (short, full) = self.wubi_codes.get(&single)?;
            return Some(if short == full {
                full.to_string()
            } else {
                format!("{short}/{full}")
            });
        }
        let full = |ch: &char| self.wubi_codes.get(ch).map(|codes| &*codes.1);
        let take =
            |ch: &char, n: usize| full(ch).map(|code| code.chars().take(n).collect::<String>());
        let code = match chars.len() {
            0 | 1 => return None,
            2 => take(&chars[0], 2)? + &take(&chars[1], 2)?,
            3 => take(&chars[0], 1)? + &take(&chars[1], 1)? + &take(&chars[2], 2)?,
            _ => {
                take(&chars[0], 1)?
                    + &take(&chars[1], 1)?
                    + &take(&chars[2], 1)?
                    + &take(chars.last()?, 1)?
            }
        };
        let known = self
            .wubi
            .get(&code)
            .is_some_and(|slot| slot.iter().any(|entry| &*entry.text == text))
            || self
                .frequency
                .as_ref()
                .is_some_and(|words| words.code(&code).any(|line| line.text == text));
        known.then_some(code)
    }

    /// The complete Wubi code is given priority, followed by Pinyin, and finally the Wubi prefix
    /// candidates are given.
    pub fn candidates(&self, raw: &str, limit: usize) -> Vec<Candidate> {
        self.candidates_with_page(raw, limit, 9)
    }

    /// Same as [`Self::candidates`], `page_size` is the number of candidates per page: the
    /// simplified version of the short code only gives two pages (at least 4).
    pub fn candidates_with_page(
        &self,
        raw: &str,
        limit: usize,
        page_size: usize,
    ) -> Vec<Candidate> {
        self.candidates_with_options(raw, limit, page_size, false)
    }

    /// Same as [`Self::candidates_with_page`]; `fuzzy` turns on fuzzy sounds (z/zh, n/l,
    /// an/ang...).
    ///
    /// Sequence: complete Wubi code → precise pinyin → fuzzy sound → correction of misspellings →
    /// simplified spelling → full spelling, simplified and mixed spelling → whole sentence → first
    /// words cut out according to syllables → universal key → Wubi prefix. Error correction, mixed
    /// typing, whole sentences and first paragraphs are only checked when the string of codes is
    /// neither Wubi code nor a complete Pinyin word, so the Wubi code and the pinyin results that
    /// can be directly typed are exactly the same as before.
    pub fn candidates_with_options(
        &self,
        raw: &str,
        limit: usize,
        page_size: usize,
        fuzzy: bool,
    ) -> Vec<Candidate> {
        let code = raw.to_ascii_lowercase().replace([' ', '\''], "");
        if code.is_empty() {
            return Vec::new();
        }
        let homophones = self.homophones(&code);
        let wanted = if raw.contains('\'') {
            boundaries(&raw.to_ascii_lowercase().replace(' ', ""), '\'')
        } else {
            Vec::new()
        };
        if raw.contains('\'') {
            let mut seen = HashSet::new();
            let separated = homophones
                .iter()
                .filter(|(_, _, syllables)| {
                    let own = boundaries(syllables, ' ');
                    wanted.iter().all(|offset| own.contains(offset))
                })
                .filter(|(text, _, _)| seen.insert(text.clone()))
                .map(|(text, _, _)| Candidate {
                    text: text.to_string(),
                    source: Source::Pinyin,
                    complete: true,
                    rest: String::new(),
                })
                .take(limit)
                .collect::<Vec<_>>();
            if !separated.is_empty() {
                return separated;
            }
        }
        let mut wubi: Vec<Arc<str>> = self
            .wubi
            .get(&code)
            .into_iter()
            .flatten()
            .map(|entry| entry.text.clone())
            .collect();
        if let Some(words) = &self.frequency {
            wubi.extend(words.code(&code).map(|line| Arc::from(line.text)));
        }
        let exact = !wubi.is_empty() || !homophones.is_empty();
        // Only when the coding is neither Wubi code nor Pinyin words, do error correction, mixed
        // coding, complete sentences and first paragraphs.
        let spelled = !exact && code.len() >= 4 && code.bytes().all(|b| b.is_ascii_lowercase());
        let (sentence, partial) = if spelled {
            self.segment_fallback(&code, &wanted, page_size.max(1) * 2)
        } else {
            (None, Vec::new())
        };
        let mut rests: HashMap<Arc<str>, String> = HashMap::new();
        for (text, rest) in &partial {
            rests.entry(text.clone()).or_insert_with(|| rest.clone());
        }
        let partial: Vec<Arc<str>> = partial.into_iter().map(|(text, _)| text).collect();
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |texts: Vec<Arc<str>>, source, complete| {
            for text in texts {
                if result.len() < limit && seen.insert(text.clone()) {
                    result.push(Candidate {
                        text: text.to_string(),
                        source,
                        complete,
                        rest: if source == Source::Partial {
                            rests.get(&text).cloned().unwrap_or_default()
                        } else {
                            String::new()
                        },
                    });
                }
            }
        };
        push(wubi, Source::Wubi, true);
        push(
            homophones.into_iter().map(|(text, _, _)| text).collect(),
            Source::Pinyin,
            true,
        );
        if fuzzy {
            push(self.fuzzy(&code, &wanted, limit), Source::Pinyin, true);
        }
        // The code is neither Wubi code nor Pinyin word: correct the misspelling first (zhonggou →
        // China).
        if spelled {
            push(
                self.corrections(&code, &wanted, limit),
                Source::Corrected,
                true,
            );
        }
        // The simplified pinyin is ranked after the complete pinyin and before the universal key
        // and Wubi prefix. Short codes (< 4 codes) only give two pages of simplified spellings (at
        // least 4), followed by five-stroke prefix candidates.
        let abbreviation_limit = if code.len() < 4 {
            limit.min((page_size * 2).max(4))
        } else {
            limit
        };
        push(
            self.abbreviation(&code, abbreviation_limit),
            Source::Abbreviation,
            true,
        );
        if spelled && wanted.is_empty() {
            push(
                self.mixed_abbreviation(&code, limit),
                Source::Abbreviation,
                true,
            );
        }
        push(sentence.into_iter().collect(), Source::Pinyin, true);
        push(partial, Source::Partial, true);
        if code.contains('z') && code.len() <= 4 {
            push(self.wildcard(&code, limit), Source::Wildcard, true);
        }
        if code.len() < 4 && !self.pinyin.contains_key(&code) {
            let mut prefixes = self
                .wubi
                .range(code.clone()..)
                .take_while(|(key, _)| key.starts_with(&code))
                .filter(|(key, _)| *key != &code)
                .flat_map(|(_, values)| values.iter().cloned())
                .collect::<Vec<_>>();
            prefixes.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.text.cmp(&b.text)));
            push(
                prefixes.into_iter().map(|entry| entry.text).collect(),
                Source::Wubi,
                false,
            );
        }
        result
    }
}

/// Single word → (shortest Wubi code, longest Wubi code); the one with the same length that comes
/// first in dictionary order ensures stable results.
fn code_index(wubi: &BTreeMap<String, Vec<Entry>>) -> HashMap<char, (Box<str>, Box<str>)> {
    let mut index: HashMap<char, (Box<str>, Box<str>)> = HashMap::new();
    for (code, entries) in wubi {
        for entry in entries {
            let mut chars = entry.text.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let code: &str = code;
            let slot = index
                .entry(ch)
                .or_insert_with(|| (code.into(), code.into()));
            if (code.len(), code) < (slot.0.len(), &*slot.0) {
                slot.0 = code.into();
            }
            if (code.len(), std::cmp::Reverse(code)) > (slot.1.len(), std::cmp::Reverse(&*slot.1)) {
                slot.1 = code.into();
            }
        }
    }
    index
}

/// The position of the delimiter in the letter string after removing the delimiter, such as `xi'an`
/// → [2].
fn boundaries(code: &str, separator: char) -> Vec<usize> {
    let mut letters = 0;
    let mut result = Vec::new();
    for ch in code.chars() {
        if ch == separator {
            if letters > 0 && result.last() != Some(&letters) {
                result.push(letters);
            }
        } else {
            letters += 1;
        }
    }
    result.retain(|offset| *offset < letters);
    result
}

fn parse_dictionary(contents: &str, pinyin: bool) -> BTreeMap<String, Vec<Entry>> {
    let mut result: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut body = false;
    for line in contents.lines() {
        if line == "..." {
            body = true;
            continue;
        }
        if !body || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(text), Some(code)) = (fields.next(), fields.next()) else {
            continue;
        };
        let weight = fields
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        // Illegal word frequencies such as negative numbers, NaN, overflows, etc. are treated as 0
        // as missing word frequencies, and no entries are lost.
        let weight = if weight.is_finite() && weight >= 0.0 {
            weight as u64
        } else {
            0
        };
        let key = if pinyin {
            code.replace(' ', "")
        } else {
            code.to_owned()
        };
        result.entry(key).or_default().push(Entry {
            text: text.into(),
            weight,
            syllables: if pinyin { code.into() } else { "".into() },
        });
    }
    for entries in result.values_mut() {
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.weight));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Draft, to_traditional};
    #[test]
    fn mixed_candidate_to_editable_draft_to_english_commit() {
        let wubi = "---\nname: wubi86\n...\n你好\twqvb\t100\n其他\twqva\t50\n";
        let pinyin = "---\nname: pinyin_simp\n...\n你好\tni hao\t100\n世界\tshi jie\t90\n";
        let input = MixedInput::from_rime(wubi, pinyin);
        let mut draft = Draft::default();
        draft.insert(&input.candidates("wqvb", 5)[0].text);
        draft.insert(&input.candidates("shijie", 5)[0].text);
        assert_eq!(draft.text(), "你好世界");
        draft.backspace();
        draft.insert("界");
        draft
            .translate_with(|text| {
                assert_eq!(text, "你好世界");
                Ok("Hello, world.".into())
            })
            .unwrap();
        assert_eq!(draft.take_english().as_deref(), Some("Hello, world."));
        assert_eq!(draft.text(), "");
    }

    #[test]
    fn dictionary_keeps_entries_without_integer_weight() {
        let source = "---\nname: test\n...\n𬳶\tcmkg\nχ\tzzxx\t450.5\n";
        let input = MixedInput::from_rime(source, "");
        assert_eq!(input.candidates("cmkg", 1)[0].text, "𬳶");
        assert_eq!(input.candidates("zzxx", 1)[0].text, "χ");
    }

    #[test]
    fn every_wubi_dictionary_entry_is_loaded() {
        let source = include_str!("../../../data/wubi/wubi86.dict.yaml");
        let valid_lines = source
            .split_once("\n...\n")
            .unwrap()
            .1
            .lines()
            .filter(|line| !line.starts_with('#') && line.split('\t').count() >= 2)
            .count();
        let input = MixedInput::from_rime(source, "");
        assert_eq!(
            input.wubi.values().map(Vec::len).sum::<usize>(),
            valid_lines
        );
    }

    #[test]
    fn edited_gui_draft_invalidates_translation() {
        let mut draft = Draft::default();
        draft.replace_text("你好");
        draft.accept_translation("你好", "Hello".into()).unwrap();
        draft.replace_text("你好世界");
        assert_eq!(draft.translation(), None);
        assert!(draft.accept_translation("你好", "Hello".into()).is_err());
    }

    #[test]
    fn wubi_stays_first_without_unrelated_prefixes_after_pinyin() {
        let wubi = "---\n...\n伙\two\t100\n别字\tlued\t50\n";
        let pinyin = "---\n...\n我\two\t100\n略\tlue\t100\n";
        let input = MixedInput::from_rime(wubi, pinyin);
        assert_eq!(input.candidates("wo", 3)[0].text, "伙");
        assert_eq!(
            input
                .candidates("lue", 9)
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["略"]
        );
    }

    #[test]
    fn apostrophe_keeps_pinyin_syllable_boundaries() {
        let pinyin = "---\n...\n先\txian\t1000\n西安\txi an\t10\n";
        let input = MixedInput::from_rime("", pinyin);
        assert_eq!(input.candidates("xian", 9)[0].text, "先");
        let separated = input.candidates("xi'an", 9);
        assert_eq!(separated.len(), 1);
        assert_eq!(separated[0].text, "西安");
        let full = MixedInput::from_rime(
            include_str!("../../../data/wubi/wubi86.dict.yaml"),
            include_str!("../../../data/pinyin/pinyin_simp.dict.yaml"),
        );
        assert_eq!(full.candidates("xi'an", 9)[0].text, "西安");
        assert_eq!(boundaries("'xi''an'", '\''), vec![2]);
    }

    #[test]
    fn pinyin_candidates_get_wubi_hints_and_wubi_stays_first() {
        let wubi = "...\n的\tr\t9\n的\trqy\t9\n的\trqyy\t9\n翻\ttoln\t5\n译\tycfh\t5\n翻译\ttoyc\t5\n二\tfg\t5\n";
        let pinyin = "...\n翻\tfan\t9\n翻译\tfan yi\t9\n的\tde\t9\n得\tde\t5\n";
        let input = MixedInput::from_rime(wubi, pinyin);
        assert_eq!(input.wubi_hint("的").as_deref(), Some("r/rqyy"));
        assert_eq!(input.wubi_hint("翻").as_deref(), Some("toln"));
        assert_eq!(input.wubi_hint("翻译").as_deref(), Some("toyc"));
        assert_eq!(input.wubi_hint("得"), None);
        // Multi-syllable pinyin words can also be found, with Wubi code for phrases.
        let words = input.candidates("fanyi", 9);
        assert_eq!(words[0].text, "翻译");
        assert_eq!(words[0].source, Source::Pinyin);
        // The complete Wubi code still ranks ahead of Pinyin.
        let mixed = MixedInput::from_rime("...\n二\tfan\t1\n", pinyin);
        let list = mixed.candidates("fan", 9);
        assert_eq!(list[0].source, Source::Wubi);
        assert_eq!(list[1].text, "翻");
        assert_eq!(list[1].source, Source::Pinyin);
    }

    #[test]
    fn extra_wubi_words_follow_original_entries() {
        let wubi =
            "...\n一\tg\t9\n一\tggll\t9\n心\tny\t9\n心\tnyny\t9\n意\tujnu\t9\n吃\tgngu\t500\n";
        let extra = "# 补充\n---\nname: x\n...\n一心一意\tgngu\t1\n吃\tgngu\t1\n";
        let input = MixedInput::from_rime(wubi, "").with_extra_wubi(extra);
        let list = input.candidates("gngu", 9);
        let texts: Vec<_> = list.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["吃", "一心一意"]);
        assert_eq!(input.wubi_hint("一心一意").as_deref(), Some("gngu"));
    }

    #[test]
    fn bundled_extra_lexicon_has_idioms_with_standard_codes() {
        let input = MixedInput::from_rime(include_str!("../../../data/wubi/wubi86.dict.yaml"), "")
            .with_extra_wubi(include_str!(
                "../../../data/wubi_extra/wubi86_extra.dict.yaml"
            ));
        for (word, code) in [("一心一意", "gngu"), ("画蛇添足", "gjik"), ("翻译", "toyc")]
        {
            assert!(
                input.candidates(code, 90).iter().any(|c| c.text == word),
                "{word} 应可用 {code} 输入"
            );
        }
        // Idioms that are only in the supplementary thesaurus can also be entered using the
        // standard four-character word code.
        assert!(input.wubi_hint("期期艾艾").is_some());
    }

    #[test]
    fn frequency_words_reorder_pinyin_and_append_after_wubi() {
        let wubi = "...\n一\tggll\t9\n心\tnyny\t9\n意\tujnu\t9\n吃\tgngu\t500\n";
        let pinyin = "...\n是\tshi\t100\n事\tshi\t50\n";
        let words = "# 注释\n一心一意\tyi xin yi yi\t618\tgngu\n事\tshi\t900\t\n吃\tchi\t1\tgngu\n";
        let input = MixedInput::from_rime(wubi, pinyin).with_frequency_words(words.to_owned());
        let wubi_list: Vec<_> = input
            .candidates("gngu", 9)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(wubi_list, ["吃", "一心一意"]);
        let shi: Vec<_> = input
            .candidates("shi", 9)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(shi, ["事", "是"]);
        let phrase = input.candidates("yixinyiyi", 9);
        assert_eq!(phrase[0].text, "一心一意");
        assert_eq!(input.wubi_hint("一心一意").as_deref(), Some("gngu"));
    }

    #[test]
    fn pinyin_initials_abbreviation() {
        let wubi =
            "...\n一\tggll\t9\n心\tnyny\t9\n意\tujnu\t9\n中\tkhk\t9\n国\tlgyi\t9\n人\twwww\t9\n";
        let pinyin = "...\n中国人\tzhong guo ren\t50\n字根\tzi gen\t5\n";
        let words = "一心一意\tyi xin yi yi\t618\tgngu\n中国\tzhong guo\t900\tkhlg\n总共\tzong gong\t10\tukaw\n字号\tzi hao\t3\tpbkg\n知乎\tzhi hu\t2\ttdtu\n";
        let input = MixedInput::from_rime(wubi, pinyin).with_frequency_words(words.to_owned());
        let texts = |code: &str| -> Vec<String> {
            input
                .candidates(code, 9)
                .into_iter()
                .map(|c| c.text)
                .collect()
        };
        let first = input.candidates("yxyy", 9);
        assert_eq!(first[0].text, "一心一意");
        assert_eq!(first[0].source, Source::Abbreviation);
        assert_eq!(input.wubi_hint("一心一意").as_deref(), Some("gngu"));
        assert_eq!(texts("zgr"), ["中国人"]);
        assert_eq!(texts("zhgr"), ["中国人"]);
        // z is not limited to flat and tilted tongues; zh only needs to be tilted and can also be
        // split into z + h.
        assert_eq!(texts("zg"), ["中国", "总共", "字根"]);
        assert_eq!(texts("zhg"), ["中国"]);
        assert_eq!(texts("zh"), ["字号", "知乎"]);
        // The complete Wubi code is given priority, the complete Pinyin is second, and the
        // simplified Pinyin is last.
        assert_eq!(texts("ggll")[0], "一");
        // When there is 1 code per page, at least 4 short codes are given; when there are 9 codes
        // per page, the maximum number is 18.
        let abbreviations = |code: &str, page: usize| {
            input
                .candidates_with_page(code, 90, page)
                .into_iter()
                .filter(|c| c.source == Source::Abbreviation)
                .count()
        };
        assert_eq!(abbreviations("zg", 1), 3);
        assert_eq!(abbreviations("zg", 9), 3);
        assert!(texts("y").is_empty() || !texts("y").contains(&"一心一意".to_owned()));
    }

    #[test]
    fn wildcard_user_words_and_traditional() {
        let wubi = "...\n工\taaaa\t5\n式\taad\t9\n戒\taak\t3\n一\tggll\t9\n";
        let input = MixedInput::from_rime(wubi, "...\n").with_user_words("# 注释\n我们的\taak\n");
        let list: Vec<_> = input
            .candidates("aaz", 9)
            .into_iter()
            .map(|c| (c.text, c.source))
            .collect();
        assert_eq!(list[0], ("我们的".to_owned(), Source::Wildcard));
        assert!(list.iter().any(|(text, _)| text == "式"));
        assert_eq!(input.candidates("aak", 9)[0].text, "我们的");
        assert_eq!(input.wubi_count("aak"), 2);
        assert!(!input.pinyin_continues("aak"));
        assert_eq!(to_traditional("头发后来发展了"), "頭髮後來發展了");
    }

    #[test]
    fn invalid_weights_count_as_zero() {
        let wubi = "---\n...\n甲\taaaa\t-1\n乙\taaab\tnan\n丙\taaac\t1e400\n";
        let input = MixedInput::from_rime(wubi, "");
        for code in ["aaaa", "aaab", "aaac"] {
            assert_eq!(input.candidates(code, 1).len(), 1, "{code}");
        }
    }
}
