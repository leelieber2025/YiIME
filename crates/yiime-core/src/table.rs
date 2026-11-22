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
            (code.replace(' ', ""), code.to_owned())
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
    pub fn as_bytes(&self) -> &[u8] {
        self.data()
    }
    /// Size of the blob in bytes.
    pub fn len(&self) -> usize {
        self.data().len()
    }
    pub fn is_empty(&self) -> bool {
        self.key_count == [0, 0]
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
