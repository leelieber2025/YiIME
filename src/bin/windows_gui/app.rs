//! Windows candidate box, tray, settings and TSF companion process.
use super::dictionary;
mod candidate;
mod history;
mod ipc;
mod key;
use candidate::{create_font, paint, relayout};
use ipc::{notify_tsf_ready, send_tsf_text, watch_tsf_owner};
use key::{edit_proc, handle_char, insert_text, pick, set_code};
mod settings;
mod tray;
use history::{history_proc, open_history};
use settings::{open_settings, settings_proc};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};
use tray::{add_tray, tray_data, tray_menu, update_tray_tip};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, ClientToScreen, CreateFontW,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DT_CALCRECT, DT_EDITCONTROL, DT_NOPREFIX,
    DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetMonitorInfoW,
    GetStockObject, GetTextExtentPoint32W, GetTextMetricsW, HDC, HFONT, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromPoint, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT,
    TextOutW, WHITE_BRUSH,
};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::{
    EM_GETSEL, EM_LIMITTEXT, EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL,
};
use windows_sys::Win32::UI::Input::Ime::ImmAssociateContextEx;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    SendInput, SetFocus, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_HOME, VK_LEFT,
    VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use yiime_core::{
    ApiSettings, ComposeOptions, Composer, Engine, Flag, HISTORY_CAP, HISTORY_FILE, History,
    HistoryEntry, Hotkey, LEARNED_FILE, Learned, LetterAction, LetterRules, MAX_CODE_LEN,
    MixedInput, OptionStore, Options, Quotes, Sealer, TranslationStyle, USER_WORDS_FILE,
    classify_letter, history_label, parse_hotkey, parse_page_size, selection_index, single_line,
};

const DRAFT: i32 = 103;
const SETTINGS_URL: i32 = 201;
const SETTINGS_MODEL: i32 = 202;
const SETTINGS_KEY: i32 = 203;
const SETTINGS_STYLE: i32 = 204;
const SETTINGS_CUSTOM: i32 = 205;
const SETTINGS_PLAIN: i32 = 206;
const SETTINGS_HOTKEY: i32 = 207;
const SETTINGS_USER_WORDS: i32 = 208;
const SETTINGS_HISTORY: i32 = 220;
const SETTINGS_HISTORY_KEY: i32 = 221;
const SETTINGS_CLEAR_HISTORY: i32 = 222;
const HISTORY_LIST: i32 = 301;
const HISTORY_COPY_ZH: i32 = 302;
const HISTORY_COPY_EN: i32 = 303;
const HISTORY_REUSE: i32 = 304;
const HISTORY_CLOSE: i32 = 305;
const HISTORY_STATUS: i32 = 306;
const TRAY_HISTORY: usize = 4;
/// Settings-window switches: (control ID, label, option).
const OPTION_BOXES: [(i32, &str, Flag); 12] = [
    (210, "中文标点（Ctrl+. 切换）", Flag::PunctEnglish),
    (211, "繁体输出（Ctrl+Shift+F）", Flag::Traditional),
    (212, "四码唯一时自动上屏", Flag::AutoCommit4),
    (213, "五码顶字 / 空码清除", Flag::PushFifth),
    (214, "；' 选第二、三候选", Flag::SemicolonSelect),
    (215, "按使用频率调整候选（自学习）", Flag::Learn),
    (216, "模糊音 z/zh n/l an/ang 等", Flag::FuzzyPinyin),
    (217, "[ ] 以词定字", Flag::BracketCharPick),
    (218, "， 。 也翻页（组字时）", Flag::CommaPeriodPaging),
    (219, "引号括号自动配对", Flag::AutoPair),
    (229, "拼音优先（关闭五笔顶字）", Flag::PinyinFirst),
    (230, "只显示常用字（隐藏生僻字）", Flag::CommonOnly),
];
/// Each extra row of switches moves the controls below it down.
const OPTION_SHIFT: i32 = (OPTION_BOXES.len() as i32 + 1) / 2 * 26 - 3 * 26;
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const TRAY_SETTINGS: usize = 1;
const TRAY_EXIT: usize = 2;
const TRAY_PLAIN: usize = 3;
const TRAY_TRANSLATE: usize = 5;
/// Polls the mode (also changed by IME shortcuts and the taskbar button) to refresh the tray
/// tooltip, and retries switching to the large lexicon if it arrived mid-composition.
const TRAY_TIMER: usize = 7;
/// Registry key shared with the TSF DLL.
const STATE_KEY: &str = "Software\\InputTranslation";
/// A translation worker finished: drain `translation_rx`.
const WM_TRANSLATED: u32 = WM_APP + 1;
const WM_TSF_ACTIVATE: u32 = WM_APP + 2;
/// The application that owns the TSF receiver has exited: close this input box.
const WM_TSF_EXIT: u32 = WM_APP + 3;
const WM_TRAY: u32 = WM_APP + 4;
/// Asks the running tray instance to open settings (`--settings`, or a second launch).
const WM_OPEN_SETTINGS: u32 = WM_APP + 5;
/// Asks the running tray instance to open input history (`--history`).
const WM_OPEN_HISTORY: u32 = WM_APP + 6;
/// The dictionary with the large lexicon finished loading in the background.
const WM_FULL_INPUT: u32 = WM_APP + 7;
/// Tray (portable) and TSF editor instances use different titles so each can find its single
/// instance.
const TRAY_TITLE: &str = "YiIME 译 · 托盘";
const TSF_TITLE: &str = "YiIME 译 · TSF";
const APP_NAME: &str = "YiIME 译";
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
static TRAY_V4: AtomicBool = AtomicBool::new(false);
const EN_CHANGE_CODE: u16 = 0x0300;
const SETTINGS_PAGE_SIZE: i32 = 223;
const SETTINGS_ENGINE: i32 = 224;
const SETTINGS_GOOGLE_KEY: i32 = 225;
const SETTINGS_AZURE_KEY: i32 = 226;
const SETTINGS_AZURE_REGION: i32 = 227;
const SETTINGS_DEEPL_KEY: i32 = 228;
/// Height of the Translation Engine row at the top of the settings window; the other controls
/// move down by this much.
const ENGINE_SHIFT: i32 = 168;
/// Longest custom LLM instruction kept from the settings window, in characters.
const MAX_CUSTOM_LEN: usize = 500;
const PAD: i32 = 6;
const EDIT_H: i32 = 26;
const LINE_H: i32 = 24;
const GAP: i32 = 14;
const MIN_W: i32 = 340;
/// Widest candidate bar (fits 9 candidates with Wubi hints).
const MAX_W: i32 = 1000;
/// Drafts wrap beyond this width and then grow only in height.
const DRAFT_MAX_W: i32 = 480;
/// Draft lines shown before it scrolls inside the box.
const DRAFT_MAX_LINES: i32 = 8;
const HINT: &str = "Enter 翻译上屏 · Shift+Enter 中文上屏 · Shift 夹英文 · Esc 取消";
const ENGLISH_HINT: &str = "[英] 字母原样输入 · 单击 Shift 回中文 · Enter 翻译上屏";
const PLAIN_HINT: &str = "普通五笔模式 · 空格/数字 选字上屏 · Enter 上屏字母 · Esc 取消";
// COLORREF is 0x00BBGGRR.
const BLUE: u32 = 0x00C0_6000;
const GRAY: u32 = 0x0080_8080;
const RED: u32 = 0x0000_00C0;
const INK: u32 = 0x0020_2020;
const HIGHLIGHT: u32 = 0x00FB_E9D6;
const SEPARATOR: u32 = 0x00E0_E0E0;

