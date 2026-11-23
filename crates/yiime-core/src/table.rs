//! Compact, read-only dictionary index shared by the build script and the library.
//!
//! The bundled Rime dictionaries (Wubi 86 + supplementary words, simplified pinyin) are parsed
//! once at build time (`build.rs`) into one little-endian blob that is embedded with
//! `include_bytes!`. At run time nothing is parsed: lookups binary-search sorted fixed-size
//! records directly inside the blob, which lives in the read-only data segment and is paged in by
//! the OS on demand (the same effect as `mmap`). Tests and [`crate::MixedInput::from_rime`] build
//! the same blob in memory from text.
//!
//! Layout (all integers `u32` LE unless noted):
//! - header: magic `YIMEIDX2`, then [`HEADER_WORDS`] words of section offsets/counts;
//! - strings: UTF-8 texts, codes and syllables, deduplicated;
//! - for Wubi and pinyin each: keys `(key_off, key_len, first_entry)` sorted bytewise plus one
//!   sentinel, and entries `(text_off, text_len, syll_off, syll_len, weight_lo, weight_hi)` sorted
//!   by weight (descending, stable) within a key;
//! - abbreviations: pinyin entry numbers of multi-syllable words sorted by folded initials
//!   (zh/ch/sh → z/c/s), most frequent first;
//! - code index: `(char, shortest key, longest key)` sorted by char, for Wubi hints.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

pub const MAGIC: &[u8; 8] = b"YIMEIDX2";
const HEADER_WORDS: usize = 12;
const KEY_SIZE: usize = 12;
const ENTRY_SIZE: usize = 24;
const CODE_SIZE: usize = 12;

/// One parsed dictionary line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub text: String,
    pub weight: u64,
    /// Pinyin syllables separated by spaces (`xi an`); empty for Wubi.
    pub syllables: String,
}

pub type RawTable = BTreeMap<String, Vec<RawEntry>>;

/// `lve`/`nve` (and `lv`-`e` written as one syllable) are the keyboard spellings of lüe/nüe. Rime
/// writes them `lue`/`nue`, rime-frost `lve`/`nve`; both are stored as `lue`/`nue`.
pub fn normalize_syllable(syllable: &str) -> &str {
    match syllable {
        "lve" => "lue",
        "nve" => "nue",
        other => other,
    }
}

/// Normalizes the space-separated syllables of a pinyin entry (see [`normalize_syllable`]).
pub fn normalize_syllables(syllables: &str) -> String {
    if !syllables.contains("ve") {
        return syllables.to_owned();
    }
    syllables
        .split(' ')
        .map(normalize_syllable)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rime dictionary text, one `word<TAB>code<TAB>frequency` per line after the `...` line.
pub fn parse_dictionary(contents: &str, pinyin: bool) -> RawTable {
    let mut result: RawTable = BTreeMap::new();
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
        // Negative, NaN or overflowing frequencies count as 0; the entry is kept.
        let weight = if weight.is_finite() && weight >= 0.0 {
            weight as u64
        } else {
            0
        };
        let (key, syllables) = if pinyin {
            let syllables = normalize_syllables(code);
            (syllables.replace(' ', ""), syllables)
        } else {
            (code.to_owned(), String::new())
        };
        result.entry(key).or_default().push(RawEntry {
            text: text.to_owned(),
            weight,
            syllables,
        });
    }
    for entries in result.values_mut() {
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.weight));
    }
    result
}

/// Merges supplementary words (skipping words already under the same code) and re-sorts by
/// frequency; the original dictionary has higher frequencies and stays first.
pub fn merge_extra(table: &mut RawTable, extra: RawTable) {
    for (code, entries) in extra {
        let slot = table.entry(code).or_default();
        for entry in entries {
            if !slot.iter().any(|existing| existing.text == entry.text) {
                slot.push(entry);
            }
        }
        slot.sort_by_key(|entry| std::cmp::Reverse(entry.weight));
    }
}

/// First letter of each syllable with zh/ch/sh folded to z/c/s.
pub fn folded_initials(syllables: &str) -> impl Iterator<Item = u8> + '_ {
    syllables
        .split(' ')
        .filter_map(|syllable| syllable.bytes().next())
}

struct Writer {
    out: Vec<u8>,
    strings: Vec<u8>,
    seen: HashMap<String, (u32, u32)>,
}

