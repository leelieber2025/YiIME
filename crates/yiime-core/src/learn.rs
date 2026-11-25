//! Learning: remembers which candidate is picked for each code and ranks well-established picks
//! first next time (exact matches only).
//!
//! Picks are aggregated per `(code, word)` into a decayed score and the time of the last pick, so
//! the store never grows with typing (it is compacted to [`MAX_ENTRIES`]) and is not a log of what
//! was typed. A pick adds 1 to the score; the score halves every [`HALF_LIFE_DAYS`]. A word is
//! only promoted once its current score reaches [`PROMOTE_SCORE`] (about two recent picks), so one
//! accidental pick no longer beats the dictionary's most frequent word forever.
//!
//! The file ([`LEARNED_FILE`]) is encrypted with the same [`Sealer`] as the input history (DPAPI on
//! Windows) and replaced atomically (written to a temporary file, then renamed). The old plain-text
//! `learned.txt` pick log ([`LEGACY_LEARNED_FILE`]) is migrated on first load and then deleted.

use crate::dict::Candidate;
use crate::settings::{PlainSealer, Sealer};
use crate::write_atomically;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Encrypted learning table.
pub const LEARNED_FILE: &str = "learned.dat";
/// Plain-text pick log of versions up to 0.1.5 (one `code<TAB>word` per pick), migrated and
/// removed.
pub const LEGACY_LEARNED_FILE: &str = "learned.txt";
/// Scores halve after this many days without picks.
pub const HALF_LIFE_DAYS: f64 = 30.0;
/// Current score a word needs before it is ranked ahead of dictionary order.
pub const PROMOTE_SCORE: f64 = 1.9;
/// Most `(code, word)` pairs kept; the weakest are dropped first.
pub const MAX_ENTRIES: usize = 20_000;
/// Entries whose score decayed below this are dropped when saving.
const FORGET_SCORE: f64 = 0.05;
/// Saves are spaced at least this far apart while typing; [`Learned::flush`] (also run on drop)
/// writes pending changes.
const SAVE_INTERVAL: Duration = Duration::from_secs(2);
const HEADER: &str = "yiime-learned 2";

#[derive(Debug, Clone, Copy, PartialEq)]
struct Stat {
    /// Score at `last` (seconds since the Unix epoch).
    score: f64,
    last: u64,
}

impl Stat {
    fn score_at(&self, now: u64) -> f64 {
        let days = now.saturating_sub(self.last) as f64 / 86_400.0;
        self.score * 0.5f64.powf(days / HALF_LIFE_DAYS)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

/// Outcome of [`Learned::forget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetResult {
    /// No learned record existed for `(code, word)`.
    NotFound,
    /// Removed from memory and persisted to disk.
    Persisted,
    /// Removed from memory only (saving to disk failed).
    MemoryOnly,
}

pub struct Learned {
    stats: HashMap<String, HashMap<String, Stat>>,
    path: Option<PathBuf>,
    sealer: Box<dyn Sealer>,
    /// Number of `(code, word)` pairs in `stats`.
    count: usize,
    migrated_legacy: bool,
    /// Existing file could not be read or decrypted on load: do not overwrite it; merge on recovery.
    unreadable: bool,
    forgotten: HashSet<(String, String)>,
    dirty: bool,
    last_save: Option<Instant>,
}

impl Default for Learned {
    fn default() -> Self {
        Self {
            stats: HashMap::new(),
            path: None,
            sealer: Box::new(PlainSealer),
            count: 0,
            dirty: false,
            migrated_legacy: false,
            unreadable: false,
            forgotten: HashSet::new(),
            last_save: None,
        }
    }
}

impl std::fmt::Debug for Learned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Learned")
            .field("codes", &self.stats.len())
            .field("path", &self.path)
            .finish()
    }
}

