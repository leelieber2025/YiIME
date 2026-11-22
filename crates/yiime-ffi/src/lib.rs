//! C interface to the YiIME core, used by the macOS input method (Swift/InputMethodKit).
//!
//! An [`Ime`] is one input session: code composition, the Chinese draft, and Shift-tap inline
//! English. Strings are UTF-8 C strings; returned strings must be freed with [`yiime_string_free`].

use serde_json::json;
use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;
use yiime_core::{
    ApiSettings, ComposeOptions, Composer, Draft, Engine, Flushed, Learned, LetterAction,
    LetterRules, MixedInput, Pick, RequestGate, classify_letter, is_literal_in_english,
    literal_code, paging_direction,
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
    /// Pending translation; kept when the session text is taken (see [`yiime_begin_request`]).
    requests: RequestGate,
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
    /// Commits the whole code segment by segment (see [`Composer::flush_all`]), so nothing is
    /// lost when the session ends and punctuation typed next lands after all of it.
    fn flush_all(&mut self) {
        let steps = self.composer.flush_all(input(), &OPTIONS, &self.learned);
        for step in steps {
            match step {
                Flushed::Pick(pick) => self.draft.insert(&pick.text),
                Flushed::Raw(raw) => self.draft.insert(&raw.replace('\'', "")),
            }
        }
    }
    /// Space: commits the first candidate if there is one (a partial one leaves the rest of the
    /// code composing), otherwise the raw letters.
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
                self.flush_all();
                self.draft.insert(&ch.to_string());
                true
            }
            _ if self.active() && !ch.is_control() => {
                self.flush_all();
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
    /// Ends the session and returns the Chinese text to commit (a pending code is committed whole,
    /// segment by segment).
    pub fn take_text(&mut self) -> String {
        self.flush_all();
        let text = self.draft.text().to_owned();
        let requests = std::mem::take(&mut self.requests);
        *self = Ime::default();
        self.requests = requests;
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

#[cfg(test)]
thread_local! {
    /// Tests set this to make the next guarded call panic inside the guard.
    static INJECT_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs an FFI body without letting a panic unwind into the caller (Swift/C): a panic returns
/// `fallback()` instead.
fn guard<T>(fallback: impl FnOnce() -> T, body: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if INJECT_PANIC.with(|flag| flag.replace(false)) {
            panic!("injected panic");
        }
        body()
    }))
    .unwrap_or_else(|_| fallback())
}

/// Like [`guard`] for calls that change the session: after a panic the session may be half
/// updated, so it is reset to an empty one.
///
/// # Safety
/// `ime` is null or comes from [`yiime_new`].
unsafe fn guard_ime<T>(ime: *mut Ime, fallback: T, body: impl FnOnce(&mut Ime) -> T) -> T {
    let Some(session) = (unsafe { ime.as_mut() }) else {
        return fallback;
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if INJECT_PANIC.with(|flag| flag.replace(false)) {
            panic!("injected panic");
        }
        body(session)
    })) {
        Ok(value) => value,
        Err(_) => {
            // Replacing the value may itself drop something broken; never unwind from here.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                *ime = Ime::default();
            }));
            fallback
        }
    }
}

/// A new session, or null if it could not be created.
#[unsafe(no_mangle)]
pub extern "C" fn yiime_new() -> *mut Ime {
    guard(std::ptr::null_mut, || Box::into_raw(Box::default()))
}

/// Preloads dictionaries (safe from a background thread).
#[unsafe(no_mangle)]
pub extern "C" fn yiime_preload() {
    guard(
        || (),
        || {
            let _ = input();
        },
    )
}

/// # Safety
/// `ime` comes from [`yiime_new`] and is no longer used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_free(ime: *mut Ime) {
    if !ime.is_null() {
        guard(|| (), || drop(unsafe { Box::from_raw(ime) }));
    }
}