impl Writer {
    fn string(&mut self, text: &str) -> (u32, u32) {
        if let Some(found) = self.seen.get(text) {
            return *found;
        }
        let slot = (self.strings.len() as u32, text.len() as u32);
        self.strings.extend_from_slice(text.as_bytes());
        self.seen.insert(text.to_owned(), slot);
        slot
    }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn encode_side(writer: &mut Writer, table: &RawTable) -> (Vec<u8>, Vec<u8>) {
    let mut keys = Vec::with_capacity((table.len() + 1) * KEY_SIZE);
    let mut entries = Vec::new();
    let mut count = 0u32;
    for (key, list) in table {
        let (off, len) = writer.string(key);
        push_u32(&mut keys, off);
        push_u32(&mut keys, len);
        push_u32(&mut keys, count);
        for entry in list {
            let (text_off, text_len) = writer.string(&entry.text);
            let (syll_off, syll_len) = writer.string(&entry.syllables);
            push_u32(&mut entries, text_off);
            push_u32(&mut entries, text_len);
            push_u32(&mut entries, syll_off);
            push_u32(&mut entries, syll_len);
            push_u32(&mut entries, entry.weight as u32);
            push_u32(&mut entries, (entry.weight >> 32) as u32);
            count += 1;
        }
    }
    push_u32(&mut keys, 0);
    push_u32(&mut keys, 0);
    push_u32(&mut keys, count);
    (keys, entries)
}

/// Builds the index blob from parsed Wubi (already merged with supplementary words) and pinyin
/// tables.
pub fn encode(wubi: &RawTable, pinyin: &RawTable) -> Vec<u8> {
    let mut writer = Writer {
        out: Vec::new(),
        strings: Vec::new(),
        seen: HashMap::new(),
    };
    let (wubi_keys, wubi_entries) = encode_side(&mut writer, wubi);
    let (pinyin_keys, pinyin_entries) = encode_side(&mut writer, pinyin);
    // Abbreviations: multi-syllable pinyin words in dictionary order, then stably sorted by folded
    // initials and descending frequency.
    let mut abbreviations: Vec<(Vec<u8>, u64, u32)> = Vec::new();
    let mut index = 0u32;
    for list in pinyin.values() {
        for entry in list {
            if entry.syllables.contains(' ') {
                abbreviations.push((
                    folded_initials(&entry.syllables).collect(),
                    entry.weight,
                    index,
                ));
            }
            index += 1;
        }
    }
    abbreviations.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    // Character → (shortest, longest) Wubi code; among equal lengths the first in dictionary
    // order (shortest) or the last (longest) wins, as before.
    let mut codes: BTreeMap<char, (usize, usize)> = BTreeMap::new();
    let keys: Vec<&String> = wubi.keys().collect();
    for (key_index, (code, list)) in wubi.iter().enumerate() {
        for entry in list {
            let mut chars = entry.text.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let slot = codes.entry(ch).or_insert((key_index, key_index));
            let short = keys[slot.0];
            if (code.len(), code) < (short.len(), short) {
                slot.0 = key_index;
            }
            let long = keys[slot.1];
            if (code.len(), std::cmp::Reverse(code)) > (long.len(), std::cmp::Reverse(long)) {
                slot.1 = key_index;
            }
        }
    }
    let mut sections: Vec<Vec<u8>> = Vec::new();
    let mut abbreviation_bytes = Vec::with_capacity(abbreviations.len() * 4);
    for (_, _, index) in &abbreviations {
        push_u32(&mut abbreviation_bytes, *index);
    }
    let mut code_bytes = Vec::with_capacity(codes.len() * CODE_SIZE);
    for (ch, (short, long)) in &codes {
        push_u32(&mut code_bytes, *ch as u32);
        push_u32(&mut code_bytes, *short as u32);
        push_u32(&mut code_bytes, *long as u32);
    }
    let strings = std::mem::take(&mut writer.strings);
    let mut header = Vec::new();
    let mut offset = (MAGIC.len() + HEADER_WORDS * 4) as u32;
    let mut place = |bytes: Vec<u8>, header: &mut Vec<u8>, sections: &mut Vec<Vec<u8>>| {
        push_u32(header, offset);
        offset += bytes.len() as u32;
        sections.push(bytes);
    };
    place(strings, &mut header, &mut sections);
    place(wubi_keys, &mut header, &mut sections);
    push_u32(&mut header, wubi.len() as u32);
    place(wubi_entries, &mut header, &mut sections);
    place(pinyin_keys, &mut header, &mut sections);
    push_u32(&mut header, pinyin.len() as u32);
    place(pinyin_entries, &mut header, &mut sections);
    push_u32(&mut header, abbreviations.len() as u32);
    place(abbreviation_bytes, &mut header, &mut sections);
    push_u32(&mut header, codes.len() as u32);
    place(code_bytes, &mut header, &mut sections);
    push_u32(&mut header, offset);
    assert_eq!(header.len(), HEADER_WORDS * 4);
    let out = &mut writer.out;
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&header);
    for section in sections {
        out.extend_from_slice(&section);
    }
    std::mem::take(out)
}

