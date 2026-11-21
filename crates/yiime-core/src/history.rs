//! Input memory: Translate the original Chinese text and translation on the screen, with the latest
//! first, and only the latest Chinese sentence for the same sentence is retained. The file contents
//! are [`Sealer`] encrypted (DPAPI on Windows, local only).

use crate::settings::Sealer;
use crate::text::single_line;
use std::path::PathBuf;

/// The maximum number of entries to keep.
pub const HISTORY_CAP: usize = 500;
pub const HISTORY_FILE: &str = "history.dat";

/// One input memory: local time, Chinese original text, and translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub time: String,
    pub chinese: String,
    pub english: String,
}

/// Input memory in the encrypted file (each line "time <TAB> Chinese <TAB> translation", \TAB CR LF
/// escape within the field).
pub struct History<'a> {
    path: PathBuf,
    sealer: &'a dyn Sealer,
}

fn escape_field(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn unescape_field(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

impl<'a> History<'a> {
    pub fn new(path: PathBuf, sealer: &'a dyn Sealer) -> Self {
        Self { path, sealer }
    }
    /// Read (newest first); empty if the file does not exist or decryption failed.
    pub fn load(&self) -> Vec<HistoryEntry> {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return Vec::new();
        };
        let Some(plain) = self.sealer.open(&bytes) else {
            return Vec::new();
        };
        String::from_utf8_lossy(&plain)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                Some(HistoryEntry {
                    time: unescape_field(fields.next()?),
                    chinese: unescape_field(fields.next()?),
                    english: unescape_field(fields.next()?),
                })
            })
            .collect()
    }
    pub fn save(&self, entries: &[HistoryEntry]) -> bool {
        let text: String = entries
            .iter()
            .take(HISTORY_CAP)
            .map(|entry| {
                format!(
                    "{}\t{}\t{}\n",
                    escape_field(&entry.time),
                    escape_field(&entry.chinese),
                    escape_field(&entry.english)
                )
            })
            .collect();
        let Some(sealed) = self.sealer.seal(text.as_bytes()) else {
            return false;
        };
        let _ = std::fs::create_dir_all(self.path.parent().unwrap_or(&self.path));
        std::fs::write(&self.path, sealed).is_ok()
    }
    pub fn clear(&self) -> bool {
        !self.path.exists() || std::fs::remove_file(&self.path).is_ok()
    }
    /// Make a note of the translation that appears on the screen (blank original text is not
    /// recorded); only the latest Chinese version of the same sentence will be retained, and the
    /// oldest one will be discarded if it exceeds the upper limit.
    pub fn remember(&self, chinese: &str, english: &str, time: String) -> bool {
        if chinese.trim().is_empty() {
            return false;
        }
        let mut entries = self.load();
        entries.retain(|entry| entry.chinese != chinese);
        entries.insert(
            0,
            HistoryEntry {
                time,
                chinese: chinese.to_owned(),
                english: english.to_owned(),
            },
        );
        entries.truncate(HISTORY_CAP);
        self.save(&entries)
    }
}

/// Enter a line in the memory window list: time, truncated source text, and translation.
pub fn history_label(entry: &HistoryEntry) -> String {
    let clip = |text: &str, n: usize| {
        let flat = single_line(text);
        if flat.chars().count() > n {
            flat.chars().take(n).collect::<String>() + "…"
        } else {
            flat
        }
    };
    format!(
        "{}   {}   →   {}",
        entry.time,
        clip(&entry.chinese, 24),
        clip(&entry.english, 40)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PlainSealer;

    #[test]
    fn remember_dedupes_caps_and_escapes() {
        let dir = std::env::temp_dir().join(format!("yiime-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let history = History::new(dir.join(HISTORY_FILE), &PlainSealer);
        assert!(history.load().is_empty());
        assert!(!history.remember("  ", "x", "t".into()));
        assert!(history.remember("第一\t行\n二", "a\\b", "t1".into()));
        assert!(history.remember("你好", "Hello", "t2".into()));
        assert!(history.remember("第一\t行\n二", "again", "t3".into()));
        let entries = history.load();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].chinese, "第一\t行\n二");
        assert_eq!(entries[0].english, "again");
        assert_eq!(entries[1].english, "Hello");
        for i in 0..HISTORY_CAP + 5 {
            history.remember(&format!("句{i}"), "e", "t".into());
        }
        let entries = history.load();
        assert_eq!(entries.len(), HISTORY_CAP);
        assert_eq!(entries[0].chinese, format!("句{}", HISTORY_CAP + 4));
        assert_eq!(
            history_label(&HistoryEntry {
                time: "2026-11-21 15:00".into(),
                chinese: "一\n二".into(),
                english: "x".repeat(41),
            }),
            format!("2026-11-21 15:00   一 二   →   {}…", "x".repeat(40))
        );
        assert!(history.clear());
        assert!(history.load().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
