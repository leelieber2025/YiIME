//! Reloading user words in a running service. The shared input box stays resident (up to 30
//! minutes idle), so an import or an edit of user_words.txt — from the settings window of any
//! YiIME process, or from Notepad — must reach it without a restart: the service checks the
//! file's stamp when a session or a code starts and reloads only when it changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Size and modification time of a file (None when missing). Atomic writes replace the file, so
/// any save changes the stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp(Option<(SystemTime, u64)>);

impl FileStamp {
    pub fn of(path: &Path) -> Self {
        Self(std::fs::metadata(path).ok().map(|meta| {
            (
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
            )
        }))
    }
}

/// Stamps of the API settings: settings.json and the encrypted key files. The settings window
/// runs in the tray process while the resident input box (another process) translates, so the
/// box compares this stamp before each translation and reloads the settings when it changed;
/// otherwise a new engine (DeepL, LLM…) only took effect after the box restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsStamp([FileStamp; 5]);

impl SettingsStamp {
    pub fn of(dir: &Path) -> Self {
        use crate::settings::SETTINGS_FILE;
        use crate::settings::{AZURE_KEY_FILE, DEEPL_KEY_FILE, GOOGLE_KEY_FILE, KEY_FILE};
        Self(
            [
                SETTINGS_FILE,
                KEY_FILE,
                GOOGLE_KEY_FILE,
                AZURE_KEY_FILE,
                DEEPL_KEY_FILE,
            ]
            .map(|name| FileStamp::of(&dir.join(name))),
        )
    }
}

/// Watches the user words file of one process.
#[derive(Debug)]
pub struct UserWordsReloader {
    path: Option<PathBuf>,
    /// Stamp of the contents last handed out; None forces the next check to reload.
    loaded: Option<FileStamp>,
}

impl UserWordsReloader {
    /// Reads `path` for the first load. The stamp is taken before reading, so a write racing
    /// with the read is picked up by the next [`Self::check`].
    pub fn load(path: Option<PathBuf>) -> (Self, Option<String>) {
        let stamp = path.as_deref().map(FileStamp::of);
        let contents = path
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok());
        (
            Self {
                path,
                loaded: stamp,
            },
            contents,
        )
    }

    /// New contents when the file changed since the last load (empty when it was deleted), None
    /// when it did not change or cannot be read right now (the next check retries).
    pub fn check(&mut self) -> Option<String> {
        let path = self.path.as_deref()?;
        let stamp = FileStamp::of(path);
        if self.loaded == Some(stamp) {
            return None;
        }
        let contents = if stamp.0.is_none() {
            String::new()
        } else {
            std::fs::read_to_string(path).ok()?
        };
        self.loaded = Some(stamp);
        Some(contents)
    }

    /// Makes the next [`Self::check`] reload even if the file did not change (used when a new
    /// dictionary snapshot replaces the one that had the words).
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Tests may unwrap.
    use super::*;

    #[test]
    fn reloads_only_after_a_change() {
        let dir = std::env::temp_dir().join(format!("yiime-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("user_words.txt");
        let _ = std::fs::remove_file(&path);
        let (mut reloader, first) = UserWordsReloader::load(Some(path.clone()));
        assert_eq!(first, None);
        assert_eq!(reloader.check(), None);
        std::fs::write(&path, "你好 nihao\n").unwrap();
        assert_eq!(reloader.check().as_deref(), Some("你好 nihao\n"));
        assert_eq!(reloader.check(), None);
        // A different size always changes the stamp, even within the same mtime tick.
        std::fs::write(&path, "你好 nihao\n中国 zhongguo\n").unwrap();
        assert_eq!(
            reloader.check().as_deref(),
            Some("你好 nihao\n中国 zhongguo\n")
        );
        reloader.invalidate();
        assert!(reloader.check().is_some());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(reloader.check().as_deref(), Some(""));
        assert_eq!(reloader.check(), None);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(UserWordsReloader::load(None).0.check(), None);
    }

    #[test]
    fn settings_stamp_changes_when_another_process_saves() {
        use crate::settings::{ApiSettings, Engine, PlainSealer};
        let dir = std::env::temp_dir().join(format!("yiime-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let empty = SettingsStamp::of(&dir);
        let mut settings = ApiSettings::default();
        settings.save(&dir, &PlainSealer).unwrap();
        let saved = SettingsStamp::of(&dir);
        assert_ne!(empty, saved);
        assert_eq!(saved, SettingsStamp::of(&dir));
        // "azure" and "deepl" have the same length: a key file appearing still changes it.
        settings.engine = Engine::DeepL;
        settings.deepl_key = "k:fx".to_owned();
        settings.save(&dir, &PlainSealer).unwrap();
        let deepl = SettingsStamp::of(&dir);
        assert_ne!(saved, deepl);
        assert_eq!(ApiSettings::load(&dir, &PlainSealer).engine, Engine::DeepL);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