enum Bytes {
    Static(&'static [u8]),
    Owned(Box<[u8]>),
}

/// Which keyed table of the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Wubi,
    Pinyin,
}

/// One entry inside the blob.
#[derive(Debug, Clone, Copy)]
pub struct EntryRef<'a> {
    pub text: &'a str,
    pub weight: u64,
    pub syllables: &'a str,
}

/// Read-only view of an index blob.
pub struct Table {
    bytes: Bytes,
    strings: usize,
    keys: [usize; 2],
    key_count: [usize; 2],
    entries: [usize; 2],
    abbreviations: usize,
    abbreviation_count: usize,
    codes: usize,
    code_count: usize,
}

impl Table {
    pub fn from_static(bytes: &'static [u8]) -> Self {
        Self::new(Bytes::Static(bytes))
    }
    pub fn from_owned(bytes: Vec<u8>) -> Self {
        Self::new(Bytes::Owned(bytes.into_boxed_slice()))
    }
    fn new(bytes: Bytes) -> Self {
        let data: &[u8] = match &bytes {
            Bytes::Static(data) => data,
            Bytes::Owned(data) => data,
        };
        assert!(
            data.len() >= MAGIC.len() + HEADER_WORDS * 4 && &data[..8] == MAGIC,
            "invalid dictionary index"
        );
        let word = |i: usize| read_u32(data, MAGIC.len() + i * 4) as usize;
        assert_eq!(word(11), data.len(), "truncated dictionary index");
        Self {
            strings: word(0),
            keys: [word(1), word(4)],
            key_count: [word(2), word(5)],
            entries: [word(3), word(6)],
            abbreviation_count: word(7),
            abbreviations: word(8),
            code_count: word(9),
            codes: word(10),
            bytes,
        }
    }
    fn data(&self) -> &[u8] {
        match &self.bytes {
            Bytes::Static(data) => data,
            Bytes::Owned(data) => data,
        }
    }
    #[cfg(test)]
    pub fn as_bytes(&self) -> &[u8] {
        self.data()
    }
    fn word(&self, at: usize) -> u32 {
        read_u32(self.data(), at)
    }
    fn string(&self, off: u32, len: u32) -> &str {
        let start = self.strings + off as usize;
        std::str::from_utf8(&self.data()[start..start + len as usize]).unwrap_or("")
    }
    fn side(side: Side) -> usize {
        match side {
            Side::Wubi => 0,
            Side::Pinyin => 1,
        }
    }
    pub fn key_count(&self, side: Side) -> usize {
        self.key_count[Self::side(side)]
    }
    pub fn key(&self, side: Side, index: usize) -> &str {
        let at = self.keys[Self::side(side)] + index * KEY_SIZE;
        self.string(self.word(at), self.word(at + 4))
    }
    fn first_entry(&self, side: Side, index: usize) -> usize {
        self.word(self.keys[Self::side(side)] + index * KEY_SIZE + 8) as usize
    }
    /// Number of the first key not less than `code` (bytewise).
    pub fn lower_bound(&self, side: Side, code: &str) -> usize {
        let (mut low, mut high) = (0, self.key_count(side));
        while low < high {
            let mid = (low + high) / 2;
            if self.key(side, mid).as_bytes() < code.as_bytes() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }
    pub fn find(&self, side: Side, code: &str) -> Option<usize> {
        let index = self.lower_bound(side, code);
        (index < self.key_count(side) && self.key(side, index) == code).then_some(index)
    }
    pub fn contains_key(&self, side: Side, code: &str) -> bool {
        self.find(side, code).is_some()
    }
    pub fn entry(&self, side: Side, index: usize) -> EntryRef<'_> {
        let at = self.entries[Self::side(side)] + index * ENTRY_SIZE;
        EntryRef {
            text: self.string(self.word(at), self.word(at + 4)),
            syllables: self.string(self.word(at + 8), self.word(at + 12)),
            weight: self.word(at + 16) as u64 | (self.word(at + 20) as u64) << 32,
        }
    }
    /// Entries under the `index`th key, most frequent first.
    pub fn entries_at(&self, side: Side, index: usize) -> impl Iterator<Item = EntryRef<'_>> {
        let range = self.first_entry(side, index)..self.first_entry(side, index + 1);
        range.map(move |i| self.entry(side, i))
    }
    /// Entries for exactly `code` (none if absent).
    pub fn get(&self, side: Side, code: &str) -> impl Iterator<Item = EntryRef<'_>> {
        let range = match self.find(side, code) {
            Some(index) => self.first_entry(side, index)..self.first_entry(side, index + 1),
            None => 0..0,
        };
        range.map(move |i| self.entry(side, i))
    }
    /// Every entry, in key order.
    pub fn all(&self, side: Side) -> impl Iterator<Item = EntryRef<'_>> {
        let count = self.first_entry(side, self.key_count(side));
        (0..count).map(move |i| self.entry(side, i))
    }
    pub fn abbreviation_count(&self) -> usize {
        self.abbreviation_count
    }
    /// The `index`th multi-syllable pinyin word in initials order.
    pub fn abbreviation(&self, index: usize) -> EntryRef<'_> {
        let entry = self.word(self.abbreviations + index * 4) as usize;
        self.entry(Side::Pinyin, entry)
    }
    /// (shortest, longest) Wubi code of a character.
    pub fn codes_of(&self, ch: char) -> Option<(&str, &str)> {
        let (mut low, mut high) = (0, self.code_count);
        let wanted = ch as u32;
        while low < high {
            let mid = (low + high) / 2;
            let at = self.codes + mid * CODE_SIZE;
            match self.word(at).cmp(&wanted) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    let short = self.word(at + 4) as usize;
                    let long = self.word(at + 8) as usize;
                    return Some((self.key(Side::Wubi, short), self.key(Side::Wubi, long)));
                }
            }
        }
        None
    }
    /// Decodes one side back into a table (to merge more words at run time).
    pub fn decode(&self, side: Side) -> RawTable {
        let mut table = RawTable::new();
        for index in 0..self.key_count(side) {
            table.insert(
                self.key(side, index).to_owned(),
                self.entries_at(side, index)
                    .map(|entry| RawEntry {
                        text: entry.text.to_owned(),
                        weight: entry.weight,
                        syllables: entry.syllables.to_owned(),
                    })
                    .collect(),
            );
        }
        table
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&data[at..at + 4]);
    u32::from_le_bytes(word)
}