impl Learned {
    /// Reads the encrypted table at `path` (empty if missing or unreadable), migrating a legacy
    /// `learned.txt` next to it. `None` keeps records in memory only.
    pub fn load(path: Option<PathBuf>, sealer: Box<dyn Sealer>) -> Self {
        let mut learned = Self {
            stats: HashMap::new(),
            path,
            sealer,
            count: 0,
            dirty: false,
            migrated_legacy: false,
            unreadable: false,
            forgotten: HashSet::new(),
            last_save: None,
        };
        let Some(path) = learned.path.clone() else {
            return learned;
        };
        match std::fs::read(&path) {
            Ok(bytes) => match learned.sealer.open(&bytes) {
                Some(plain) => {
                    let ok = std::str::from_utf8(&plain)
                        .ok()
                        .is_some_and(|text| learned.parse(text));
                    if !ok {
                        learned.unreadable = true;
                        return learned;
                    }
                }
                None => {
                    // Decrypt failure: keep original file intact until recovery.
                    learned.unreadable = true;
                    return learned;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                // Transient read error: keep original file intact until recovery.
                learned.unreadable = true;
                return learned;
            }
        }
        let legacy = path.with_file_name(LEGACY_LEARNED_FILE);
        if !learned.migrated_legacy {
            if let Ok(text) = std::fs::read_to_string(&legacy) {
                // Old picks have no time; count them as made when the log was last written.
                let when = std::fs::metadata(&legacy)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or_else(now_secs, |time| time.as_secs());
                for line in text.lines() {
                    if let Some((code, word)) = line.split_once('\t') {
                        learned.bump(code, word, when);
                    }
                }
                learned.migrated_legacy = true;
                learned.dirty = true;
                if learned.save() {
                    let _ = std::fs::remove_file(&legacy);
                }
            }
        }
        learned
    }

    fn parse(&mut self, text: &str) -> bool {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return false;
        }
        for line in lines {
            if line == "# migrated legacy" {
                self.migrated_legacy = true;
                continue;
            }
            let mut fields = line.split('\t');
            let (Some(code), Some(word), Some(score), Some(last)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(score), Ok(last)) = (score.parse::<f64>(), last.parse::<u64>()) else {
                continue;
            };
            if score.is_finite()
                && score > 0.0
                && self
                    .stats
                    .entry(code.to_owned())
                    .or_default()
                    .insert(word.to_owned(), Stat { score, last })
                    .is_none()
            {
                self.count += 1;
            }
        }
        true
    }

    fn bump(&mut self, code: &str, word: &str, now: u64) {
        let words = self.stats.entry(code.to_owned()).or_default();
        if !words.contains_key(word) {
            self.count += 1;
        }
        let stat = words.entry(word.to_owned()).or_insert(Stat {
            score: 0.0,
            last: now,
        });
        stat.score = stat.score_at(now) + 1.0;
        stat.last = stat.last.max(now);
    }

    /// Current (decayed) score of `word` under `code`; 0 if never picked.
    pub fn score(&self, code: &str, word: &str) -> f64 {
        self.score_at(code, word, now_secs())
    }

    pub fn score_at(&self, code: &str, word: &str, now: u64) -> f64 {
        self.stats
            .get(code)
            .and_then(|words| words.get(word))
            .map_or(0.0, |stat| stat.score_at(now))
    }

    /// Number of `(code, word)` pairs kept.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records one pick; returns false (not recorded) if the code is empty or has a TAB, or the
    /// word has a TAB or newline.
    pub fn record(&mut self, code: &str, word: &str) -> bool {
        self.record_at(code, word, now_secs())
    }

    pub fn record_at(&mut self, code: &str, word: &str, now: u64) -> bool {
        if code.is_empty() || code.contains(['\t', '\n']) || word.contains(['\t', '\n']) {
            return false;
        }
        self.forgotten.remove(&(code.to_owned(), word.to_owned()));
        self.bump(code, word, now);
        self.dirty = true;
        // Compact with some slack so a full store does not re-sort on every pick.
        if self.len() > MAX_ENTRIES + MAX_ENTRIES / 20 {
            self.compact(now);
        }
        let due = self
            .last_save
            .is_none_or(|last| last.elapsed() >= SAVE_INTERVAL);
        if due {
            self.save();
        }
        true
    }

    /// Deletes the record of `word` under `code` (Ctrl+Shift+digit in Microsoft Pinyin,
    /// Ctrl+Delete in Rime) and saves at once, reporting whether the delete was persisted.
    pub fn forget(&mut self, code: &str, word: &str) -> ForgetResult {
        let removed = self
            .stats
            .get_mut(code)
            .and_then(|words| words.remove(word))
            .is_some();
        if !removed {
            return ForgetResult::NotFound;
        }
        self.count -= 1;
        if self.stats.get(code).is_some_and(HashMap::is_empty) {
            self.stats.remove(code);
        }
        if self.unreadable {
            self.forgotten.insert((code.to_owned(), word.to_owned()));
        }
        self.dirty = true;
        if self.save() {
            ForgetResult::Persisted
        } else {
            ForgetResult::MemoryOnly
        }
    }