/// Input-box state: composition lives in the core (`yiime_core::Composer`); this adds the
/// candidate click areas.
#[derive(Default)]
struct Composition {
    engine: Composer,
    /// Horizontal extent (client coordinates) of each candidate on this page, for mouse clicks.
    hits: Vec<(i32, i32)>,
    row_top: i32,
}
impl std::ops::Deref for Composition {
    type Target = Composer;
    fn deref(&self) -> &Composer {
        &self.engine
    }
}
impl std::ops::DerefMut for Composition {
    fn deref_mut(&mut self) -> &mut Composer {
        &mut self.engine
    }
}

struct State {
    input: RefCell<Arc<MixedInput>>,
    full_input: Arc<OnceLock<Arc<MixedInput>>>,
    edit: HWND,
    font: HFONT,
    small_font: HFONT,
    /// Current draft height (grows with multi-line content).
    draft_h: Cell<i32>,
    composition: RefCell<Composition>,
    /// Status line: (text, is error).
    status: RefCell<Option<(String, bool)>>,
    settings: RefCell<ApiSettings>,
    settings_window: Cell<HWND>,
    /// Screen rectangle of the target's text cursor; the box is placed against it.
    anchor: Cell<RECT>,
    /// Cursor rectangle sent by the TSF DLL before activation.
    pending_anchor: Cell<Option<RECT>>,
    target: Cell<HWND>,
    /// Bumped on every activation, cancel and commit; stale translation results are dropped.
    generation: Cell<u64>,
    pending: Cell<bool>,
    translation_tx: Sender<TranslationResult>,
    translation_rx: Receiver<TranslationResult>,
    suppress_deactivate: Cell<bool>,
    tsf_receiver: HWND,
    finished: Cell<bool>,
    /// The plain-mode shortcut was pressed in the box: swallow the WM_CHAR it produces.
    swallow_char: Cell<bool>,
    /// Mode last shown in the tray tooltip.
    last_plain: Cell<bool>,
    /// Learned picks, saved in %APPDATA%\\InputTranslation\\learned.txt.
    learned: RefCell<Learned>,
    quotes: Cell<Quotes>,
    /// The draft was recalled with the history shortcut: handle it in translation mode (plain
    /// mode would commit it immediately).
    recall_session: Cell<bool>,
    /// History entry the draft came from (0 = newest).
    history_pos: Cell<Option<usize>>,
    taskbar_created: u32,
    /// Inline English segment (Shift tap while composing): letters, spaces and punctuation go
    /// into the draft as typed until Shift is tapped again.
    english: Cell<bool>,
    shift_tap: Cell<yiime_core::ShiftTap>,
    /// A numpad digit was just pressed: its WM_CHAR does not pick a candidate.
    numpad: Cell<bool>,
}