/// Whether the key was handled; `false` (pass the key on) also after an internal error.
///
/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_key(ime: *mut Ime, ch: u32) -> bool {
    let Some(ch) = char::from_u32(ch) else {
        return false;
    };
    unsafe { guard_ime(ime, false, |ime| ime.key(ch)) }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_shift_tap(ime: *mut Ime) {
    unsafe { guard_ime(ime, (), Ime::shift_tap) }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_commit_code(ime: *mut Ime) {
    unsafe { guard_ime(ime, (), Ime::commit_code) }
}

/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_active(ime: *const Ime) -> bool {
    guard(
        || false,
        || unsafe { ime.as_ref() }.is_some_and(Ime::active),
    )
}

/// The session as JSON, or null (also after an internal error).
///
/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_state(ime: *const Ime) -> *mut c_char {
    guard(std::ptr::null_mut, || match unsafe { ime.as_ref() } {
        Some(ime) => to_c(ime.state_json()),
        None => std::ptr::null_mut(),
    })
}

/// The finished text, or null (also after an internal error, which resets the session).
///
/// # Safety
/// `ime` comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_take_text(ime: *mut Ime) -> *mut c_char {
    unsafe { guard_ime(ime, std::ptr::null_mut(), |ime| to_c(ime.take_text())) }
}

/// Blocking translation (call from a background thread). Returns `{"ok":true,"text":…}` or
/// `{"ok":false,"error":…}`; null only if even the error could not be built.
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
    guard(
        || {
            std::panic::catch_unwind(|| {
                to_c(json!({"ok": false, "error": "翻译处理出错，请重试"}).to_string())
            })
            .unwrap_or(std::ptr::null_mut())
        },
        || {
            let (source, url, model, key) =
                unsafe { (from_c(source), from_c(url), from_c(model), from_c(key)) };
            let value = match translate_text(&source, &url, &model, &key) {
                Ok(text) => json!({"ok": true, "text": text}),
                Err(error) => json!({"ok": false, "error": error}),
            };
            to_c(value.to_string())
        },
    )
}

/// Starts an asynchronous request (a translation) for this session and returns its id, or 0 on
/// failure. Its result may be used only if [`yiime_finish_request`] later returns true.
///
/// # Safety
/// `ime` is null or comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_begin_request(ime: *mut Ime) -> u64 {
    unsafe { guard_ime(ime, 0, |ime| ime.requests.begin()) }
}

/// The session ended or moved to another client (deactivation, app switch): results of pending
/// requests must be dropped.
///
/// # Safety
/// `ime` is null or comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_cancel_request(ime: *mut Ime) {
    unsafe { guard_ime(ime, (), |ime| ime.requests.cancel()) }
}

/// Request `id` completed: true if its result may be committed now (still the current request,
/// not cancelled), at most once; false means drop it.
///
/// # Safety
/// `ime` is null or comes from [`yiime_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_finish_request(ime: *mut Ime, id: u64) -> bool {
    unsafe { guard_ime(ime, false, |ime| ime.requests.finish(id)) }
}

