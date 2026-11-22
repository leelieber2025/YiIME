//! C interface to the YiIME core, used by the macOS input method (Swift/InputMethodKit).
//!
//! An [`Ime`] is one input session: code composition, the Chinese draft, and Shift-tap inline
//! English. Strings are UTF-8 C strings; returned strings must be freed with [`yiime_string_free`].

use serde_json::json;
use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;
use yiime_core::{
    ApiSettings, ComposeOptions, Composer, Draft, Engine, Learned, LetterAction, LetterRules,
    MixedInput, Pick, classify_letter, is_literal_in_english, literal_code, paging_direction,
};

fn input() -> &'static MixedInput {
    static INPUT: OnceLock<MixedInput> = OnceLock::new();
    INPUT.get_or_init(MixedInput::bundled)
}

const OPTIONS: ComposeOptions = ComposeOptions {
    page_size: 9,
    traditional: false,
    learn: false,
    fuzzy: false,
    pinyin_first: false,
    common_only: true,
    shuangpin: None,
};
const RULES: LetterRules = LetterRules {
    push_fifth: true,
    auto_commit4: true,
};

#[derive(Default)]
pub struct Ime {
    composer: Composer,
    draft: Draft,
    english: bool,
    learned: Learned,
}

impl Ime {
    pub fn code(&self) -> &str {
        &self.composer.code
    }
    pub fn draft(&self) -> &str {
        self.draft.text()
    }
    pub fn english(&self) -> bool {
        self.english
    }
    /// Whether a code or draft is in progress, i.e. keys belong to the input method.
    pub fn active(&self) -> bool {
        self.composer.is_composing() || !self.draft.text().is_empty()
    }
    fn set_code(&mut self, code: String) {
        self.composer
            .set_code(input(), code, &OPTIONS, &self.learned);
    }
    fn take_pick(&mut self, pick: Pick) {
        self.draft.insert(&pick.text);
        self.set_code(pick.rest);
    }
    /// Commits the first candidate if there is one, otherwise the raw letters.
    fn flush_first(&mut self) {
        if !self.composer.is_composing() {
            return;
        }
        match self.composer.candidate_at(0) {
            Some(pick) => self.take_pick(pick),
            None => {
                let raw = self.composer.code.replace('\'', "");
                self.draft.insert(&raw);
                self.set_code(String::new());
            }
        }
    }
    /// Handles one character (letter, space, digit, punctuation, backspace `\u{8}`, Esc `\u{1b}`).
    /// Returns false when there is no session, so the host types it as is.
    pub fn key(&mut self, ch: char) -> bool {
        if ch == '\u{1b}' {
            if self.composer.is_composing() {
                self.set_code(String::new());
                return true;
            }
            if !self.draft.text().is_empty() {
                self.draft.replace_text("");
                self.english = false;
                return true;
            }
            return false;
        }
        if ch == '\u{8}' {
            if self.composer.is_composing() {
                let mut code = self.composer.code.clone();
                code.pop();
                self.set_code(code);
                return true;
            }
            if !self.draft.text().is_empty() {
                self.draft.backspace();
                return true;
            }
            return false;
        }
        if self.english && self.composer.code.is_empty() && is_literal_in_english(ch) {
            self.draft.insert(&ch.to_string());
            return true;
        }
        if self.composer.is_composing() {
            match ch {
                ' ' => {
                    self.flush_first();
                    return true;
                }
                '1'..='9' => {
                    let index = ch as usize - '1' as usize;
                    if let Some(pick) = self.composer.candidate_at(index) {
                        self.take_pick(pick);
                    }
                    return true;
                }
                _ => {}
            }
            if let Some(forward) = paging_direction(ch, false) {
                self.composer.turn_page(forward);
                return true;
            }
        }
        match classify_letter(ch, false, self.composer.is_composing()) {
            Some(LetterAction::Compose(letter)) => {
                let picks =
                    self.composer
                        .type_letter(input(), letter, RULES, &OPTIONS, &self.learned);
                for pick in picks {
                    self.take_pick(pick);
                }
                true
            }
            Some(LetterAction::Raw { .. }) if self.active() => {
                self.flush_first();
                self.draft.insert(&ch.to_string());
                true
            }
            _ if self.active() && !ch.is_control() => {
                self.flush_first();
                let text = yiime_core::full_width(ch, false)
                    .map(str::to_owned)
                    .unwrap_or_else(|| ch.to_string());
                self.draft.insert(&text);
                true
            }
            _ => false,
        }
    }
    /// Shift tap: toggles the inline English segment; entering English puts the pending code into
    /// the draft as letters.
    pub fn shift_tap(&mut self) {
        if !self.english {
            if let Some(letters) = literal_code(&self.composer.code) {
                self.draft.insert(&letters);
                self.set_code(String::new());
            }
        }
        self.english = !self.english;
    }
    /// Enter while composing: the code goes into the draft as letters (as in Microsoft Pinyin).
    pub fn commit_code(&mut self) {
        if let Some(letters) = literal_code(&self.composer.code) {
            self.draft.insert(&letters);
        }
        self.set_code(String::new());
    }
    /// Ends the session and returns the Chinese text to commit (a pending code commits its first
    /// candidate).
    pub fn take_text(&mut self) -> String {
        self.flush_first();
        let text = self.draft.text().to_owned();
        *self = Ime::default();
        text
    }
    /// Current state as JSON: code, draft, english, candidates (text, hint), page.
    pub fn state_json(&self) -> String {
        let candidates: Vec<_> = self
            .composer
            .page_items()
            .iter()
            .map(|c| json!({"text": c.text, "hint": self.composer.hint(c)}))
            .collect();
        json!({
            "code": self.composer.code,
            "draft": self.draft.text(),
            "english": self.english,
            "candidates": candidates,
            "page": self.composer.page_label(),
        })
        .to_string()
    }
}