struct TranslationResult {
    generation: u64,
    source: String,
    result: Result<String, String>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
/// The IME in the language list: language 0804 + TIP CLSID + profile GUID.
const TIP_ID: &str =
    "0804:{A72091C2-7459-4C47-85F4-4626AE42A6C8}{54A39D73-0625-4560-B878-94660908C69D}";
/// Marks that the IME was added to this user's language list once, so it is not re-added after
/// the user removes it. The installer clears it.
const TIP_MARKER: &str = "ime-list-added";
/// Adds the IME to the current (non-admin) user's Chinese input list. Set-WinUserLanguageList
/// in the elevated installer sometimes misses the user session, so the tray makes sure on
/// start.
fn ensure_tip_in_language_list() {
    let Some(dir) = settings_dir() else {
        return;
    };
    let marker = dir.join(TIP_MARKER);
    if marker.exists() {
        return;
    }
    // input.dll!InstallLayoutOrTip is a no-op if already present.
    let added = unsafe {
        let library = LoadLibraryW(wide("input.dll").as_ptr());
        if library.is_null() {
            return;
        }
        let Some(function) = GetProcAddress(library, c"InstallLayoutOrTip".as_ptr().cast()) else {
            return;
        };
        let install: unsafe extern "system" fn(*const u16, u32) -> i32 =
            std::mem::transmute(function);
        install(wide(TIP_ID).as_ptr(), 0) != 0
    };
    if added {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&marker, b"1");
    }
}
/// Settings switches in HKCU\\Software\\InputTranslation (shared with the TSF DLL).
struct Registry;
impl OptionStore for Registry {
    fn get_u32(&self, name: &str) -> Option<u32> {
        let mut value = 0u32;
        let mut size = 4u32;
        (unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(STATE_KEY).as_ptr(),
                wide(name).as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                &mut value as *mut u32 as _,
                &mut size,
            )
        } == 0)
            .then_some(value)
    }
    fn set_u32(&self, name: &str, value: u32) -> bool {
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(STATE_KEY).as_ptr(),
                wide(name).as_ptr(),
                REG_DWORD,
                &value as *const u32 as _,
                4,
            ) == 0
        }
    }
    fn get_string(&self, name: &str) -> Option<String> {
        let mut buffer = [0u16; 64];
        let mut size = (buffer.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(STATE_KEY).as_ptr(),
                wide(name).as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr() as _,
                &mut size,
            )
        } == 0;
        ok.then(|| {
            let end = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..end])
        })
    }
    fn set_string(&self, name: &str, text: &str) -> bool {
        let value = wide(text);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(STATE_KEY).as_ptr(),
                wide(name).as_ptr(),
                REG_SZ,
                value.as_ptr() as _,
                (value.len() * 2) as u32,
            ) == 0
        }
    }
}
const OPTIONS: Options<Registry> = Options(Registry);
fn option(flag: Flag) -> bool {
    OPTIONS.flag(flag)
}
fn set_option(flag: Flag, on: bool) -> bool {
    OPTIONS.set_flag(flag, on)
}
fn plain_mode() -> bool {
    option(Flag::PlainMode)
}
fn set_plain_mode(on: bool) -> bool {
    set_option(Flag::PlainMode, on)
}
/// Shortcut for toggling plain mode; Ctrl+Shift+Space when unset or invalid.
fn plain_toggle_text() -> String {
    OPTIONS.plain_toggle_key()
}
fn history_key_text() -> String {
    OPTIONS.history_key()
}
/// Current options, read from the registry each time so changes from TSF or the settings window
/// apply at once.
fn compose_options() -> ComposeOptions {
    ComposeOptions {
        page_size: OPTIONS.page_size(),
        traditional: option(Flag::Traditional),
        learn: option(Flag::Learn),
        fuzzy: option(Flag::FuzzyPinyin),
        pinyin_first: option(Flag::PinyinFirst),
        common_only: option(Flag::CommonOnly),
    }
}
fn history_path() -> Option<PathBuf> {
    settings_dir().map(|dir| dir.join(HISTORY_FILE))
}
/// Input history, encrypted per user with DPAPI and kept on this machine only.
fn history() -> Option<History<'static>> {
    history_path().map(|path| History::new(path, &Dpapi))
}
fn load_history() -> Vec<HistoryEntry> {
    history().map(|history| history.load()).unwrap_or_default()
}
fn save_history(entries: &[HistoryEntry]) -> bool {
    history().is_some_and(|history| history.save(entries))
}
fn clear_history() -> bool {
    history().is_none_or(|history| history.clear())
}
fn local_time() -> String {
    let mut time: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}
