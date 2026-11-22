use crate::table::{RawTable, Side, Table, encode, merge_extra, parse_dictionary};
pub(crate) use crate::table::{folded_initials, normalize_frequency_pinyin};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// Bundled dictionaries precompiled by `build.rs` (see `table.rs`).
#[cfg(feature = "bundled")]
static BUNDLED_INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundled.idx"));
/// Index of the rime-frost lexicon shipped in data/frost, precompiled by `build.rs`; used only when
/// the loaded file is byte-identical, otherwise the index is built at load time.
#[cfg(feature = "bundled")]
static FROST_INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/frost.idx"));
#[cfg(not(feature = "bundled"))]
static FROST_INDEX: &[u8] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Wubi,
    Pinyin,
    /// Wubi code with the wildcard `z`, which stands for any letter (as in Microsoft Wubi).
    Wildcard,
    /// Pinyin initials (`yxyy` → 一心一意, `zgr` → 中国人), or initials mixed with full syllables
    /// (`zhongg` → 中国).
    Abbreviation,
    /// Pinyin with a typo corrected (`zhonggou` → 中国); ranked after exact matches.
    Corrected,
    /// The whole code is not a word: the leading syllables form one, and [`Candidate::rest`] keeps
    /// the rest of the code for the next pick.
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub source: Source,
    pub complete: bool,
    /// Code left after picking this candidate; non-empty only for [`Source::Partial`].
    pub rest: String,
}

pub struct MixedInput {
    /// Wubi and pinyin tables, initials order and per-character Wubi codes (used to annotate
    /// pinyin candidates; word codes are derived from character full codes by the standard rules
    /// and checked against the Wubi table). Precompiled at build time for the bundled
    /// dictionaries.
    table: Table,
    /// User words by code, ranked first under their code.
    user: BTreeMap<String, Vec<Arc<str>>>,
    frequency: Option<FrequencyWords>,
    /// Large-lexicon initials index, built on first use so startup stays fast.
    abbreviations: OnceLock<Abbreviations>,
    /// Valid syllables, built on first use.
    syllable_table: OnceLock<spell::SyllableTable>,
}

#[path = "spell.rs"]
mod spell;

/// Large-lexicon pinyin-initials index. Keys are the first letter of each syllable (zh/ch/sh
/// folded to z/c/s), sorted bytewise so a prefix query is a binary-searched range; retroflex
/// initials are re-checked from the syllables to filter queries that spell out zh/ch/sh. The Rime
/// words have the same index precompiled in the table.
struct Abbreviations {
    /// Large-lexicon line numbers sorted by folded key, most frequent first within a key.
    frost: Vec<u32>,
}

/// First index in `0..count` for which `before` is false (`before` must be monotonic).
fn partition(count: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = (low + high) / 2;
        if before(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// Pinyin spelling of a code for lookups: `lve`/`nve` → `lue`/`nue` (Wubi lookups keep the code as
/// typed).
fn pinyin_spelling(code: &str) -> std::borrow::Cow<'_, str> {
    if code.contains("ve") {
        std::borrow::Cow::Owned(code.replace("lve", "lue").replace("nve", "nue"))
    } else {
        std::borrow::Cow::Borrowed(code)
    }
}

/// Words below this frequency are left out of initials (`bjdx`) matches and single-character
/// pinyin homophones when only common characters are shown: they are mostly classical idioms and
/// variant characters (伯喈倒屣, 鞎).
pub const COMMON_FREQUENCY_FLOOR: u64 = 10;

/// Whether every character of `text` is commonly displayable: no characters outside the Basic
/// Multilingual Plane (CJK extensions B+, often shown as boxes), CJK Extension A, compatibility
/// ideographs or private-use characters.
pub fn is_common_text(text: &str) -> bool {
    text.chars().all(|ch| {
        let code = ch as u32;
        code <= 0xFFFF
            && !(0x3400..=0x4DBF).contains(&code)
            && !(0xE000..=0xF8FF).contains(&code)
            && !(0xF900..=0xFAFF).contains(&code)
    })
}

/// First letter of each syllable; zh/ch/sh become Z/C/S.
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

/// Parses initials input. zh/ch/sh may be one initial or two letters (`zh` → 字号), so a code has
/// several parses: (folded key, per-letter requirement), where 0 accepts any initial and Z/C/S
/// requires zh/ch/sh.
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

/// Large-lexicon index: line ranges plus line numbers sorted by pinyin key (without spaces) and by
/// Wubi code, most frequent first within a key.
struct FrequencyWords {
    text: String,
    lines: Vec<(u32, u32)>,
    by_pinyin: Vec<u32>,
    by_code: Vec<u32>,
    /// Precompiled initials order (when the file is the one shipped with this build).
    abbreviations: Option<Vec<u32>>,
}

struct FrequencyLine<'a> {
    text: &'a str,
    pinyin: &'a str,
    weight: u64,
    code: &'a str,
}

