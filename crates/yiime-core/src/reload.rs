//! Reloading user words in a running service. The shared input box stays resident (up to 30
//! minutes idle), so an import or an edit of user_words.txt — from the settings window of any
//! YiIME process, or from Notepad — must reach it without a restart: the service checks the
//! file's stamp when a session or a code starts and reloads only when it changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Size and modification time of a file (None when missing). Atomic writes replace the file, so
/// any save changes the stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StampKind {
    Missing,
    Present(SystemTime, u64),
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp(StampKind);

impl FileStamp {
    pub fn of(path: &Path) -> Self {
        Self(match std::fs::metadata(path) {
            Ok(meta) => StampKind::Present(
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StampKind::Missing,
            Err(_) => StampKind::Unreadable,
        })
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
        let read = path.as_deref().map(std::fs::read_to_string);
        let retry = matches!(&read, Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound)
            || stamp.is_some_and(|s| s.0 == StampKind::Unreadable);
        let contents = read.and_then(Result::ok);
        (
            Self {
                path,
                loaded: if retry { None } else { stamp },
            },
            contents,
        )
    }

    /// New contents when the file changed since the last load (empty when it was deleted), None
    /// when it did not change or cannot be read right now (the next check retries).
    /// Updates the watched path (e.g. when `InputTranslation` migrates to `YiIME`) and checks
    /// for changes, invalidating the cached stamp when the path changes.
    pub fn check_path(&mut self, path: Option<PathBuf>) -> Option<String> {
        if self.path != path {
            self.path = path;
            self.loaded = None;
        }
        self.check()
    }

    pub fn check(&mut self) -> Option<String> {
        let path = self.path.as_deref()?;
        let stamp = FileStamp::of(path);
        match stamp.0 {
            StampKind::Unreadable => None,
            StampKind::Missing => {
                if self.loaded == Some(stamp) {
                    return None;
                }
                self.loaded = Some(stamp);
                Some(String::new())
            }
            StampKind::Present(_, _) => {
                if self.loaded == Some(stamp) {
                    return None;
                }
                let contents = std::fs::read_to_string(path).ok()?;
                self.loaded = Some(stamp);
                Some(contents)
            }
        }
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
    fn check_path_follows_directory_migration_and_import() {
        let root = std::env::temp_dir().join(format!("yiime-relpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old_dir = root.join("InputTranslation");
        let new_dir = root.join("YiIME");
        std::fs::create_dir_all(&old_dir).unwrap();
        let old_file = old_dir.join("user_words.txt");
        std::fs::write(&old_file, "旧词 jiuci\n").unwrap();

        let (mut reloader, first) = UserWordsReloader::load(Some(old_file.clone()));
        assert_eq!(first.as_deref(), Some("旧词 jiuci\n"));

        // Simulate migrate_settings renaming InputTranslation -> YiIME and importing a word.
        std::fs::rename(&old_dir, &new_dir).unwrap();
        let new_file = new_dir.join("user_words.txt");
        std::fs::write(&new_file, "旧词 jiuci\n新词 xinci\n").unwrap();

        // Still pointing at old path does not see new_file, whereas check_path picks it up.
        assert_eq!(
            reloader.check_path(Some(new_file.clone())).as_deref(),
            Some("旧词 jiuci\n新词 xinci\n")
        );
        assert_eq!(reloader.check_path(Some(new_file)), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_keeps_words_and_reloads_when_permission_returns() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("yiime-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("user_words.txt");
        std::fs::write(
            &path,
            "你好 nihao
",
        )
        .unwrap();
        let (mut reloader, first) = UserWordsReloader::load(Some(path.clone()));
        assert_eq!(
            first.as_deref(),
            Some(
                "你好 nihao
"
            )
        );

        // Make parent dir non-searchable so metadata() returns PermissionDenied (unless root).
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::metadata(&path).is_err() {
            assert_eq!(
                reloader.check(),
                None,
                "must not return empty string and drop custom words"
            );
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                reloader.check(),
                None,
                "unchanged after permission restored"
            );
            std::fs::write(
                &path,
                "你好 nihao
新词 xinci
",
            )
            .unwrap();
            assert_eq!(
                reloader.check().as_deref(),
                Some(
                    "你好 nihao
新词 xinci
"
                )
            );
            std::fs::remove_file(&path).unwrap();
            assert_eq!(
                reloader.check().as_deref(),
                Some(""),
                "explicit NotFound clears"
            );
        } else {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
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
