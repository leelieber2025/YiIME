//! Input history: translated Chinese text and its translation, newest first, one entry per Chinese
//! sentence. The file is encrypted with [`Sealer`] (DPAPI on Windows, local only).

use crate::settings::Sealer;
use crate::text::single_line;
use std::path::PathBuf;

pub const HISTORY_CAP: usize = 500;
pub const HISTORY_FILE: &str = "history.dat";

/// One history entry: local time, Chinese text, translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub time: String,
    pub chinese: String,
    pub english: String,
}

/// History file: one `time<TAB>Chinese<TAB>translation` per line, with TAB/CR/LF escaped inside
/// fields.
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
    /// Newest first; empty if the file is missing or cannot be decrypted.
    pub fn load(&self) -> Vec<HistoryEntry> {
        self.try_load().unwrap_or_default()
    }
    /// Newest first; None if the existing file could not be read, decrypted or parsed.
    pub fn try_load(&self) -> Option<Vec<HistoryEntry>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
            Err(_) => return None,
        };
        let plain = self.sealer.open(&bytes)?;
        std::str::from_utf8(&plain)
            .ok()?
            .lines()
            .map(|line| {
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
        crate::write_atomically(&self.path, &sealed)
    }
    pub fn promote(&self, entry: &HistoryEntry) -> bool {
        let Ok(_lock) = crate::storage::lock(&self.path) else {
            return false;
        };
        let Some(mut entries) = self.try_load() else {
            return false;
        };
        let Some(index) = entries.iter().position(|item| item == entry) else {
            return false;
        };
        let entry = entries.remove(index);
        entries.insert(0, entry);
        self.save(&entries)
    }
    pub fn clear(&self) -> bool {
        let Ok(_lock) = crate::storage::lock(&self.path) else {
            return false;
        };
        !self.path.exists() || std::fs::remove_file(&self.path).is_ok()
    }
    /// Records a committed translation (blank text is skipped). A repeated sentence keeps only its
    /// newest entry; the oldest entries are dropped past the cap.
    pub fn remember(&self, chinese: &str, english: &str, time: String) -> bool {
        if chinese.trim().is_empty() {
            return false;
        }
        let Ok(_lock) = crate::storage::lock(&self.path) else {
            return false;
        };
        let Some(mut entries) = self.try_load() else {
            return false;
        };
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

/// One line in the history list: time, shortened source and translation.
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
        let stale = entries[0].clone();
        assert!(history.clear());
        assert!(!history.promote(&stale));
        std::fs::write(&history.path, [0xff]).unwrap();
        assert_eq!(history.try_load(), None);
        assert!(!history.remember("新记录", "new", "now".into()));
        assert!(!history.promote(&stale));
        assert_eq!(std::fs::read(&history.path).unwrap(), [0xff]);
        let malformed = b"t1\t\xe4\xbd\xa0\xe5\xa5\xbd\tHello\nt2\tbroken\n";
        std::fs::write(&history.path, malformed).unwrap();
        assert!(!history.remember("新记录", "new", "now".into()));
        assert_eq!(std::fs::read(&history.path).unwrap(), malformed);
        assert!(history.clear());
        assert_eq!(history.try_load(), Some(Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