/// Records a committed translation (not when history is off); a repeated sentence keeps only
/// its newest entry.
fn remember_translation(chinese: &str, english: &str) {
    if !option(Flag::HistoryEnabled) || chinese.trim().is_empty() {
        return;
    }
    if let Some(history) = history() {
        history.remember(chinese, english, local_time());
    }
}
/// Loads history entry `index` into the draft (replacing it); the status line shows its time
/// and translation.
unsafe fn show_history(hwnd: HWND, state: &State, index: usize) -> bool {
    let entries = load_history();
    let Some(entry) = entries.get(index) else {
        return false;
    };
    state.history_pos.set(Some(index));
    unsafe {
        set_code(hwnd, state, String::new());
        set_text(state.edit, &entry.chinese);
        let end = GetWindowTextLengthW(state.edit) as usize;
        SendMessageW(state.edit, EM_SETSEL, end, end as isize);
        let preview: String = entry.english.chars().take(40).collect();
        set_status(
            hwnd,
            state,
            Some((
                format!(
                    "记忆 {}/{} · {} · ↑↓ 切换 · 上次译文：{preview}",
                    index + 1,
                    entries.len(),
                    entry.time
                ),
                false,
            )),
        );
    }
    true
}
/// History shortcut: load the newest entry as the draft.
unsafe fn recall_latest(hwnd: HWND, state: &State) {
    state.recall_session.set(true);
    let shown = option(Flag::HistoryEnabled) && unsafe { show_history(hwnd, state, 0) };
    if !shown {
        unsafe {
            set_status(
                hwnd,
                state,
                Some((
                    if option(Flag::HistoryEnabled) {
                        "还没有输入记忆（翻译上屏后会自动记住）".to_owned()
                    } else {
                        "输入记忆已关闭（可在设置里打开）".to_owned()
                    },
                    false,
                )),
            );
        }
    }
}
unsafe fn set_clipboard(owner: HWND, text: &str) -> bool {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    let units = wide(text);
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        let mut ok = false;
        if !memory.is_null() {
            let target = GlobalLock(memory) as *mut u16;
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                GlobalUnlock(memory);
                // CF_UNICODETEXT = 13; on success the system owns the memory.
                ok = !SetClipboardData(13, memory).is_null();
            }
            if !ok {
                GlobalFree(memory);
            }
        }
        CloseClipboard();
        ok
    }
}
fn learned_path() -> Option<PathBuf> {
    settings_dir().map(|dir| dir.join(LEARNED_FILE))
}
fn user_words_path() -> Option<PathBuf> {
    settings_dir().map(|dir| dir.join(USER_WORDS_FILE))
}
/// Records a pick (unless learning is off).
fn remember_pick(state: &State, code: &str, word: &str) {
    if option(Flag::Learn) {
        state.learned.borrow_mut().record(code, word);
    }
}
fn hint(state: &State) -> &'static str {
    if state.english.get() {
        ENGLISH_HINT
    } else if plain_mode() {
        PLAIN_HINT
    } else {
        HINT
    }
}
/// Shift tap: Chinese ↔ English segment. Entering English puts the unpicked code into the draft
/// as letters (as in Microsoft Pinyin, Sogou and Rime); returning to Chinese in plain mode
/// hands the draft to the application.
unsafe fn toggle_inline_english(hwnd: HWND, state: &State) {
    let english = !state.english.get();
    if english {
        let code = state.composition.borrow().code.clone();
        if let Some(letters) = yiime_core::literal_code(&code) {
            unsafe {
                set_code(hwnd, state, String::new());
                insert_text(state, &letters);
            }
        }
    }
    state.english.set(english);
    *state.status.borrow_mut() = None;
    unsafe {
        relayout(hwnd, state);
        if !english {
            flush_plain(hwnd, state);
        }
    }
}
/// Plain Wubi mode: once a composition finishes (a pick, or letters/punctuation committed),
/// hand the draft to the application without translating; if the code is cleared and the draft
/// is empty, just close. Does nothing while translating or outside plain mode.
unsafe fn flush_plain(hwnd: HWND, state: &State) {
    if !plain_mode()
        || state.english.get()
        || state.recall_session.get()
        || state.pending.get()
        || unsafe { IsWindowVisible(hwnd) } == 0
        || !state.composition.borrow().code.is_empty()
    {
        return;
    }
    let draft = unsafe { get_text(state.edit) };
    unsafe {
        if draft.is_empty() {
            cancel_session(hwnd, state);
        } else {
            commit_to_target(hwnd, state, &draft);
        }
    }
}
/// Whether the key is the plain-mode shortcut.
fn is_plain_toggle(vk: u16) -> bool {
    let Some(Hotkey {
        ctrl,
        shift,
        alt,
        vk: wanted,
    }) = parse_hotkey(&plain_toggle_text())
    else {
        return false;
    };
    let down = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
    vk == wanted && down(VK_CONTROL) == ctrl && down(VK_SHIFT) == shift && down(VK_MENU) == alt
}
fn settings_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|path| PathBuf::from(path).join("InputTranslation"))
}
/// Per-user DPAPI encryption for API keys and history.
struct Dpapi;
impl Sealer for Dpapi {
    fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
        unsafe { protect(data, true) }
    }
    fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
        unsafe { protect(data, false) }
    }
}
unsafe fn protect(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = if encrypt {
        unsafe {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    } else {
        unsafe {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return None;
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as _);
    }
    Some(bytes)
}
fn load_settings() -> ApiSettings {
    settings_dir().map_or_else(ApiSettings::default, |dir| ApiSettings::load(&dir, &Dpapi))
}
fn save_settings(settings: &ApiSettings) -> std::io::Result<()> {
    let dir = settings_dir().ok_or_else(|| std::io::Error::other("找不到 APPDATA"))?;
    settings.save(&dir, &Dpapi)
}
unsafe fn get_text(hwnd: HWND) -> String {
    let size = unsafe { GetWindowTextLengthW(hwnd) } as usize;
    let mut buf = vec![0u16; size + 1];
    let actual = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) } as usize;
    String::from_utf16_lossy(&buf[..actual])
}
unsafe fn set_text(hwnd: HWND, text: &str) {
    unsafe {
        SetWindowTextW(hwnd, wide(text).as_ptr());
    }
}
#[allow(clippy::too_many_arguments)]
unsafe fn control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let edge = if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 };
    let child = unsafe {
        CreateWindowExW(
            edge,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            parent,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        )
    };
    if !child.is_null() {
        unsafe {
            SendMessageW(
                child,
                WM_SETFONT,
                GetStockObject(DEFAULT_GUI_FONT) as usize,
                1,
            );
        }
    }
    child
}
unsafe fn label(parent: HWND, text: &str, x: i32, y: i32, w: i32) {
    unsafe {
        control(parent, "STATIC", text, 0, x, y, w, 22, 0);
    }
}
unsafe fn send_unicode(text: &str) -> bool {
    let mut events = Vec::new();
    for unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            events.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0,
                        wScan: unit,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
    }
    unsafe {
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            size_of::<INPUT>() as i32,
        ) == events.len() as u32
    }
}
unsafe fn message(hwnd: HWND, text: &str) {
    unsafe {
        MessageBoxW(hwnd, wide(text).as_ptr(), wide(APP_NAME).as_ptr(), MB_OK);
    }
}
unsafe fn target_ready(target: HWND) -> bool {
    let thread = unsafe { GetWindowThreadProcessId(target, null_mut()) };
    for _ in 0..20 {
        let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        if unsafe {
            GetForegroundWindow() == target
                && GetGUIThreadInfo(thread, &mut info) != 0
                && !info.hwndFocus.is_null()
        } {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}
unsafe fn state_of<'a>(hwnd: HWND) -> Option<&'a State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() }
}
unsafe fn set_status(hwnd: HWND, state: &State, status: Option<(String, bool)>) {
    *state.status.borrow_mut() = status;
    unsafe {
        relayout(hwnd, state);
    }
}
/// Screen rectangle of the target's text cursor: the system caret first, else the mouse
/// position.
unsafe fn caret_of(target: HWND) -> RECT {
    let thread = if target.is_null() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(target, null_mut()) }
    };
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    if thread != 0
        && unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
        && !info.hwndCaret.is_null()
    {
        let mut top_left = POINT {
            x: info.rcCaret.left,
            y: info.rcCaret.top,
        };
        let mut bottom_right = POINT {
            x: info.rcCaret.right,
            y: info.rcCaret.bottom,
        };
        unsafe {
            ClientToScreen(info.hwndCaret, &mut top_left);
            ClientToScreen(info.hwndCaret, &mut bottom_right);
        }
        return RECT {
            left: top_left.x,
            top: top_left.y,
            right: bottom_right.x,
            bottom: bottom_right.y.max(top_left.y + 1),
        };
    }
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
    }
    RECT {
        left: point.x,
        top: point.y,
        right: point.x + 1,
        bottom: point.y + 20,
    }
}
fn valid_rect(rect: &RECT) -> bool {
    rect.bottom > rect.top
        && rect.right >= rect.left
        && (rect.left, rect.top, rect.right, rect.bottom) != (0, 0, 0, 0)
}
fn parse_rect(text: &str) -> Option<RECT> {
    let values: Vec<i32> = text
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [left, top, right, bottom] = values[..] else {
        return None;
    };
    let rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    valid_rect(&rect).then_some(rect)
}
/// Translation timing log: %TEMP%\YiIME-translate.log. Only `Attempt::log_line` is written
/// (engine, model, host, char count, timing, error category / HTTP status); never text,
/// keys or provider responses. Rotated to .old over 512 KB.
fn translate_log(attempt: &yiime_core::Attempt) {
    let message = attempt.log_line();
    use std::io::Write;
    let path = std::env::temp_dir().join("YiIME-translate.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 512 * 1024) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{now} {message}");
    }
}
unsafe fn show_box(hwnd: HWND, state: &State) {
    unsafe {
        relayout(hwnd, state);
        ShowWindow(hwnd, SW_SHOW);
        // When the editor is reused, Windows often refuses a plain SetForegroundWindow and the
        // box shows without keyboard focus; in that case attach to the foreground thread's
        // input queue and retry.
        bring_to_front(hwnd);
        SetFocus(state.edit);
        let end = GetWindowTextLengthW(state.edit) as usize;
        SendMessageW(state.edit, EM_SETSEL, end, end as isize);
    }
}
/// Starts an input: remember the target window and cursor and show the box there. A draft left
/// by clicking away is kept.
unsafe fn activate(hwnd: HWND, state: &State, target: HWND, anchor: Option<RECT>, code: String) {
    state.target.set(target);
    state.finished.set(false);
    state.pending.set(false);
    state.recall_session.set(false);
    state.history_pos.set(None);
    state.english.set(false);
    state.generation.set(state.generation.get() + 1);
    state
        .anchor
        .set(anchor.unwrap_or_else(|| unsafe { caret_of(target) }));
    *state.status.borrow_mut() = None;
    // Warm up the connection to the translation server while typing (no key or text; at most
    // once per 30 s).
    if !plain_mode() {
        let settings = state.settings.borrow().clone();
        std::thread::spawn(move || yiime_core::prewarm(&settings));
    }
    unsafe {
        set_code(hwnd, state, code);
        show_box(hwnd, state);
    }
}
unsafe fn hide_box(hwnd: HWND, state: &State) {
    state.suppress_deactivate.set(true);
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
    state.suppress_deactivate.set(false);
}
unsafe fn clear_box(hwnd: HWND, state: &State) {
    state.english.set(false);
    state.generation.set(state.generation.get() + 1);
    state.pending.set(false);
    *state.status.borrow_mut() = None;
    unsafe {
        set_text(state.edit, "");
        set_code(hwnd, state, String::new());
    }
}
/// Brings the target to the foreground; if the foreground lock refuses, temporarily attach to
/// the foreground thread's input queue and retry.
unsafe fn bring_to_front(target: HWND) -> bool {
    unsafe {
        if SetForegroundWindow(target) != 0 && GetForegroundWindow() == target {
            return true;
        }
        let ours = GetCurrentThreadId();
        let theirs = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let attached = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, 1) != 0;
        BringWindowToTop(target);
        SetForegroundWindow(target);
        if attached {
            AttachThreadInput(ours, theirs, 0);
        }
        GetForegroundWindow() == target
    }
}
/// Program icon (resource 1, from windows/tsf/ime.ico); the system default if missing.
unsafe fn app_icon(size: i32) -> HICON {
    unsafe {
        let icon = LoadImageW(
            GetModuleHandleW(null()),
            std::ptr::without_provenance::<u16>(1),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        ) as HICON;
        if icon.is_null() {
            LoadIconW(null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}
/// Tells the TSF DLL this input ended without inserting text.
unsafe fn finish_tsf(hwnd: HWND, state: &State) -> bool {
    if state.tsf_receiver.is_null() || state.finished.get() {
        return true;
    }
    let ok = unsafe { send_tsf_text(state.tsf_receiver, hwnd, "") };
    if ok {
        state.finished.set(true);
    }
    ok
}
/// Esc (not composing): discard the draft and close the box. TSF is told explicitly so the IME
/// is not left stuck in an edit.
unsafe fn cancel_session(hwnd: HWND, state: &State) {
    if unsafe { !finish_tsf(hwnd, state) } {
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("输入法没有响应，请稍后再按 Esc".into(), true)),
            );
        }
        return;
    }
    unsafe {
        clear_box(hwnd, state);
        let target = state.target.get();
        if !target.is_null() && IsWindow(target) != 0 {
            bring_to_front(target);
        }
        hide_box(hwnd, state);
    }
}
/// The box lost focus (click elsewhere): hide it and end the TSF session, keeping the draft for
/// next time.
unsafe fn deactivate(hwnd: HWND, state: &State) {
    state.generation.set(state.generation.get() + 1);
    state.pending.set(false);
    *state.status.borrow_mut() = None;
    unsafe {
        set_code(hwnd, state, String::new());
        hide_box(hwnd, state);
        finish_tsf(hwnd, state);
    }
}
/// Enter: translate and commit; Shift+Enter: commit the Chinese draft.
unsafe fn submit(hwnd: HWND, state: &State, chinese: bool) {
    if state.pending.get() {
        return;
    }
    let draft = unsafe { get_text(state.edit) };
    if draft.trim().is_empty() {
        unsafe {
            cancel_session(hwnd, state);
        }
        return;
    }
    if chinese || (plain_mode() && !state.recall_session.get()) {
        unsafe {
            commit_to_target(hwnd, state, &draft);
        }
        return;
    }
    let settings = state.settings.borrow().clone();
    if let Err(error) = settings.validate() {
        unsafe {
            set_status(hwnd, state, Some((error.into(), true)));
            open_settings(hwnd);
        }
        return;
    }
    state.pending.set(true);
    unsafe {
        set_status(hwnd, state, Some(("翻译中…".into(), false)));
    }
    let generation = state.generation.get();
    let response_tx = state.translation_tx.clone();
    let window = hwnd as usize;
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            let outcome = yiime_core::translate(&settings, &draft);
            for attempt in &outcome.attempts {
                translate_log(attempt);
            }
            outcome.result
        })
        .unwrap_or_else(|_| Err("翻译处理出错，请重试".into()));
        let response = TranslationResult {
            generation,
            source: draft,
            result,
        };
        if response_tx.send(response).is_ok() {
            unsafe { PostMessageW(window as HWND, WM_TRANSLATED, 0, 0) };
        }
    });
}
/// Switches back to the application and commits the text (through the DLL's edit session under
/// TSF, SendInput when portable).
unsafe fn commit_to_target(hwnd: HWND, state: &State, text: &str) {
    let target = state.target.get();
    if target.is_null() || unsafe { IsWindow(target) } == 0 {
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("找不到原应用窗口，请回到输入框重新唤出".into(), true)),
            );
        }
        return;
    }
    let line = single_line(text);
    // Switch back while the box is still foreground: hiding it first loses the foreground right
    // and SetForegroundWindow is refused.
    let switched = unsafe { bring_to_front(target) };
    unsafe {
        hide_box(hwnd, state);
    }
    let result = if !switched && unsafe { GetForegroundWindow() } != target {
        Err("无法切回原应用")
    } else if unsafe { !target_ready(target) } {
        Err("原应用尚未取得输入焦点")
    } else if !state.tsf_receiver.is_null() {
        if unsafe { send_tsf_text(state.tsf_receiver, hwnd, &line) } {
            state.finished.set(true);
            Ok(())
        } else {
            Err("输入法没有接受提交")
        }
    } else if unsafe { send_unicode(&line) } {
        // UIPI may silently block SendInput (e.g. to an elevated window); that cannot be
        // detected here.
        Ok(())
    } else {
        Err("提交未完成，请检查原应用中的文字")
    };
    match result {
        Ok(()) => unsafe { clear_box(hwnd, state) },
        Err(error) => unsafe {
            *state.status.borrow_mut() = Some((format!("{error}，草稿已保留"), true));
            show_box(hwnd, state);
        },
    }
}
/// Switches to the dictionary with the large lexicon once it is loaded, between compositions;
/// mid-composition the tray timer retries.
fn adopt_full_input(state: &State) {
    if let Some(input) = state.full_input.get()
        && !Arc::ptr_eq(input, &state.input.borrow())
        && state.composition.borrow().code.is_empty()
    {
        *state.input.borrow_mut() = input.clone();
    }
}
unsafe fn on_translated(hwnd: HWND, state: &State, response: TranslationResult) {
    if response.generation != state.generation.get() || !state.pending.get() {
        return;
    }
    state.pending.set(false);
    if response.source != unsafe { get_text(state.edit) } {
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("草稿已修改，请再按 Enter 翻译".into(), false)),
            );
        }
        return;
    }
    match response.result {
        Ok(english) => unsafe {
            *state.status.borrow_mut() = None;
            remember_translation(&response.source, &english);
            commit_to_target(hwnd, state, &english);
        },
        Err(error) => unsafe {
            set_status(hwnd, state, Some((format!("翻译失败：{error}"), true)));
        },
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let state = unsafe { state_of(hwnd) };
    if let Some(state) = state
        && msg == state.taskbar_created
        && msg != 0
    {
        unsafe {
            add_tray(hwnd, None);
        }
        return 0;
    }
    match msg {
        WM_CREATE => unsafe { create(hwnd) },
        WM_PAINT => {
            if let Some(state) = state {
                unsafe {
                    paint(hwnd, state);
                }
                0
            } else {
                unsafe { DefWindowProcW(hwnd, msg, w, l) }
            }
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => MA_ACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            if let Some(state) = state {
                let x = (l & 0xffff) as i16 as i32;
                let y = ((l >> 16) & 0xffff) as i16 as i32;
                let hit = {
                    let composition = state.composition.borrow();
                    (y >= composition.row_top && y < composition.row_top + LINE_H)
                        .then(|| {
                            composition
                                .hits
                                .iter()
                                .position(|(left, right)| x >= left - 3 && x <= right + 3)
                        })
                        .flatten()
                };
                unsafe {
                    if let Some(index) = hit {
                        pick(hwnd, state, index);
                    }
                    SetFocus(state.edit);
                    if hit.is_some() {
                        flush_plain(hwnd, state);
                    }
                }
            }
            0
        }
        WM_ACTIVATE => {
            if let Some(state) = state {
                if (w & 0xffff) as u32 == WA_INACTIVE {
                    let other = l as HWND;
                    if !state.suppress_deactivate.get()
                        && unsafe { IsWindowVisible(hwnd) } != 0
                        && other != state.settings_window.get()
                    {
                        unsafe {
                            deactivate(hwnd, state);
                        }
                    }
                } else {
                    unsafe {
                        SetFocus(state.edit);
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            let notification = ((w >> 16) & 0xffff) as u16;
            if let Some(state) = state
                && id == DRAFT
                && notification == EN_CHANGE_CODE
            {
                let mut status = state.status.borrow_mut();
                if !state.pending.get() {
                    *status = None;
                }
                drop(status);
                unsafe {
                    relayout(hwnd, state);
                }
            }
            0
        }
        WM_TSF_ACTIVATE => {
            // lParam = 1: the IME's history shortcut; open the box and load the newest entry.
            if let Some(state) = state
                && !state.tsf_receiver.is_null()
                && l == 1
            {
                let anchor = state.pending_anchor.take();
                unsafe {
                    activate(hwnd, state, w as HWND, anchor, String::new());
                    recall_latest(hwnd, state);
                    notify_tsf_ready(state.tsf_receiver, hwnd);
                }
            } else if let Some(state) = state
                && !state.tsf_receiver.is_null()
                && (0x41..=0x5a).contains(&(l as usize))
            {
                let anchor = state.pending_anchor.take();
                unsafe {
                    activate(
                        hwnd,
                        state,
                        w as HWND,
                        anchor,
                        (l as u8 as char).to_ascii_lowercase().to_string(),
                    );
                    notify_tsf_ready(state.tsf_receiver, hwnd);
                }
            }
            0
        }
        WM_COPYDATA => unsafe {
            let Some(state) = state else {
                return 0;
            };
            if l == 0 || state.tsf_receiver.is_null() || w as HWND != state.tsf_receiver {
                return 0;
            }
            let packet = &*(l as *const COPYDATASTRUCT);
            match packet.dwData {
                // Keystrokes buffered during startup or activation (letters), merged into the
                // current code.
                2 if !packet.lpData.is_null()
                    && packet.cbData.is_multiple_of(2)
                    && packet.cbData <= 8192 =>
                {
                    let units = std::slice::from_raw_parts(
                        packet.lpData as *const u16,
                        packet.cbData as usize / 2,
                    );
                    let Ok(keys) = String::from_utf16(units) else {
                        return 0;
                    };
                    if IsWindowVisible(hwnd) == 0 {
                        // Session already ended: refuse, and TSF gives the letters back to the
                        // host.
                        return 0;
                    }
                    // Fed through the same handler as typing in the box, so lowercase letters
                    // compose (with fifth-letter and unique-code commits) and uppercase letters
                    // go into the draft as is; anything unhandled is inserted as typed.
                    for ch in keys.chars().filter(char::is_ascii_alphabetic) {
                        if !handle_char(hwnd, state, ch as u32) {
                            insert_text(state, &ch.to_string());
                        }
                    }
                    flush_plain(hwnd, state);
                    if GetForegroundWindow() != hwnd {
                        // The box did not get the foreground (letters would land in the
                        // application): take the keyboard focus back.
                        bring_to_front(hwnd);
                        SetFocus(state.edit);
                    }
                    1
                }
                // Screen rectangle of the target's text cursor (4 i32s), followed by
                // WM_TSF_ACTIVATE.
                3 => {
                    let rect = (packet.cbData as usize == size_of::<RECT>()
                        && !packet.lpData.is_null())
                    .then(|| std::ptr::read_unaligned(packet.lpData as *const RECT))
                    .filter(valid_rect);
                    state.pending_anchor.set(rect);
                    1
                }
                _ => 0,
            }
        },
        WM_FULL_INPUT => {
            if let Some(state) = state {
                adopt_full_input(state);
            }
            0
        }
        WM_TSF_EXIT => {
            if let Some(state) = state {
                state.finished.set(true);
            }
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_TRANSLATED => {
            if let Some(state) = state {
                while let Ok(response) = state.translation_rx.try_recv() {
                    unsafe { on_translated(hwnd, state, response) };
                }
            }
            0
        }
        WM_TIMER if w == TRAY_TIMER => {
            if let Some(state) = state {
                adopt_full_input(state);
                if state.last_plain.get() != plain_mode() {
                    state.last_plain.set(plain_mode());
                    unsafe { update_tray_tip(hwnd) };
                }
            }
            0
        }
        WM_OPEN_SETTINGS => {
            unsafe {
                open_settings(hwnd);
            }
            0
        }
        WM_OPEN_HISTORY => {
            unsafe {
                open_history();
            }
            0
        }
        WM_TRAY => {
            match (l & 0xffff) as u32 {
                // Version 4: right-click WM_CONTEXTMENU, left-click NIN_SELECT, keyboard
                // NIN_KEYSELECT.
                WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT if TRAY_V4.load(Ordering::Relaxed) => unsafe {
                    tray_menu(hwnd)
                },
                // Pre-version-4 callback (if setting the version failed): right button up.
                WM_RBUTTONUP if !TRAY_V4.load(Ordering::Relaxed) => unsafe { tray_menu(hwnd) },
                WM_LBUTTONDBLCLK => unsafe { open_settings(hwnd) },
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            if let Some(state) = state {
                unsafe {
                    cancel_session(hwnd, state);
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                let data = tray_data(hwnd);
                Shell_NotifyIconW(NIM_DELETE, &data);
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
                if !ptr.is_null() {
                    if !(*ptr).tsf_receiver.is_null() && !(*ptr).finished.get() {
                        send_tsf_text((*ptr).tsf_receiver, hwnd, "");
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let state = Box::from_raw(ptr);
                    let settings = state.settings_window.get();
                    if !settings.is_null() {
                        DestroyWindow(settings);
                    }
                    DeleteObject(state.font as _);
                    DeleteObject(state.small_font as _);
                }
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
unsafe fn create(hwnd: HWND) -> LRESULT {
    let args: Vec<String> = std::env::args().collect();
    let argument = |name: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].as_str())
    };
    let handle = |name: &str| {
        argument(name)
            .and_then(|value| value.parse::<usize>().ok())
            .map_or(null_mut(), |value| value as HWND)
    };
    let tsf_receiver = handle("--tsf-hwnd");
    let target = handle("--target-hwnd");
    let input = MixedInput::bundled();
    // User words: %APPDATA%\\InputTranslation\\user_words.txt, `word code` per line, ranked
    // first under that code.
    let user_words = user_words_path().and_then(|path| std::fs::read_to_string(path).ok());
    let input = match &user_words {
        Some(words) => input.with_user_words(words),
        None => input,
    };
    // The box opens with the bundled dictionaries; the large lexicon replaces them once loaded.
    // The initials index (about 5 MB, 0.1–0.2 s) is built off the UI thread, only by the box (the
    // tray instance does not compose), and only for the dictionary it keeps.
    let words_path = dictionary::frequency_words_path();
    let warm = !tsf_receiver.is_null();
    let input = Arc::new(input);
    let full_input = Arc::new(OnceLock::new());
    if warm && words_path.is_none() {
        let warm = input.clone();
        std::thread::spawn(move || warm.warm_up());
    }
    unsafe {
        let font = create_font(18);
        let small_font = create_font(13);
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32,
            PAD,
            PAD,
            MIN_W - 2 * PAD,
            EDIT_H,
            hwnd,
            DRAFT as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        // This program composes letters itself; turn off the system IME for the edit box so two
        // compositions do not conflict.
        ImmAssociateContextEx(edit, null_mut(), 0);
        let previous = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
        SetWindowLongPtrW(edit, GWLP_USERDATA, previous);
        let full_ready = full_input.clone();
        let (translation_tx, translation_rx) = std::sync::mpsc::channel();
        let state = Box::new(State {
            input: RefCell::new(input),
            full_input,
            edit,
            font,
            small_font,
            draft_h: Cell::new(EDIT_H),
            composition: RefCell::new(Composition::default()),
            status: RefCell::new(None),
            settings: RefCell::new(load_settings()),
            settings_window: Cell::new(null_mut()),
            anchor: Cell::new(caret_of(target)),
            pending_anchor: Cell::new(None),
            target: Cell::new(target),
            generation: Cell::new(0),
            pending: Cell::new(false),
            translation_tx,
            translation_rx,
            suppress_deactivate: Cell::new(false),
            tsf_receiver,
            finished: Cell::new(false),
            swallow_char: Cell::new(false),
            last_plain: Cell::new(plain_mode()),
            learned: RefCell::new(Learned::load(learned_path())),
            quotes: Cell::new(Quotes::default()),
            english: Cell::new(false),
            shift_tap: Cell::new(yiime_core::ShiftTap::default()),
            numpad: Cell::new(false),
            recall_session: Cell::new(false),
            history_pos: Cell::new(None),
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        });
        let standalone = tsf_receiver.is_null();
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        if let Some(path) = words_path {
            dictionary::load_full_in_background(
                path,
                user_words,
                warm,
                full_ready,
                hwnd as usize,
                WM_FULL_INPUT,
            );
        }
        add_tray(
            hwnd,
            // Show the balloon only on a first launch without arguments (not for
            // --settings/--history).
            (standalone && std::env::args().count() == 1).then_some(
                "用 Win+空格 切换到“YiIME 译（五笔）”后直接打字即可。单击或右键托盘图标可打开菜单。",
            ),
        );
        SetTimer(hwnd, TRAY_TIMER, 2000, None);
    }
    0
}
pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let tsf_mode = args.iter().any(|arg| arg == "--tsf-hwnd");
    let wants_settings = args.iter().any(|arg| arg == "--settings");
    let wants_history = args.iter().any(|arg| arg == "--history");
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = wide("InputTranslationWindow");
        if !tsf_mode {
            ensure_tip_in_language_list();
            // Keep one tray instance: a second launch (e.g. the Start-menu Settings shortcut)
            // just tells the existing one to open settings.
            let existing = FindWindowW(class.as_ptr(), wide(TRAY_TITLE).as_ptr());
            if !existing.is_null() {
                let mut pid = 0;
                GetWindowThreadProcessId(existing, &mut pid);
                AllowSetForegroundWindow(pid);
                let request = if wants_history {
                    WM_OPEN_HISTORY
                } else {
                    WM_OPEN_SETTINGS
                };
                PostMessageW(existing, request, 0, 0);
                return;
            }
        }
        let wc = WNDCLASSW {
            style: CS_DROPSHADOW,
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let history_class = wide("InputTranslationHistory");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(history_proc),
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            hInstance: instance,
            lpszClassName: history_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE as usize + 1) as _,
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let settings_class = wide("InputTranslationSettings");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(settings_proc),
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            hInstance: instance,
            lpszClassName: settings_class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE as usize + 1) as _,
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide(if tsf_mode { TSF_TITLE } else { TRAY_TITLE }).as_ptr(),
            WS_POPUP | WS_BORDER,
            0,
            0,
            MIN_W,
            80,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        let Some(state) = state_of(hwnd) else {
            return;
        };
        let argument = |name: &str| {
            args.windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].as_str())
        };
        if !state.tsf_receiver.is_null() {
            watch_tsf_owner(state.tsf_receiver, hwnd);
            let initial: String = argument("--initial")
                .unwrap_or("")
                .chars()
                .filter(char::is_ascii_alphabetic)
                .map(|ch| ch.to_ascii_lowercase())
                .take(MAX_CODE_LEN)
                .collect();
            let anchor = argument("--caret").and_then(parse_rect);
            activate(hwnd, state, state.target.get(), anchor, initial);
            if args.iter().any(|arg| arg == "--recall") {
                recall_latest(hwnd, state);
            }
            notify_tsf_ready(state.tsf_receiver, hwnd);
        } else if wants_history {
            open_history();
        } else if wants_settings || state.settings.borrow().validate().is_err() {
            open_settings(hwnd);
        }
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let settings = state_of(hwnd).map_or(null_mut(), |state| state.settings_window.get());
            if !settings.is_null() && IsDialogMessageW(settings, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