// ---- Large lexicon (rime-frost, `word<TAB>pinyin<TAB>frequency<TAB>Wubi code` per line) ----

/// Rewrites `lve`/`nve` syllables in the pinyin column to `lue`/`nue` in place (same length).
pub fn normalize_frequency_pinyin(text: String) -> String {
    if !text.contains("ve") {
        return text;
    }
    let mut bytes = text.into_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| start + i);
        let line = &mut bytes[start..end];
        if let Some(first_tab) = line.iter().position(|b| *b == b'\t') {
            let field_end = line[first_tab + 1..]
                .iter()
                .position(|b| *b == b'\t')
                .map_or(line.len(), |i| first_tab + 1 + i);
            let field = &mut line[first_tab + 1..field_end];
            let mut i = 0;
            while i + 3 <= field.len() {
                let bounded = (i == 0 || field[i - 1] == b' ')
                    && (i + 3 == field.len() || field[i + 3] == b' ');
                if bounded && (&field[i..i + 3] == b"lve" || &field[i..i + 3] == b"nve") {
                    field[i + 1] = b'u';
                }
                i += 1;
            }
        }
        start = end + 1;
    }
    String::from_utf8(bytes).unwrap_or_default()
}

/// 64-bit FNV-1a, to recognise the lexicon file the index was precompiled for.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Fields of one lexicon line: (word, pinyin, frequency, Wubi code).
pub fn frost_fields(line: &str) -> (&str, &str, u64, &str) {
    let mut fields = line.split('\t');
    (
        fields.next().unwrap_or(""),
        fields.next().unwrap_or(""),
        fields.next().and_then(|w| w.parse().ok()).unwrap_or(0),
        fields.next().unwrap_or(""),
    )
}

/// Line ranges plus line numbers sorted by pinyin key (without spaces), by Wubi code and by
/// folded initials of multi-syllable words, most frequent first within a key.
pub struct FrostIndex {
    pub lines: Vec<(u32, u32)>,
    pub by_pinyin: Vec<u32>,
    pub by_code: Vec<u32>,
    /// Built on demand at run time (None) or precompiled.
    pub abbreviations: Option<Vec<u32>>,
}

/// Line ranges of the valid (non-comment, four-field) lines of normalised lexicon text.
pub fn frost_lines(text: &str) -> Vec<(u32, u32)> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if !body.starts_with('#') && body.split('\t').count() >= 4 && text.len() < u32::MAX as usize
        {
            lines.push((offset as u32, (offset + body.len()) as u32));
        }
        offset += line.len();
    }
    lines
}

