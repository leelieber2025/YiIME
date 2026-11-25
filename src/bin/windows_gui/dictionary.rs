use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
use yiime_core::MixedInput;

/// The large lexicon next to the EXE (installed) or in data\frost (portable).
pub(super) fn frequency_words_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    // YiIME-words.txt since 0.2; the old name is still accepted.
    ["YiIME-words.txt", "InputTranslation-words.txt"]
        .into_iter()
        .flat_map(|name| [dir.join(name), dir.join("data").join("frost").join(name)])
        .find(|path| path.is_file())
}

/// Builds the bundled dictionaries plus the large lexicon off the UI thread, publishes the
/// complete snapshot in `ready` (queries never see a partial index), then posts `message` to
/// `hwnd` so the window can switch to it. `warm` also builds the initials index first.
pub(super) fn load_full_in_background(
    path: PathBuf,
    user_words: Option<String>,
    warm: bool,
    ready: Arc<OnceLock<Arc<MixedInput>>>,
    hwnd: usize,
    message: u32,
    error_message: u32,
) {
    std::thread::spawn(move || {
        let words = match std::fs::read_to_string(&path) {
            Ok(words) => words,
            Err(err) => {
                let permanent = usize::from(err.kind() == std::io::ErrorKind::NotFound);
                unsafe { PostMessageW(hwnd as _, error_message, permanent, 0) };
                return;
            }
        };
        let mut full = MixedInput::bundled().with_frequency_words(words);
        if let Some(user_words) = &user_words {
            full = full.with_user_words(user_words);
        }
        if warm {
            full.warm_up();
        }
        if ready.set(Arc::new(full)).is_ok() {
            unsafe { PostMessageW(hwnd as _, message, 0, 0) };
        }
    });
}
