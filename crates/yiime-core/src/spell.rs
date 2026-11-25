//! Pinyin tolerance: syllable segmentation, common typo correction, mixed full syllables and
//! initials, optional fuzzy pinyin, and, when the whole code is not a word, a leading-syllables
//! word with the rest left in the code for the next pick. Behaviour follows Microsoft Pinyin.

use super::{Arc, BTreeSet, HashSet, MixedInput, folded_initials, partition};
use std::collections::BTreeMap;

/// Valid syllables, taken from the pinyin dictionary (without vowel-less interjections such as
/// m/n/ng/hm).
pub(super) struct SyllableTable {
    full: HashSet<Box<str>>,
    /// Proper prefixes of syllables (zh, zho, gu…), for a last syllable that is only half typed.
    prefix: HashSet<Box<str>>,
    max_len: usize,
}

impl SyllableTable {
    pub(super) fn new<'a>(syllables: impl Iterator<Item = &'a str>) -> Self {
        let mut full: HashSet<Box<str>> = HashSet::new();
        for syllable in syllables {
            if !syllable.is_empty()
                && syllable.bytes().all(|b| b.is_ascii_lowercase())
                && syllable.bytes().any(|b| b"aeiouv".contains(&b))
            {
                full.insert(syllable.into());
            }
        }
        let mut prefix = HashSet::new();
        for syllable in &full {
            for end in 1..syllable.len() {
                prefix.insert(Box::from(&syllable[..end]));
            }
        }
        let max_len = full.iter().map(|s| s.len()).max().unwrap_or(0);
        Self {
            full,
            prefix,
            max_len,
        }
    }
    pub(super) fn is_syllable(&self, text: &str) -> bool {
        self.full.contains(text)
    }
    /// Whether `code` splits into whole syllables (`xian` → xi·an or xian).
    pub(super) fn spells(&self, code: &str) -> bool {
        if code.is_empty() || !code.is_ascii() {
            return false;
        }
        // reachable[i]: code[..i] is a run of syllables.
        let mut reachable = vec![false; code.len() + 1];
        reachable[0] = true;
        for start in 0..code.len() {
            if !reachable[start] {
                continue;
            }
            for end in start + 1..=code.len().min(start + self.max_len) {
                if self.is_syllable(&code[start..end]) {
                    reachable[end] = true;
                }
            }
        }
        reachable[code.len()]
    }
    fn is_prefix(&self, text: &str) -> bool {
        self.prefix.contains(text)
    }
}