/// Free Google by default; with `url`, `model` and `key` set, the LLM is the fallback when Google
/// fails (the caller passes them for exactly that, so fallback is allowed only then).
pub fn translate_text(source: &str, url: &str, model: &str, key: &str) -> Result<String, String> {
    let mut settings = ApiSettings {
        engine: Engine::GoogleFree,
        ..ApiSettings::default()
    };
    if !url.is_empty() && !model.is_empty() && !key.is_empty() {
        settings.allow_fallback = true;
        settings.url = url.to_owned();
        settings.model = model.to_owned();
        settings.key = key.to_owned();
    }
    yiime_core::translate(&settings, source).result
}

fn to_c(text: String) -> *mut c_char {
    CString::new(text.replace('\0', ""))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `text` is a null pointer or a valid C string.
unsafe fn from_c(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn yiime_new() -> *mut Ime {
    Box::into_raw(Box::default())
}

/// Preloads dictionaries (safe from a background thread).
#[unsafe(no_mangle)]
pub extern "C" fn yiime_preload() {
    let _ = input();
}

/// # Safety
/// `ime` comes from [`yiime_new`] and is no longer used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_free(ime: *mut Ime) {
    if !ime.is_null() {
        drop(unsafe { Box::from_raw(ime) });
    }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_key(ime: *mut Ime, ch: u32) -> bool {
    let (Some(ime), Some(ch)) = (unsafe { ime.as_mut() }, char::from_u32(ch)) else {
        return false;
    };
    ime.key(ch)
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_shift_tap(ime: *mut Ime) {
    if let Some(ime) = unsafe { ime.as_mut() } {
        ime.shift_tap();
    }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_commit_code(ime: *mut Ime) {
    if let Some(ime) = unsafe { ime.as_mut() } {
        ime.commit_code();
    }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_active(ime: *const Ime) -> bool {
    unsafe { ime.as_ref() }.is_some_and(Ime::active)
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_state(ime: *const Ime) -> *mut c_char {
    match unsafe { ime.as_ref() } {
        Some(ime) => to_c(ime.state_json()),
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_take_text(ime: *mut Ime) -> *mut c_char {
    match unsafe { ime.as_mut() } {
        Some(ime) => to_c(ime.take_text()),
        None => std::ptr::null_mut(),
    }
}

/// Blocking translation (call from a background thread). Returns `{"ok":true,"text":…}` or
/// `{"ok":false,"error":…}`.
///
/// # Safety
/// The argument is a null pointer or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_translate(
    source: *const c_char,
    url: *const c_char,
    model: *const c_char,
    key: *const c_char,
) -> *mut c_char {
    let (source, url, model, key) =
        unsafe { (from_c(source), from_c(url), from_c(model), from_c(key)) };
    let value = match translate_text(&source, &url, &model, &key) {
        Ok(text) => json!({"ok": true, "text": text}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    to_c(value.to_string())
}

/// # Safety
/// `text` is the string returned from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(ime: &mut Ime, text: &str) {
        for ch in text.chars() {
            ime.key(ch);
        }
    }

    #[test]
    fn wubi_pinyin_and_inline_english() {
        let mut ime = Ime::default();
        assert!(!ime.key(' '), "没有会话时空格交给宿主");
        typed(&mut ime, "q ");
        assert_eq!(ime.draft(), "我");
        typed(&mut ime, "zhongguo ");
        assert_eq!(ime.draft(), "我中国");
        ime.shift_tap();
        typed(&mut ime, "OK 1");
        ime.shift_tap();
        typed(&mut ime, "q");
        assert_eq!(ime.take_text(), "我中国OK 1我");
        assert!(!ime.active());
    }

    #[test]
    fn pending_code_becomes_letters_and_backspace_works() {
        let mut ime = Ime::default();
        typed(&mut ime, "zhong");
        ime.shift_tap();
        assert_eq!(ime.draft(), "zhong");
        assert!(ime.english());
        ime.key('\u{8}');
        assert_eq!(ime.draft(), "zhon");
        ime.key('\u{1b}');
        assert!(!ime.active());
        typed(&mut ime, "nihao");
        ime.commit_code();
        assert_eq!(ime.take_text(), "nihao");
    }

    #[test]
    fn ffi_round_trip() {
        unsafe {
            let ime = yiime_new();
            for ch in "q ".chars() {
                yiime_key(ime, ch as u32);
            }
            let state = yiime_state(ime);
            let value: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(state).to_str().unwrap()).unwrap();
            assert_eq!(value["draft"], "我");
            yiime_string_free(state);
            let text = yiime_take_text(ime);
            assert_eq!(CStr::from_ptr(text).to_str().unwrap(), "我");
            yiime_string_free(text);
            yiime_free(ime);
        }
    }
}