impl FrequencyWords {
    fn new(raw: String) -> Self {
        let precompiled = crate::table::decode_frost(FROST_INDEX, raw.as_bytes());
        let text = normalize_frequency_pinyin(raw);
        let index = precompiled.unwrap_or_else(|| crate::table::frost_index(&text, false));
        Self {
            text,
            lines: index.lines,
            by_pinyin: index.by_pinyin,
            by_code: index.by_code,
            abbreviations: index.abbreviations,
        }
    }

    fn line(&self, index: u32) -> FrequencyLine<'_> {
        let (start, end) = self.lines[index as usize];
        let (text, pinyin, weight, code) =
            crate::table::frost_fields(&self.text[start as usize..end as usize]);
        FrequencyLine {
            text,
            pinyin,
            weight,
            code,
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

/// Pinyin key: the syllables without spaces.
fn pinyin_key(pinyin: &str) -> std::iter::Filter<std::str::Bytes<'_>, fn(&u8) -> bool> {
    pinyin.bytes().filter(|b| *b != b' ')
}

impl MixedInput {
    /// Rime Wubi and pinyin dictionaries, one `word<TAB>code<TAB>frequency` per line.
    pub fn from_rime(wubi: &str, pinyin: &str) -> Self {
        Self::from_tables(
            &parse_dictionary(wubi, false),
            &parse_dictionary(pinyin, true),
        )
    }

    fn from_tables(wubi: &RawTable, pinyin: &RawTable) -> Self {
        Self::with_table(Table::from_owned(encode(wubi, pinyin)))
    }

    fn with_table(table: Table) -> Self {
        Self {
            table,
            user: BTreeMap::new(),
            frequency: None,
            abbreviations: OnceLock::new(),
            syllable_table: OnceLock::new(),
        }
    }

    /// Size in bytes of the dictionary index (precompiled for [`Self::bundled`]).
    pub fn index_bytes(&self) -> usize {
        self.table.len()
    }

    /// Dictionaries compiled into the binary: Wubi 86, pinyin and supplementary words. The large
    /// lexicon and user words are loaded at runtime.
    #[cfg(feature = "bundled")]
    ///
    /// Nothing is parsed here: `build.rs` precompiled them into an index that is used in place.
    pub fn bundled() -> Self {
        Self::with_table(Table::from_static(BUNDLED_INDEX))
    }

    fn abbreviation_index(&self) -> &Abbreviations {
        self.abbreviations.get_or_init(|| {
            let frost = self
                .frequency
                .as_ref()
                .map(|words| {
                    words.abbreviations.clone().unwrap_or_else(|| {
                        crate::table::frost_abbreviations(&words.text, &words.lines)
                    })
                })
                .unwrap_or_default();
            Abbreviations { frost }
        })
    }

    /// Builds the initials index and syllable table ahead of time (from a background thread, so the
    /// first keystroke does not stall).
    pub fn warm_up(&self) {
        let _ = self.abbreviation_index();
        let _ = self.syllables();
    }

    fn syllables(&self) -> &spell::SyllableTable {
        self.syllable_table.get_or_init(|| {
            spell::SyllableTable::new(
                self.table
                    .all(Side::Pinyin)
                    .flat_map(|entry| entry.syllables.split(' ')),
            )
        })
    }