/// # Safety
/// `text` is the string returned from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yiime_string_free(text: *mut c_char) {
    if !text.is_null() {
        guard(|| (), || drop(unsafe { CString::from_raw(text) }));
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

    /// What committing `code` whole gives, segment by segment (the reference for the tests below).
    fn segments(code: &str) -> Vec<String> {
        let mut composer = Composer::default();
        let learned = Learned::default();
        composer.set_code(input(), code.to_owned(), &OPTIONS, &learned);
        composer
            .flush_all(input(), &OPTIONS, &learned)
            .into_iter()
            .map(|step| match step {
                Flushed::Pick(pick) => pick.text,
                Flushed::Raw(raw) => raw,
            })
            .collect()
    }

    #[test]
    fn ending_the_session_commits_the_leftover_code() {
        let leftover = segments("x").concat();
        assert!(!leftover.is_empty());
        assert_eq!(segments("zhongguox"), ["中国".to_owned(), leftover.clone()]);
        let mut ime = Ime::default();
        typed(&mut ime, "zhongguox");
        assert_eq!(ime.code(), "zhongguox");
        // Before: only 中国 was committed and the `x` was dropped.
        assert_eq!(ime.take_text(), format!("中国{leftover}"));
        assert!(!ime.active());
    }

    #[test]
    fn multi_segment_codes_commit_every_segment_in_order() {
        let code = "zhongguoxian";
        let steps = segments(code);
        let expected = steps.concat();
        assert!(!expected.is_empty());
        let mut ime = Ime::default();
        typed(&mut ime, "q ");
        typed(&mut ime, code);
        assert_eq!(ime.take_text(), format!("我{expected}"));
        // Letters with no candidate at all are committed as typed.
        let mut composer = Composer::default();
        let learned = Learned::default();
        composer.set_code(input(), "vvvv".to_owned(), &OPTIONS, &learned);
        let steps = composer.flush_all(input(), &OPTIONS, &learned);
        assert!(composer.code.is_empty());
        assert!(!steps.is_empty());
    }

    #[test]
    fn punctuation_lands_after_the_whole_code() {
        let leftover = segments("x").concat();
        let mut ime = Ime::default();
        typed(&mut ime, "zhongguox,");
        // Before: 中国，then the `x` candidate after the comma (or lost).
        assert_eq!(ime.draft(), format!("中国{leftover}，"));
        assert!(ime.code().is_empty());
        typed(&mut ime, "zhongguox.");
        assert_eq!(ime.take_text(), format!("中国{leftover}，中国{leftover}。"));
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

    fn inject_panic() {
        INJECT_PANIC.with(|flag| flag.set(true));
    }

    #[test]
    fn panics_return_error_values_instead_of_unwinding() {
        unsafe {
            inject_panic();
            assert!(yiime_new().is_null());
            let ime = yiime_new();
            assert!(!ime.is_null());
            for ch in "nihao".chars() {
                assert!(yiime_key(ime, ch as u32));
            }
            assert!(yiime_active(ime));
            inject_panic();
            assert!(!yiime_key(ime, ' ' as u32), "panic → key passed on");
            // The half-updated session was reset.
            assert!(!yiime_active(ime));
            inject_panic();
            assert!(!yiime_active(ime));
            inject_panic();
            assert!(yiime_state(ime).is_null());
            inject_panic();
            assert!(yiime_take_text(ime).is_null());
            inject_panic();
            yiime_shift_tap(ime);
            inject_panic();
            yiime_commit_code(ime);
            inject_panic();
            yiime_preload();
            inject_panic();
            let error = yiime_translate(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            );
            let value: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(error).to_str().unwrap()).unwrap();
            assert_eq!(value["ok"], false);
            yiime_string_free(error);
            // Still usable afterwards.
            for ch in "q ".chars() {
                yiime_key(ime, ch as u32);
            }
            let text = yiime_take_text(ime);
            assert_eq!(CStr::from_ptr(text).to_str().unwrap(), "我");
            yiime_string_free(text);
            inject_panic();
            assert_eq!(yiime_begin_request(ime), 0);
            inject_panic();
            yiime_cancel_request(ime);
            inject_panic();
            assert!(!yiime_finish_request(ime, 1));
            // Null sessions are rejected without touching anything.
            assert!(!yiime_key(std::ptr::null_mut(), 'a' as u32));
            assert_eq!(yiime_begin_request(std::ptr::null_mut()), 0);
            assert!(yiime_state(std::ptr::null()).is_null());
            yiime_free(ime);
        }
    }

    #[test]
    fn translation_results_commit_only_into_the_session_that_asked() {
        unsafe {
            let ime = yiime_new();
            for ch in "nihao".chars() {
                yiime_key(ime, ch as u32);
            }
            // Enter: the text is taken, then the translation starts.
            yiime_string_free(yiime_take_text(ime));
            let first = yiime_begin_request(ime);
            assert_ne!(first, 0);
            // The user switches applications before the result arrives.
            yiime_cancel_request(ime);
            assert!(!yiime_finish_request(ime, first), "stale result is dropped");
            // A new translation in the next session; taking its text keeps the request.
            for ch in "q".chars() {
                yiime_key(ime, ch as u32);
            }
            let second = yiime_begin_request(ime);
            yiime_string_free(yiime_take_text(ime));
            assert!(!yiime_finish_request(ime, first));
            assert!(yiime_finish_request(ime, second));
            assert!(!yiime_finish_request(ime, second), "committed once");
            yiime_free(ime);
        }
    }
}