fn frost_line<'a>(
    text: &'a str,
    lines: &[(u32, u32)],
    index: u32,
) -> (&'a str, &'a str, u64, &'a str) {
    let (start, end) = lines[index as usize];
    frost_fields(&text[start as usize..end as usize])
}

/// Line numbers by descending frequency (stable).
pub fn frost_by_weight(text: &str, lines: &[(u32, u32)]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..lines.len() as u32).collect();
    order.sort_by_cached_key(|i| std::cmp::Reverse(frost_line(text, lines, *i).2));
    order
}

/// Initials order of multi-syllable words (see [`FrostIndex::abbreviations`]).
pub fn frost_abbreviations(text: &str, lines: &[(u32, u32)]) -> Vec<u32> {
    // Frequency order first, so the stable sort keeps the most frequent words first within
    // each key.
    let mut order: Vec<u32> = frost_by_weight(text, lines)
        .into_iter()
        .filter(|i| frost_line(text, lines, *i).1.contains(' '))
        .collect();
    order.sort_by_cached_key(|i| {
        folded_initials(frost_line(text, lines, *i).1).collect::<Vec<u8>>()
    });
    order
}

/// Builds the index of normalised lexicon text (abbreviations only when `with_abbreviations`).
pub fn frost_index(text: &str, with_abbreviations: bool) -> FrostIndex {
    let lines = frost_lines(text);
    // Sort by frequency first, then stably by key, so each key lists its most frequent words
    // first.
    let by_weight = frost_by_weight(text, &lines);
    let mut by_pinyin = by_weight.clone();
    by_pinyin.sort_by_cached_key(|i| {
        frost_line(text, &lines, *i)
            .1
            .bytes()
            .filter(|b| *b != b' ')
            .collect::<Vec<u8>>()
    });
    let mut by_code = by_weight;
    by_code.sort_by_cached_key(|i| frost_line(text, &lines, *i).3.to_owned());
    let abbreviations = with_abbreviations.then(|| frost_abbreviations(text, &lines));
    FrostIndex {
        lines,
        by_pinyin,
        by_code,
        abbreviations,
    }
}

pub const FROST_MAGIC: &[u8; 8] = b"YIMEFRS1";

/// Serialises a precompiled lexicon index for the raw file with hash `hash` and length `len`.
pub fn encode_frost(hash: u64, len: usize, index: &FrostIndex) -> Vec<u8> {
    let mut out = FROST_MAGIC.to_vec();
    out.extend_from_slice(&hash.to_le_bytes());
    out.extend_from_slice(&(len as u64).to_le_bytes());
    let abbreviations = index.abbreviations.as_deref().unwrap_or(&[]);
    for count in [
        index.lines.len(),
        index.by_pinyin.len(),
        index.by_code.len(),
        abbreviations.len(),
    ] {
        push_u32(&mut out, count as u32);
    }
    for (start, end) in &index.lines {
        push_u32(&mut out, *start);
        push_u32(&mut out, *end);
    }
    for list in [&index.by_pinyin[..], &index.by_code[..], abbreviations] {
        for value in list {
            push_u32(&mut out, *value);
        }
    }
    out
}

/// The precompiled index if it was built for exactly this raw file content.
pub fn decode_frost(blob: &[u8], raw: &[u8]) -> Option<FrostIndex> {
    if blob.len() < 40 || &blob[..8] != FROST_MAGIC {
        return None;
    }
    let u64_at = |at: usize| {
        let mut word = [0u8; 8];
        word.copy_from_slice(&blob[at..at + 8]);
        u64::from_le_bytes(word)
    };
    if u64_at(16) != raw.len() as u64 || u64_at(8) != fnv1a(raw) {
        return None;
    }
    let counts: Vec<usize> = (0..4)
        .map(|i| read_u32(blob, 24 + i * 4) as usize)
        .collect();
    if blob.len() != 40 + (counts[0] * 2 + counts[1] + counts[2] + counts[3]) * 4 {
        return None;
    }
    let mut at = 40;
    let mut take = |count: usize| {
        let list: Vec<u32> = (0..count).map(|i| read_u32(blob, at + i * 4)).collect();
        at += count * 4;
        list
    };
    let flat = take(counts[0] * 2);
    let lines = flat.chunks(2).map(|pair| (pair[0], pair[1])).collect();
    let by_pinyin = take(counts[1]);
    let by_code = take(counts[2]);
    let abbreviations = take(counts[3]);
    Some(FrostIndex {
        lines,
        by_pinyin,
        by_code,
        abbreviations: Some(abbreviations),
    })
}