    /// Words for a pinyin key from the Rime dictionary and the large lexicon, sorted by frequency.
    fn homophones(&self, key: &str) -> Vec<(Arc<str>, u64, Box<str>)> {
        let mut homophones: Vec<(Arc<str>, u64, Box<str>)> = self
            .table
            .get(Side::Pinyin, key)
            .map(|entry| {
                (
                    Arc::from(entry.text),
                    entry.weight,
                    Box::from(entry.syllables),
                )
            })
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

    /// Words for pinyin initials (at least two), with exactly that many syllables, sorted by
    /// frequency.
    fn abbreviation(&self, code: &str, limit: usize, min_weight: u64) -> Vec<Arc<str>> {
        if code.len() < 2 || code.len() > 12 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }
        let index = self.abbreviation_index();
        let mut found: Vec<(Arc<str>, u64)> = Vec::new();
        let table = &self.table;
        let count = table.abbreviation_count();
        for (key, need) in abbreviation_parses(code) {
            let start = partition(count, |i| {
                folded_initials(table.abbreviation(i).syllables).lt(key.bytes())
            });
            for i in start..count {
                let entry = table.abbreviation(i);
                if !folded_initials(entry.syllables).eq(key.bytes()) {
                    break;
                }
                if initials_match(&syllable_initials(entry.syllables), &need) {
                    found.push((Arc::from(entry.text), entry.weight));
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
        found.retain(|(_, weight)| *weight >= min_weight);
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut seen = HashSet::new();
        found
            .into_iter()
            .filter(|(text, _)| seen.insert(text.clone()))
            .take(limit)
            .map(|(text, _)| text)
            .collect()
    }

    /// Merges supplementary Wubi words (skipping words already under the same code), then re-sorts
    /// by frequency; the original dictionary has higher frequencies and stays first.
    pub fn with_extra_wubi(self, extra: &str) -> Self {
        let mut wubi = self.table.decode(Side::Wubi);
        merge_extra(&mut wubi, parse_dictionary(extra, false));
        let pinyin = self.table.decode(Side::Pinyin);
        Self {
            table: Table::from_owned(encode(&wubi, &pinyin)),
            user: self.user,
            frequency: self.frequency,
            abbreviations: OnceLock::new(),
            syllable_table: OnceLock::new(),
        }
    }

    /// Candidates under one full Wubi code: user words first, then the dictionary by frequency.
    fn wubi_slot(&self, code: &str) -> Vec<(Arc<str>, u64)> {
        let user = self.user.get(code);
        let mut slot: Vec<(Arc<str>, u64)> = user
            .into_iter()
            .flatten()
            .map(|text| (text.clone(), u64::MAX))
            .collect();
        slot.extend(
            self.table
                .get(Side::Wubi, code)
                .filter(|entry| user.is_none_or(|words| !words.iter().any(|w| &**w == entry.text)))
                .map(|entry| (Arc::from(entry.text), entry.weight)),
        );
        slot
    }

    /// Wubi entries of every code that starts with `prefix` and passes `keep`, in code order.
    fn wubi_with_prefix(&self, prefix: &str, keep: impl Fn(&str) -> bool) -> Vec<(Arc<str>, u64)> {
        let mut codes: BTreeSet<&str> = BTreeSet::new();
        let table = &self.table;
        for i in table.lower_bound(Side::Wubi, prefix)..table.key_count(Side::Wubi) {
            let key = table.key(Side::Wubi, i);
            if !key.starts_with(prefix) {
                break;
            }
            if keep(key) {
                codes.insert(key);
            }
        }
        for key in self
            .user
            .range::<str, _>((
                std::ops::Bound::Included(prefix),
                std::ops::Bound::Unbounded,
            ))
            .map(|(key, _)| key.as_str())
            .take_while(|key| key.starts_with(prefix))
        {
            if keep(key) {
                codes.insert(key);
            }
        }
        codes
            .into_iter()
            .flat_map(|code| self.wubi_slot(code))
            .collect()
    }

    /// Attaches the large lexicon (from rime-frost, `word<TAB>pinyin<TAB>frequency<TAB>Wubi code`
    /// per line). The text is kept as one block with two line-number indexes (about 10 MB + 2.4 MB)
    /// instead of expanded tables, to save memory and start fast. Pinyin words are ranked together
    /// with the Rime homophones by frequency; Wubi words go after the original words.
    pub fn with_frequency_words(mut self, contents: String) -> Self {
        self.frequency = Some(FrequencyWords::new(contents));
        self
    }

    /// Wildcard `z` matches any letter; only Wubi codes of the same length, sorted by frequency.
    fn wildcard(&self, code: &str, limit: usize) -> Vec<Arc<str>> {
        let prefix: String = code.chars().take_while(|ch| *ch != 'z').collect();
        let mut found = self.wubi_with_prefix(&prefix, |key| {
            key.len() == code.len()
                && key
                    .bytes()
                    .zip(code.bytes())
                    .all(|(have, want)| want == b'z' || have == want)
        });
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        found
            .into_iter()
            .take(limit)
            .map(|(text, _)| text)
            .collect()
    }

    /// Merges user words (`word code` per line, whitespace-separated, `#` starts a comment), ranked
    /// first under their code.
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
            let slot = self.user.entry(code).or_default();
            slot.retain(|word| &**word != text);
            slot.insert(0, text.into());
        }
        self
    }

    /// Number of distinct candidates for a full Wubi code, including the large lexicon.
    pub fn wubi_count(&self, code: &str) -> usize {
        let slot = self.wubi_slot(code);
        let mut texts: HashSet<&str> = slot.iter().map(|(text, _)| &**text).collect();
        if let Some(words) = &self.frequency {
            texts.extend(words.code(code).map(|line| line.text));
        }
        texts.len()
    }

    /// Whether some pinyin starts with `code`, i.e. typing on may still give pinyin.
    pub fn pinyin_continues(&self, code: &str) -> bool {
        let code = &*pinyin_spelling(code);
        let next = self.table.lower_bound(Side::Pinyin, code);
        (next < self.table.key_count(Side::Pinyin)
            && self.table.key(Side::Pinyin, next).starts_with(code))
            || self
                .frequency
                .as_ref()
                .is_some_and(|words| words.has_pinyin_prefix(code))
    }

    /// Whether swapping two adjacent letters could still give pinyin (typo correction).
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

    /// Wubi code hint for a character or word: the full code (`toln`), or `shortest/full` when a
    /// short code exists (`r/rqyy`). None if not in the dictionary.
    pub fn wubi_hint(&self, text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        if let [single] = chars[..] {
            let (short, full) = self.table.codes_of(single)?;
            return Some(if short == full {
                full.to_string()
            } else {
                format!("{short}/{full}")
            });
        }
        let full = |ch: &char| self.table.codes_of(*ch).map(|codes| codes.1);
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
            .wubi_slot(&code)
            .iter()
            .any(|(word, _)| &**word == text)
            || self
                .frequency
                .as_ref()
                .is_some_and(|words| words.code(&code).any(|line| line.text == text));
        known.then_some(code)
    }

    /// Full Wubi codes first, then pinyin, then Wubi-prefix candidates.
    pub fn candidates(&self, raw: &str, limit: usize) -> Vec<Candidate> {
        self.candidates_with_page(raw, limit, 9)
    }

    /// Like [`Self::candidates`] with `page_size` candidates per page; short codes get at most two
    /// pages of initials matches (at least 4).
    pub fn candidates_with_page(
        &self,
        raw: &str,
        limit: usize,
        page_size: usize,
    ) -> Vec<Candidate> {
        self.candidates_with_options(raw, limit, page_size, false, false)
    }

    /// Like [`Self::candidates_with_page`]; `fuzzy` enables fuzzy pinyin (z/zh, n/l, an/ang…).
    /// `pinyin_first` ranks exact pinyin before full Wubi codes.
    ///
    /// Order: full Wubi code → exact pinyin → fuzzy pinyin → typo correction → initials → mixed
    /// full/initials → whole sentence → leading syllables → wildcard → Wubi prefix. Typo
    /// correction, mixed input, whole sentence and leading syllables run only when the code is
    /// neither a Wubi code nor a pinyin word, so codes that type directly are unaffected.
    pub fn candidates_with_options(
        &self,
        raw: &str,
        limit: usize,
        page_size: usize,
        fuzzy: bool,
        pinyin_first: bool,
    ) -> Vec<Candidate> {
        self.candidates_filtered(raw, limit, page_size, fuzzy, pinyin_first, false)
    }

    /// Like [`Self::candidates_with_options`]; `common_only` leaves out rare characters (see
    /// [`is_common_text`]) and initials or single-character pinyin matches below
    /// [`COMMON_FREQUENCY_FLOOR`].
    pub fn candidates_filtered(
        &self,
        raw: &str,
        limit: usize,
        page_size: usize,
        fuzzy: bool,
        pinyin_first: bool,
        common_only: bool,
    ) -> Vec<Candidate> {
        let code = raw.to_ascii_lowercase().replace([' ', '\''], "");
        if code.is_empty() {
            return Vec::new();
        }
        // Pinyin lookups read lve/nve as lüe/nüe; Wubi lookups keep the code as typed.
        let spelling = pinyin_spelling(&code).into_owned();
        let py = spelling.as_str();
        // A `'` separator only exists in pinyin, so such a code never matches Wubi.
        let pinyin_only = raw.contains('\'');
        let mut homophones = self.homophones(py);
        if common_only {
            homophones.retain(|(text, weight, _)| {
                is_common_text(text)
                    && (*weight >= COMMON_FREQUENCY_FLOOR || text.chars().nth(1).is_some())
            });
        }
        let wanted = if raw.contains('\'') {
            boundaries(&raw.to_ascii_lowercase().replace(' ', ""), '\'')
        } else {
            Vec::new()
        };
        // User words (e.g. imported pinyin words) also come first in pinyin-only codes; otherwise
        // they lead the exact Wubi slot below.
        let user: Vec<Arc<str>> = if pinyin_only {
            self.user.get(&code).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        if raw.contains('\'') {
            let mut seen = HashSet::new();
            let separated = user
                .iter()
                .cloned()
                .chain(
                    homophones
                        .iter()
                        .filter(|(_, _, syllables)| {
                            let own = boundaries(syllables, ' ');
                            wanted.iter().all(|offset| own.contains(offset))
                        })
                        .map(|(text, _, _)| text.clone()),
                )
                .filter(|text| seen.insert(text.clone()))
                .map(|text| Candidate {
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
        let mut wubi: Vec<Arc<str>> = user;
        if !pinyin_only {
            wubi.extend(self.wubi_slot(&code).into_iter().map(|(text, _)| text));
            if let Some(words) = &self.frequency {
                wubi.extend(words.code(&code).map(|line| Arc::from(line.text)));
            }
        }
        let exact = !wubi.is_empty() || !homophones.is_empty();
        // Only when the code is neither a Wubi code nor a pinyin word.
        let spelled = !exact && code.len() >= 4 && code.bytes().all(|b| b.is_ascii_lowercase());
        let (sentence, partial) = if spelled {
            self.segment_fallback(py, &wanted, page_size.max(1) * 2)
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
                if common_only && !is_common_text(&text) {
                    continue;
                }
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
        let pinyin: Vec<_> = homophones.into_iter().map(|(text, _, _)| text).collect();
        if pinyin_first {
            push(pinyin, Source::Pinyin, true);
            push(wubi, Source::Wubi, true);
        } else {
            push(wubi, Source::Wubi, true);
            push(pinyin, Source::Pinyin, true);
        }
        if fuzzy {
            push(self.fuzzy(py, &wanted, limit), Source::Pinyin, true);
        }
        // Typo correction first (`zhonggou` → 中国).
        if spelled {
            push(
                self.corrections(py, &wanted, limit),
                Source::Corrected,
                true,
            );
        }
        // Initials go after full pinyin and before wildcard and Wubi prefix. Short codes (< 4
        // letters) get at most two pages (at least 4), then Wubi-prefix candidates.
        let abbreviation_limit = if code.len() < 4 {
            limit.min((page_size * 2).max(4))
        } else {
            limit
        };
        push(
            self.abbreviation(
                &code,
                abbreviation_limit,
                if common_only {
                    COMMON_FREQUENCY_FLOOR
                } else {
                    0
                },
            ),
            Source::Abbreviation,
            true,
        );
        if spelled && wanted.is_empty() {
            push(
                self.mixed_abbreviation(py, limit),
                Source::Abbreviation,
                true,
            );
        }
        push(sentence.into_iter().collect(), Source::Pinyin, true);
        push(partial, Source::Partial, true);
        if !pinyin_only && code.contains('z') && code.len() <= 4 {
            push(self.wildcard(&code, limit), Source::Wildcard, true);
        }
        if !pinyin_only && code.len() < 4 && !self.table.contains_key(Side::Pinyin, py) {
            let mut prefixes = self.wubi_with_prefix(&code, |key| key != code);
            prefixes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            push(
                prefixes.into_iter().map(|(text, _)| text).collect(),
                Source::Wubi,
                false,
            );
        }
        result
    }
}

/// Separator positions in the code with separators removed: `xi'an` → [2].
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
        assert_eq!(input.table.all(Side::Wubi).count(), valid_lines);
    }

    #[test]
    fn bundled_index_matches_dictionary_sources() {
        // The index precompiled by build.rs is byte-identical to one built from the sources at
        // run time, and both answer like the parsed dictionaries did.
        let built = MixedInput::from_rime(
            include_str!("../../../data/wubi/wubi86.dict.yaml"),
            include_str!("../../../data/pinyin/pinyin_simp.dict.yaml"),
        )
        .with_extra_wubi(include_str!(
            "../../../data/wubi_extra/wubi86_extra.dict.yaml"
        ));
        let bundled = MixedInput::bundled();
        assert!(built.table.as_bytes() == bundled.table.as_bytes());
        for code in ["wqvb", "zhongguo", "bjdx", "aaz", "g", "fanyi"] {
            assert_eq!(
                built.candidates(code, 30),
                bundled.candidates(code, 30),
                "{code}"
            );
        }
        assert_eq!(bundled.wubi_hint("的").as_deref(), Some("r/rqyy"));
    }

    #[test]
    fn lve_nve_spell_lue_nue_and_apostrophe_is_pinyin_only() {
        let wubi = "...\n男女朋友\tlvel\t9\n恨\tnve\t9\n略\tlttm\t1\n";
        let pinyin = "...\n略\tlue\t100\n虐\tnue\t50\n绿\tlv\t80\n";
        let words = "略微\tlve wei\t3330\tlttm\n女儿\tnv er\t10\tvvqt\n";
        let input = MixedInput::from_rime(wubi, pinyin).with_frequency_words(words.to_owned());
        let texts = |code: &str| -> Vec<String> {
            input
                .candidates(code, 9)
                .into_iter()
                .map(|c| c.text)
                .collect()
        };
        // lve/lue and nve/nue give the same pinyin; the Wubi code `nve` still comes first.
        assert_eq!(texts("lve")[0], "略");
        assert_eq!(texts("lue")[0], "略");
        assert_eq!(texts("nve"), ["恨", "虐"]);
        assert_eq!(texts("lvewei")[0], "略微");
        assert_eq!(texts("luewei")[0], "略微");
        assert!(input.pinyin_continues("lve"));
        // With `'` nothing comes from Wubi (no `男女朋友` prefix match, no `恨`).
        assert_eq!(texts("lv'e"), ["略"]);
        assert_eq!(texts("n've"), ["虐"]);
        assert!(!texts("nv'e").contains(&"恨".to_owned()));
        // The real dictionaries: lve/nve no longer fall through to Wubi words.
        let full = MixedInput::bundled();
        assert_eq!(full.candidates("lve", 9)[0].text, "略");
        assert!(
            full.candidates("lve", 9)
                .iter()
                .all(|c| c.text != "男女朋友")
        );
        assert_eq!(full.candidates("nve", 9)[0].text, "恨");
        assert!(full.candidates("nve", 9).iter().any(|c| c.text == "虐"));
        assert_eq!(
            normalize_frequency_pinyin("略\tlve\t1\tx\n".into()),
            "略\tlue\t1\tx\n"
        );
    }

    #[test]
    fn precompiled_frost_index_matches_runtime_build() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/frost/InputTranslation-words.txt"
        ))
        .unwrap();
        let precompiled = crate::table::decode_frost(FROST_INDEX, raw.as_bytes()).unwrap();
        let built = crate::table::frost_index(&normalize_frequency_pinyin(raw.clone()), true);
        assert_eq!(precompiled.lines, built.lines);
        assert_eq!(precompiled.by_pinyin, built.by_pinyin);
        assert_eq!(precompiled.by_code, built.by_code);
        assert_eq!(precompiled.abbreviations, built.abbreviations);
        // Any other content falls back to building the index at load time.
        assert!(crate::table::decode_frost(FROST_INDEX, b"x\tx\t1\tx\n").is_none());
    }

    #[test]
    fn common_only_hides_rare_characters_and_low_frequency_matches() {
        let wubi = "...\n𧦮\tyxyy\t9\n高级语言\tyxyy\t5\n㐂\tabcd\t9\n";
        let pinyin = "...\n很\then\t160208\n鞎\then\t3\n拫\then\t2\n";
        let words = "北京大学\tbei jing da xue\t2010\tuydi\n伯喈倒屣\tbo jie dao xi\t0\twkwn\n";
        let input = MixedInput::from_rime(wubi, pinyin).with_frequency_words(words.to_owned());
        let texts = |code: &str, common: bool| -> Vec<String> {
            input
                .candidates_filtered(code, 9, 9, false, false, common)
                .into_iter()
                .map(|c| c.text)
                .collect()
        };
        assert_eq!(texts("yxyy", false), ["𧦮", "高级语言"]);
        assert_eq!(texts("yxyy", true), ["高级语言"]);
        assert_eq!(texts("hen", false), ["很", "鞎", "拫"]);
        assert_eq!(texts("hen", true), ["很"]);
        assert_eq!(texts("bjdx", false), ["北京大学", "伯喈倒屣"]);
        assert_eq!(texts("bjdx", true), ["北京大学"]);
        assert!(texts("abcd", true).is_empty());
        assert!(is_common_text("中文，ok〇"));
        assert!(!is_common_text("𢝧") && !is_common_text("㐂"));
        // The real dictionaries: lve no longer offers the Ext-B 𨋨 prefix, bjdx only 北京大学.
        let full = MixedInput::bundled();
        let real = |code: &str| -> Vec<String> {
            full.candidates_filtered(code, 90, 9, false, false, true)
                .into_iter()
                .map(|c| c.text)
                .collect()
        };
        assert!(real("nve").iter().all(|text| is_common_text(text)));
        assert!(real("lve").iter().all(|text| is_common_text(text)));
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
        // Multi-syllable pinyin words get Wubi hints too.
        let words = input.candidates("fanyi", 9);
        assert_eq!(words[0].text, "翻译");
        assert_eq!(words[0].source, Source::Pinyin);
        // A full Wubi code still ranks before pinyin.
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
        // Idioms only in the supplementary words have the standard four-character code.
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
        // `z` matches z and zh; `zh` matches only zh, or z + h.
        assert_eq!(texts("zg"), ["中国", "总共", "字根"]);
        assert_eq!(texts("zhg"), ["中国"]);
        assert_eq!(texts("zh"), ["字号", "知乎"]);
        // Full Wubi code first, full pinyin second, initials last.
        assert_eq!(texts("ggll")[0], "一");
        // One candidate per page still gives at least 4 initials matches; nine per page gives at
        // most 18.
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