    /// Forgets everything (and deletes the file).
    pub fn clear(&mut self) -> bool {
        self.stats.clear();
        self.count = 0;
        self.dirty = false;
        match &self.path {
            Some(path) if path.exists() => std::fs::remove_file(path).is_ok(),
            _ => true,
        }
    }

    /// Drops decayed entries and keeps the [`MAX_ENTRIES`] strongest.
    fn compact(&mut self, now: u64) {
        let mut all: Vec<(f64, String, String)> = Vec::new();
        for (code, words) in &self.stats {
            for (word, stat) in words {
                let score = stat.score_at(now);
                if score >= FORGET_SCORE {
                    all.push((score, code.clone(), word.clone()));
                }
            }
        }
        if all.len() > MAX_ENTRIES {
            all.sort_by(|a, b| b.0.total_cmp(&a.0));
            all.truncate(MAX_ENTRIES);
        }
        let mut kept: HashMap<String, HashMap<String, Stat>> = HashMap::new();
        self.count = all.len();
        for (_, code, word) in all {
            if let Some(stat) = self.stats.get(&code).and_then(|words| words.get(&word)) {
                kept.entry(code).or_default().insert(word, *stat);
            }
        }
        self.stats = kept;
    }

    /// Writes pending changes now (also done on drop).
    pub fn flush(&mut self) -> bool {
        !self.dirty || self.save()
    }