/// Segment of mixed input: a full syllable must match exactly; `Prefix` only requires the syllable
/// to start with it (an initial or half a syllable).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece<'a> {
    Full(&'a str),
    Prefix(&'a str),
}

/// Fuzzy pinyin: initials z/zh c/ch s/sh n/l and finals an/ang en/eng in/ing (including ian/iang,
/// uan/uang).
const FUZZY_INITIALS: [(&str, &str); 4] = [("zh", "z"), ("ch", "c"), ("sh", "s"), ("n", "l")];
const FUZZY_FINALS: [(&str, &str); 3] = [("ang", "an"), ("eng", "en"), ("ing", "in")];
/// Whole-code replacements for common typos (swapped adjacent letters such as ou/uo, ei/ie, ui/iu,
/// ng/gn, hz/zh are handled separately).
const SPELLING_FIXES: [(&str, &str); 4] =
    [("uen", "un"), ("iou", "iu"), ("uei", "ui"), ("vn", "un")];

fn fuzzy_alternatives(syllable: &str) -> Vec<String> {
    let mut heads = vec![syllable.to_owned()];
    for (long, short) in FUZZY_INITIALS {
        if let Some(rest) = syllable.strip_prefix(long) {
            heads.push(format!("{short}{rest}"));
        } else if let Some(rest) = syllable.strip_prefix(short) {
            heads.push(format!("{long}{rest}"));
        }
    }
    let mut out = Vec::new();
    for head in heads {
        for (long, short) in FUZZY_FINALS {
            if let Some(stem) = head.strip_suffix(long) {
                out.push(format!("{stem}{short}"));
            } else if let Some(stem) = head.strip_suffix(short) {
                out.push(format!("{stem}{long}"));
            }
        }
        out.push(head);
    }
    out
}

impl MixedInput {
    /// Every split into full syllables (end offset of each), fewest syllables first; `forced` holds
    /// the `'` positions.
    pub(super) fn segmentations(&self, code: &str, forced: &[usize]) -> Vec<Vec<usize>> {
        fn walk(
            table: &SyllableTable,
            code: &str,
            forced: &[usize],
            pos: usize,
            ends: &mut Vec<usize>,
            out: &mut Vec<Vec<usize>>,
        ) {
            if out.len() >= 16 {
                return;
            }
            if pos == code.len() {
                out.push(ends.clone());
                return;
            }
            for len in (1..=table.max_len.min(code.len() - pos)).rev() {
                let end = pos + len;
                if forced.iter().any(|f| *f > pos && *f < end) {
                    continue;
                }
                if table.is_syllable(&code[pos..end]) {
                    ends.push(end);
                    walk(table, code, forced, end, ends, out);
                    ends.pop();
                }
            }
        }
        if code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }
        let mut out = Vec::new();
        walk(self.syllables(), code, forced, 0, &mut Vec::new(), &mut out);
        out.sort_by_key(Vec::len);
        out
    }

    /// Typo correction: swap adjacent letters (not across `'`), uen→un and the like, keeping
    /// results that split into syllables and are words. Sorted by frequency and ranked after exact
    /// matches of the original code.
    pub(super) fn corrections(&self, code: &str, forced: &[usize], limit: usize) -> Vec<Arc<str>> {
        if code.len() < 3 || code.len() > 24 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }
        let bytes = code.as_bytes();
        let mut variants: BTreeSet<String> = BTreeSet::new();
        for i in 0..bytes.len() - 1 {
            if bytes[i] != bytes[i + 1] && !forced.contains(&(i + 1)) {
                let mut swapped = bytes.to_vec();
                swapped.swap(i, i + 1);
                variants.insert(String::from_utf8(swapped).unwrap_or_default());
            }
        }
        if forced.is_empty() {
            for (wrong, right) in SPELLING_FIXES {
                if code.contains(wrong) {
                    variants.insert(code.replace(wrong, right));
                }
            }
        }
        variants.remove(code);
        let mut found: Vec<(Arc<str>, u64)> = Vec::new();
        for variant in &variants {
            if self.segmentations(variant, forced).is_empty() {
                continue;
            }
            found.extend(
                self.homophones(variant)
                    .into_iter()
                    .filter(|(_, _, syllables)| {
                        let own = crate::dict::boundaries(syllables, ' ');
                        forced.iter().all(|offset| own.contains(offset))
                    })
                    .map(|(text, weight, _)| (text, weight)),
            );
        }
        rank(found, limit)
    }

    /// Fuzzy pinyin (when enabled): swap syllables for their fuzzy counterparts; results go after
    /// exact matches.
    pub(super) fn fuzzy(&self, code: &str, forced: &[usize], limit: usize) -> Vec<Arc<str>> {
        let mut variants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for ends in self.segmentations(code, forced).into_iter().take(4) {
            let mut partial: Vec<(String, String)> = vec![(String::new(), String::new())];
            let mut start = 0;
            for end in ends {
                let syllable = &code[start..end];
                start = end;
                let options: Vec<String> = fuzzy_alternatives(syllable)
                    .into_iter()
                    .filter(|alt| self.syllables().is_syllable(alt))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let mut next = Vec::new();
                for (head, head_syl) in &partial {
                    for option in &options {
                        if next.len() < 64 {
                            let joined = format!("{head}{option}");
                            let syl = if head_syl.is_empty() {
                                option.clone()
                            } else {
                                format!("{head_syl} {option}")
                            };
                            next.push((joined, syl));
                        }
                    }
                }
                partial = next;
            }
            for (joined, syl) in partial {
                if joined != code {
                    variants.entry(joined).or_default().insert(syl);
                }
            }
        }
        let mut found = Vec::new();
        for (variant, expected_syllables) in &variants {
            found.extend(
                self.homophones(variant)
                    .into_iter()
                    .filter(|(_, _, syllables)| {
                        forced.is_empty() || expected_syllables.contains(syllables.as_ref())
                    })
                    .map(|(text, weight, _)| (text, weight)),
            );
        }
        rank(found, limit)
    }

    /// Mixed full syllables and initials: each segment is a full syllable or an initial (the last
    /// may be half a syllable), e.g. zhongg, zgren, zhongguor, xiandai'h. At least one segment is
    /// full and one is not; pure full pinyin and pure initials are handled elsewhere.
    pub(super) fn mixed_abbreviation(
        &self,
        code: &str,
        limit: usize,
        min_weight: u64,
    ) -> Vec<Arc<str>> {
        if code.len() < 3 || code.len() > 24 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
            return Vec::new();
        }
        let parses = self.mixed_parses(code);
        if parses.is_empty() {
            return Vec::new();
        }
        let index = self.abbreviation_index();
        let mut found: Vec<(Arc<str>, u64)> = Vec::new();
        for pieces in &parses {
            let key: String = pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Full(text) | Piece::Prefix(text) => text.as_bytes()[0] as char,
                })
                .collect();
            let matches = |syllables: &str| {
                let own: Vec<&str> = syllables.split(' ').filter(|s| !s.is_empty()).collect();
                own.len() == pieces.len()
                    && own.iter().zip(pieces).all(|(have, piece)| match piece {
                        Piece::Full(want) => have == want,
                        Piece::Prefix(want) => have.starts_with(want),
                    })
            };
            let table = &self.table;
            let count = table.abbreviation_count();
            let start = partition(count, |i| {
                folded_initials(table.abbreviation(i).syllables).lt(key.bytes())
            });
            for i in start..count {
                let entry = table.abbreviation(i);
                if !folded_initials(entry.syllables).eq(key.bytes()) {
                    break;
                }
                if entry.weight >= min_weight && matches(entry.syllables) {
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
                    if line.weight >= min_weight && matches(line.pinyin) {
                        found.push((Arc::from(line.text), line.weight));
                    }
                }
            }
        }
        rank(found, limit)
    }

    fn mixed_parses<'a>(&self, code: &'a str) -> Vec<Vec<Piece<'a>>> {
        fn walk<'a>(
            table: &SyllableTable,
            code: &'a str,
            pos: usize,
            pieces: &mut Vec<Piece<'a>>,
            out: &mut Vec<Vec<Piece<'a>>>,
        ) {
            if out.len() >= 16 {
                return;
            }
            if pos == code.len() {
                let full = pieces
                    .iter()
                    .filter(|p| matches!(p, Piece::Full(_)))
                    .count();
                if pieces.len() >= 2 && full >= 1 && full < pieces.len() {
                    out.push(pieces.clone());
                }
                return;
            }
            let rest = &code[pos..];
            // Full syllables of two or more letters (single-letter a/e/o count as initials).
            for len in (2..=table.max_len.min(rest.len())).rev() {
                if table.is_syllable(&rest[..len]) {
                    pieces.push(Piece::Full(&rest[..len]));
                    walk(table, code, pos + len, pieces, out);
                    pieces.pop();
                }
            }
            // Half-typed last syllable (`zhonggu` → 中国).
            if rest.len() >= 2 && table.is_prefix(rest) && !table.is_syllable(rest) {
                pieces.push(Piece::Prefix(rest));
                walk(table, code, code.len(), pieces, out);
                pieces.pop();
            }
            // Initials: zh/ch/sh or a single letter.
            if rest.len() >= 2 && matches!(&rest[..2], "zh" | "ch" | "sh") {
                pieces.push(Piece::Prefix(&rest[..2]));
                walk(table, code, pos + 2, pieces, out);
                pieces.pop();
            }
            if table.is_prefix(&rest[..1]) || table.is_syllable(&rest[..1]) {
                pieces.push(Piece::Prefix(&rest[..1]));
                walk(table, code, pos + 1, pieces, out);
                pieces.pop();
            }
        }
        let mut out = Vec::new();
        walk(self.syllables(), code, 0, &mut Vec::new(), &mut out);
        out
    }

    /// Fallback when the whole code is not a word: split it into syllables, taking the longest
    /// leading part that splits. Returns (whole-sentence candidate, leading candidates [(word,
    /// remaining code)]). The sentence uses the fewest words, then the highest frequency.
    pub(super) fn segment_fallback(
        &self,
        code: &str,
        forced: &[usize],
        per_prefix: usize,
    ) -> Fallback {
        let Some((covered, ends)) = (1..=code.len()).rev().find_map(|end| {
            let inner: Vec<usize> = forced.iter().copied().filter(|f| *f < end).collect();
            self.segmentations(&code[..end], &inner)
                .into_iter()
                .next()
                .map(|ends| (end, ends))
        }) else {
            return (None, Vec::new());
        };
        // The uncovered tail can only be half a syllable (the `r` in `zhongguor`); English words
        // like `hello` do not split and stay as typed.
        let tail = &code[covered..];
        if !tail.is_empty() && !self.syllables().is_prefix(tail) {
            return (None, Vec::new());
        }
        let mut starts = vec![0];
        starts.extend(ends.iter().copied());
        let count = ends.len();
        // Whole sentence: best[j] = (word count, -Σln(frequency+1), text) for syllables 0..j.
        let mut sentence = None;
        if covered == code.len() && count >= 2 {
            let mut best: Vec<Option<(usize, f64, String)>> = vec![None; count + 1];
            best[0] = Some((0, 0.0, String::new()));
            for j in 1..=count {
                for i in j.saturating_sub(8)..j {
                    let Some((words, cost, text)) = best[i].clone() else {
                        continue;
                    };
                    let key = &code[starts[i]..starts[j]];
                    let Some((word, weight, _)) = self.homophones(key).into_iter().next() else {
                        continue;
                    };
                    let candidate = (words + 1, cost - ((weight as f64) + 1.0).ln(), text + &word);
                    let better = best[j]
                        .as_ref()
                        .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1));
                    if better {
                        best[j] = Some(candidate);
                    }
                }
            }
            if let Some((words, _, text)) = best[count].clone() {
                if words >= 2 {
                    sentence = Some(Arc::from(text.as_str()));
                }
            }
        }
        let mut partial = Vec::new();
        for j in (1..=count).rev() {
            let end = starts[j];
            if end == code.len() {
                continue;
            }
            for (text, _, _) in self.homophones(&code[..end]).into_iter().take(per_prefix) {
                partial.push((text, code[end..].to_owned()));
            }
        }
        (sentence, partial)
    }
}

/// (whole-sentence candidate, leading candidates [(word, remaining code)]).
type Fallback = (Option<Arc<str>>, Vec<(Arc<str>, String)>);

/// Sorts by frequency (descending) and removes duplicates.
fn rank(mut found: Vec<(Arc<str>, u64)>, limit: usize) -> Vec<Arc<str>> {
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|(text, _)| seen.insert(text.clone()))
        .take(limit)
        .map(|(text, _)| text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_alternatives_cover_initials_and_finals() {
        let alts = fuzzy_alternatives("zhang");
        for want in ["zhang", "zhan", "zang", "zan"] {
            assert!(alts.iter().any(|a| a == want), "{want} in {alts:?}");
        }
        let alts = fuzzy_alternatives("lin");
        for want in ["lin", "ling", "nin", "ning"] {
            assert!(alts.iter().any(|a| a == want), "{want} in {alts:?}");
        }
    }
}
