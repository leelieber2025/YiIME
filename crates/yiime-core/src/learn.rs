//! Self-study: record the candidates selected under each encoding, and rank the most selected
//! candidates first next time (only the candidates with complete matching will be adjusted).
//! "Encoding <TAB> candidates" for each line of the file can be appended one line at a time. No
//! additional lines will be added after it exceeds 4MB.

use crate::dict::Candidate;
use std::collections::HashMap;
use std::path::PathBuf;

pub const LEARNED_FILE: &str = "learned.txt";
const MAX_FILE_BYTES: u64 = 4 << 20;

#[derive(Debug, Default)]
pub struct Learned {
    counts: HashMap<String, HashMap<String, u32>>,
    path: Option<PathBuf>,
}

impl Learned {
    /// Read the record file (empty if it does not exist); subsequent selections are appended to the
    /// same file. `None` means only stored in memory.
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut learned = Self {
            counts: HashMap::new(),
            path,
        };
        if let Some(text) = learned
            .path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            for line in text.lines() {
                if let Some((code, word)) = line.split_once('\t') {
                    learned.bump(code, word);
                }
            }
        }
        learned
    }
    fn bump(&mut self, code: &str, word: &str) {
        *self
            .counts
            .entry(code.to_owned())
            .or_default()
            .entry(word.to_owned())
            .or_default() += 1;
    }
    pub fn count(&self, code: &str, word: &str) -> u32 {
        self.counts
            .get(code)
            .and_then(|words| words.get(word))
            .copied()
            .unwrap_or(0)
    }
    /// Remember to select once; if it contains TAB/line feed or the encoding is empty, it will not
    /// be remembered. Return whether to remember it.
    pub fn record(&mut self, code: &str, word: &str) -> bool {
        if code.is_empty() || code.contains('\t') || word.contains(['\t', '\n']) {
            return false;
        }
        self.bump(code, word);
        let path = self.path.as_ref().filter(|path| {
            std::fs::metadata(path).map_or(true, |meta| meta.len() < MAX_FILE_BYTES)
        });
        if let Some(path) = path {
            use std::io::Write;
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "{code}\t{word}");
            }
        }
        true
    }
    /// Delete all learning records of the next candidate for a certain code (convention of
    /// Microsoft Pinyin Ctrl+Shift+Number, Rime Ctrl+Delete). Record files are rewritten
    /// synchronously; return whether there are original records.
    pub fn forget(&mut self, code: &str, word: &str) -> bool {
        let removed = self
            .counts
            .get_mut(code)
            .and_then(|words| words.remove(word))
            .is_some();
        if removed {
            if let Some(path) = &self.path {
                if let Ok(text) = std::fs::read_to_string(path) {
                    let wanted = format!("{code}\t{word}");
                    let kept: String = text
                        .lines()
                        .filter(|line| *line != wanted)
                        .map(|line| format!("{line}\n"))
                        .collect();
                    let _ = std::fs::write(path, kept);
                }
            }
        }
        removed
    }
    /// Rearrange according to the number of selections: complete matches are in front (the ones
    /// with the most times are in front, stable sorting), and prefix candidates are still in the
    /// back.
    pub fn reorder(&self, code: &str, candidates: &mut [Candidate]) {
        if let Some(counts) = self.counts.get(code) {
            candidates.sort_by_key(|candidate| {
                (
                    !candidate.complete,
                    std::cmp::Reverse(counts.get(&candidate.text).copied().unwrap_or(0)),
                )
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MixedInput;

    #[test]
    fn learning_persists_and_reorders_complete_candidates() {
        let dir = std::env::temp_dir().join(format!("yiime-learn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(LEARNED_FILE);
        let wubi = "...\n甲\taaaa\t9\n乙\taaaa\t5\n丙\taaaab\t9\n";
        let input = MixedInput::from_rime(wubi, "");
        let mut learned = Learned::load(Some(path.clone()));
        assert!(learned.record("aaaa", "乙"));
        assert!(!learned.record("", "乙"));
        assert!(!learned.record("aaaa", "a\tb"));
        let reloaded = Learned::load(Some(path.clone()));
        assert_eq!(reloaded.count("aaaa", "乙"), 1);
        let mut list = input.candidates("aaaa", 9);
        reloaded.reorder("aaaa", &mut list);
        let texts: Vec<_> = list.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts[0], "乙");
        assert_eq!(texts[1], "甲");
        // Delete learning records: memory and files are synchronized, and other records are
        // retained.
        let mut learned = Learned::load(Some(path.clone()));
        assert!(learned.record("aaaa", "甲"));
        assert!(learned.forget("aaaa", "乙"));
        assert!(!learned.forget("aaaa", "乙"));
        let reloaded = Learned::load(Some(path.clone()));
        assert_eq!(reloaded.count("aaaa", "乙"), 0);
        assert_eq!(reloaded.count("aaaa", "甲"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