    fn save(&mut self) -> bool {
        self.last_save = Some(Instant::now());
        let Some(path) = self.path.clone() else {
            self.dirty = false;
            return true;
        };
        if self.unreadable {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let Some(plain) = self.sealer.open(&bytes) else {
                        return false;
                    };
                    // Recovered: parse disk table and merge in-memory picks/deletes made while unreadable.
                    let mut recovered = Learned::default();
                    let ok = std::str::from_utf8(&plain)
                        .ok()
                        .is_some_and(|text| recovered.parse(text));
                    if !ok {
                        return false;
                    }
                    for (code, words) in std::mem::take(&mut self.stats) {
                        for (word, stat) in words {
                            let target = recovered.stats.entry(code.clone()).or_default();
                            match target.get_mut(&word) {
                                Some(existing) => {
                                    existing.score = existing.score_at(stat.last) + stat.score;
                                    existing.last = existing.last.max(stat.last);
                                }
                                None => {
                                    target.insert(word, stat);
                                    recovered.count += 1;
                                }
                            }
                        }
                    }
                    for (code, word) in std::mem::take(&mut self.forgotten) {
                        if let Some(words) = recovered.stats.get_mut(&code) {
                            if words.remove(&word).is_some() {
                                recovered.count -= 1;
                                if words.is_empty() {
                                    recovered.stats.remove(&code);
                                }
                            }
                        }
                    }
                    self.stats = std::mem::take(&mut recovered.stats);
                    self.count = recovered.count;
                    self.migrated_legacy |= recovered.migrated_legacy;
                    self.unreadable = false;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.forgotten.clear();
                    self.unreadable = false;
                }
                Err(_) => return false,
            }
        }
        self.compact(now_secs());
        let mut text = String::from(HEADER);
        text.push('\n');
        if self.migrated_legacy {
            text.push_str("# migrated legacy\n");
        }
        let mut codes: Vec<_> = self.stats.iter().collect();
        codes.sort_by(|a, b| a.0.cmp(b.0));
        for (code, words) in codes {
            let mut words: Vec<_> = words.iter().collect();
            words.sort_by(|a, b| a.0.cmp(b.0));
            for (word, stat) in words {
                text.push_str(&format!(
                    "{code}\t{word}\t{:.4}\t{}\n",
                    stat.score, stat.last
                ));
            }
        }
        let Some(sealed) = self.sealer.seal(text.as_bytes()) else {
            return false;
        };
        if write_atomically(&path, &sealed) {
            self.dirty = false;
            true
        } else {
            false
        }
    }

    /// Ranks confident exact matches first by current score (stable: candidates below
    /// [`PROMOTE_SCORE`] keep dictionary order after them); prefix candidates stay behind.
    /// User words stay ahead of every learned dictionary word.
    pub fn reorder(&self, code: &str, candidates: &mut [Candidate]) {
        self.reorder_at(code, candidates, now_secs());
    }

    pub fn reorder_at(&self, code: &str, candidates: &mut [Candidate], now: u64) {
        let Some(words) = self.stats.get(code) else {
            return;
        };
        let confident = |text: &str| {
            words
                .get(text)
                .map(|stat| stat.score_at(now))
                .filter(|score| *score >= PROMOTE_SCORE)
        };
        if !candidates
            .iter()
            .any(|candidate| candidate.complete && confident(&candidate.text).is_some())
        {
            return;
        }
        candidates.sort_by(|a, b| {
            let rank = |candidate: &Candidate| {
                (
                    !candidate.user,
                    !candidate.complete,
                    candidate
                        .complete
                        .then(|| confident(&candidate.text))
                        .flatten(),
                )
            };
            let (a_user, a_partial, a_score) = rank(a);
            let (b_user, b_partial, b_score) = rank(b);
            a_user
                .cmp(&b_user)
                .then_with(|| a_partial.cmp(&b_partial))
                .then_with(|| match (a_score, b_score) {
                    (Some(x), Some(y)) => y.total_cmp(&x),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
        });
    }
}

impl Drop for Learned {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MixedInput;

    struct Xor;
    impl Sealer for Xor {
        fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
            Some(data.iter().map(|b| b ^ 0x5A).collect())
        }
        fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
            self.seal(data)
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yiime-learn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const DAY: u64 = 86_400;
    const NOW: u64 = 1_800_000_000;

    fn texts(list: &[Candidate]) -> Vec<&str> {
        list.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn learning_persists_encrypted_and_reorders_confident_picks() {
        let dir = temp_dir("persist");
        let path = dir.join(LEARNED_FILE);
        let wubi = "...\n甲\taaaa\t9\n乙\taaaa\t5\n丙\taaaab\t9\n";
        let input = MixedInput::from_rime(wubi, "");
        let mut learned = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!(learned.record("aaaa", "乙"));
        assert!(!learned.record("", "乙"));
        assert!(!learned.record("aaaa", "a\tb"));
        assert!(learned.flush());
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("aaaa"),
            "file is encrypted"
        );
        // One pick is not enough to beat the dictionary order.
        let reloaded = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!((reloaded.score("aaaa", "乙") - 1.0).abs() < 0.01);
        let mut list = input.candidates("aaaa", 9);
        reloaded.reorder("aaaa", &mut list);
        assert_eq!(texts(&list), ["甲", "乙"]);
        drop(reloaded);
        // A second pick promotes it.
        let mut learned = Learned::load(Some(path.clone()), Box::new(Xor));
        learned.record("aaaa", "乙");
        drop(learned); // flushes
        let reloaded = Learned::load(Some(path.clone()), Box::new(Xor));
        let mut list = input.candidates("aaaa", 9);
        reloaded.reorder("aaaa", &mut list);
        assert_eq!(texts(&list), ["乙", "甲"]);
        drop(reloaded);
        // Forgetting updates memory and file and keeps other records.
        let mut learned = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!(learned.record("aaaa", "甲"));
        assert_eq!(learned.forget("aaaa", "乙"), ForgetResult::Persisted);
        assert_eq!(learned.forget("aaaa", "乙"), ForgetResult::NotFound);
        drop(learned);
        let reloaded = Learned::load(Some(path.clone()), Box::new(Xor));
        assert_eq!(reloaded.score("aaaa", "乙"), 0.0);
        assert!(reloaded.score("aaaa", "甲") > 0.9);
        // A file that does not decrypt is not overwritten when a new pick arrives.
        std::fs::write(&path, b"garbage").unwrap();
        let mut damaged = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!(damaged.is_empty());
        assert!(damaged.record("aaaa", "甲"));
        assert!(!damaged.flush());
        assert_eq!(damaged.forget("aaaa", "甲"), ForgetResult::MemoryOnly);
        assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
        drop(damaged);
        assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
        // No temporary files are left behind.
        drop(reloaded);
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scores_decay_over_time() {
        let mut learned = Learned::default();
        let candidates = || {
            ["甲", "乙", "丙"]
                .into_iter()
                .map(|text| Candidate {
                    text: text.into(),
                    source: crate::Source::Wubi,
                    complete: true,
                    rest: String::new(),
                    user: false,
                })
                .collect::<Vec<_>>()
        };
        for _ in 0..3 {
            learned.record_at("aaaa", "丙", NOW);
        }
        let mut list = candidates();
        learned.reorder_at("aaaa", &mut list, NOW);
        assert_eq!(texts(&list), ["丙", "甲", "乙"]);
        // One half-life later the score has halved and no longer promotes.
        let later = NOW + 30 * DAY;
        assert!((learned.score_at("aaaa", "丙", later) - 1.5).abs() < 0.01);
        let mut list = candidates();
        learned.reorder_at("aaaa", &mut list, later);
        assert_eq!(texts(&list), ["甲", "乙", "丙"]);
        // Recent picks of another word outrank old picks.
        learned.record_at("aaaa", "乙", later);
        learned.record_at("aaaa", "乙", later);
        learned.record_at("aaaa", "丙", later);
        let mut list = candidates();
        learned.reorder_at("aaaa", &mut list, later);
        assert_eq!(texts(&list), ["丙", "乙", "甲"]);
        assert!(learned.score_at("aaaa", "丙", later) > learned.score_at("aaaa", "乙", later));
        // Prefix candidates stay behind complete ones.
        let mut list = candidates();
        list[0].complete = false;
        learned.record_at("aaaa", "甲", later);
        learned.record_at("aaaa", "甲", later);
        learned.record_at("aaaa", "甲", later);
        learned.record_at("aaaa", "甲", later);
        learned.reorder_at("aaaa", &mut list, later);
        assert_eq!(texts(&list)[2], "甲");
        // A user word stays first even when a dictionary word has a higher score.
        let mut list = candidates();
        list[2].user = true;
        learned.reorder_at("aaaa", &mut list, later);
        assert_eq!(texts(&list)[0], "丙");
    }

    #[test]
    fn legacy_log_is_migrated_and_removed() {
        let dir = temp_dir("legacy");
        let legacy = dir.join(LEGACY_LEARNED_FILE);
        std::fs::write(&legacy, "aaaa\t乙\naaaa\t乙\naaaa\t甲\nbroken line\n").unwrap();
        let path = dir.join(LEARNED_FILE);
        let learned = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!(learned.score("aaaa", "乙") > 1.9);
        assert!(learned.score("aaaa", "甲") > 0.9);
        assert!(
            !legacy.exists(),
            "plain-text log is deleted after migration"
        );
        assert!(path.exists());
        drop(learned);
        let reloaded = Learned::load(Some(path), Box::new(Xor));
        assert_eq!(reloaded.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transient_decrypt_failure_preserves_old_file_and_merges_after_recovery() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct Flaky(Arc<AtomicBool>);
        impl Sealer for Flaky {
            fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
                Xor.seal(data)
            }
            fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
                if self.0.load(Ordering::Relaxed) {
                    Xor.open(data)
                } else {
                    None
                }
            }
        }
        let dir = temp_dir("flaky");
        let path = dir.join(LEARNED_FILE);
        let mut initial = Learned::load(Some(path.clone()), Box::new(Xor));
        assert!(initial.record_at("aaaa", "甲", NOW));
        assert!(initial.flush());
        drop(initial);
        let original_bytes = std::fs::read(&path).unwrap();

        // Decryptor fails once on load (and on save while still failing): original bytes stay intact.
        let allow_open = Arc::new(AtomicBool::new(false));
        let mut session = Learned::load(Some(path.clone()), Box::new(Flaky(allow_open.clone())));
        assert!(session.is_empty());
        assert!(session.record_at("bbbb", "乙", NOW + 1));
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

        // Decryptor recovers: flushing merges old ("甲") + new ("乙") instead of overwriting.
        allow_open.store(true, Ordering::Relaxed);
        assert!(session.flush());
        drop(session);

        let recovered = Learned::load(Some(path), Box::new(Xor));
        assert!(recovered.score_at("aaaa", "甲", NOW + 1) > 0.9);
        assert!(recovered.score_at("bbbb", "乙", NOW + 1) > 0.9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_is_compacted_instead_of_stopping() {
        let mut learned = Learned::default();
        for i in 0..MAX_ENTRIES * 2 {
            learned.record_at(&format!("c{i}"), "字", NOW);
        }
        assert!(learned.len() <= MAX_ENTRIES + MAX_ENTRIES / 20);
        learned.compact(NOW);
        assert_eq!(learned.len(), MAX_ENTRIES);
        // New picks are still recorded after compaction.
        assert!(learned.record_at("new", "词", NOW + 1));
        assert!(learned.score_at("new", "词", NOW + 1) > 0.9);
        // Long-decayed entries are dropped.
        learned.compact(NOW + 3650 * DAY);
        assert!(learned.is_empty());
    }
}
