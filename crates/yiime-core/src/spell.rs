//! Error tolerance and coverage of Pinyin: syllable segmentation, correction of common
//! misspellings, mixed spelling of full and simplified spelling, fuzzy sounds (optional), and when
//! the whole sentence is not in the vocabulary, it will be divided into syllables to give the first
//! part of the word (after selection, the remaining syllables will remain in the code for continued
//! selection). Behavior refers to Microsoft Pinyin.

use super::{Arc, BTreeSet, HashSet, MixedInput, folded_initials};

/// List of legal syllables (taken from the Pinyin dictionary itself, excluding interjectional
/// sounds without vowels such as m/n/ng/hm).
pub(super) struct SyllableTable {
    full: HashSet<Box<str>>,
    /// The true prefix of a syllable (zh, zho, gu...), used for "the last paragraph is only half
    /// typed".
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
    fn is_prefix(&self, text: &str) -> bool {
        self.prefix.contains(text)
    }
}

/// Mixed paragraph: the complete syllables must be the same; `Prefix` only requires that the
/// syllable begins with it (initial consonant or half syllable).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece<'a> {
    Full(&'a str),
    Prefix(&'a str),
}

/// Fuzzy sounds: initial consonants z/zh c/ch s/sh n/l, finals an/ang en/eng in/ing (including
/// ian/iang, uan/uang).
const FUZZY_INITIALS: [(&str, &str); 4] = [("zh", "z"), ("ch", "c"), ("sh", "s"), ("n", "l")];
const FUZZY_FINALS: [(&str, &str); 3] = [("ang", "an"), ("eng", "en"), ("ing", "in")];
/// Whole string replacement of common misspellings (there are also two adjacent letters swapped:
/// ou/uo, ei/ie, ui/iu, ng/gn, hz/zh...).
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
    /// All are divided into complete syllables (each is the end position of each syllable), with
    /// the fewest syllables first; `forced` is the position of the isolation note.
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

    /// Correction of misspellings: swapping adjacent letters (without crossing diaphragms), uen→un,
    /// etc. After the correction, the whole string can be cut into syllables and there are words in
    /// the vocabulary. The results are sorted by word frequency (the same word has a higher word
    /// frequency), marked as an error correction candidate, and ranked after the accurate result of
    /// the original encoding.
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
                    .map(|(text, weight, _)| (text, weight)),
            );
        }
        rank(found, limit)
    }

    /// Fuzzy sound (when turned on in settings): Change the syllable to the fuzzy corresponding
    /// sound, and the found words will be ranked after the precise results.
    pub(super) fn fuzzy(&self, code: &str, forced: &[usize], limit: usize) -> Vec<Arc<str>> {
        let mut variants: BTreeSet<String> = BTreeSet::new();
        for ends in self.segmentations(code, forced).into_iter().take(4) {
            let mut partial = vec![String::new()];
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
                for head in &partial {
                    for option in &options {
                        if next.len() < 64 {
                            next.push(format!("{head}{option}"));
                        }
                    }
                }
                partial = next;
            }
            variants.extend(partial);
        }
        variants.remove(code);
        let mut found = Vec::new();
        for variant in &variants {
            found.extend(
                self.homophones(variant)
                    .into_iter()
                    .map(|(text, weight, _)| (text, weight)),
            );
        }
        rank(found, limit)
    }

    /// A mixture of Quan Pin and Simplified Pin: each paragraph is a complete syllable or initial
    /// consonant (the last paragraph can be half a syllable), such as zhongg, zgren, zhongguor,
    /// xiandai'h. At least one section is complete and one section is incomplete (there are
    /// separate ways to check pure Quan Pin and pure Simplified Pin).
    pub(super) fn mixed_abbreviation(&self, code: &str, limit: usize) -> Vec<Arc<str>> {
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
            let start = index
                .rime
                .partition_point(|entry| entry.0.to_ascii_lowercase() < key);
            for (initials, text, weight, syllables) in index.rime[start..]
                .iter()
                .take_while(|entry| entry.0.eq_ignore_ascii_case(&key))
            {
                let _ = initials;
                if matches(syllables) {
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
                    if matches(line.pinyin) {
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
            // Complete syllables (more than two letters; single-letter syllables a/e/o are treated
            // as initial consonants).
            for len in (2..=table.max_len.min(rest.len())).rev() {
                if table.is_syllable(&rest[..len]) {
                    pieces.push(Piece::Full(&rest[..len]));
                    walk(table, code, pos + len, pieces, out);
                    pieces.pop();
                }
            }
            // Half a syllable of the last paragraph (zhonggu → Chinese).
            if rest.len() >= 2 && table.is_prefix(rest) && !table.is_syllable(rest) {
                pieces.push(Piece::Prefix(rest));
                walk(table, code, code.len(), pieces, out);
                pieces.pop();
            }
            // Initial consonants: zh/ch/sh or single letters.
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

    /// The bottom line when the whole string is not in the lexicon: cut it according to syllables,
    /// and take the longest front part that can be cut into syllables. Return (whole sentence
    /// candidate, previous paragraph candidate [(word, remaining code)]). The entire sentence is
    /// spelled out using the minimum number of words (the number of the same words is the one with
    /// the highest frequency).
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
        // The remaining tail that has not been cut into syllables can only be half a syllable (the
        // r in zhongguor); English words such as hello are not full of details, and the spaces
        // remain on the screen as they are.
        let tail = &code[covered..];
        if !tail.is_empty() && !self.syllables().is_prefix(tail) {
            return (None, Vec::new());
        }
        let mut starts = vec![0];
        starts.extend(ends.iter().copied());
        let count = ends.len();
        // Whole sentence: best[j] = (number of words, -Σln(word frequency+1), text), the best
        // spelling of syllable 0..j.
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

/// (whole sentence candidate, first paragraph candidate [(word, remaining code)]).
type Fallback = (Option<Arc<str>>, Vec<(Arc<str>, String)>);

/// Sort by word frequency in descending order and remove duplicate words from the same word.
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
