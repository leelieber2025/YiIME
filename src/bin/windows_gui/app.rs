//! Windows candidate box, tray, settings and TSF companion process.
use super::dictionary;
mod candidate;
mod history;
mod ipc;
mod key;
use candidate::{create_font, paint, relayout, scaled_line_h};
use ipc::{notify_tsf_ready, send_tsf_english, send_tsf_held, send_tsf_text, watch_tsf_owner};
use key::{edit_proc, handle_char, insert_text, pick, set_code};
mod settings;
mod tray;
use history::{history_proc, open_history};
use settings::{open_settings, settings_proc};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, OnceLock};
use tray::{add_tray, tray_data, tray_menu, update_tray_tip};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, HWND, LPARAM, LRESULT,
    LocalFree, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, ClientToScreen,
    CreateFontIndirectW, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_GUI_FONT, DT_CALCRECT, DT_EDITCONTROL, DT_NOPREFIX, DT_WORDBREAK,
    DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetMonitorInfoW, GetStockObject,
    GetTextExtentPoint32W, GetTextMetricsW, HBRUSH, HDC, HFONT, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow, NULL_BRUSH,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkColor,
    SetBkMode, SetTextColor, SetWindowRgn, TEXTMETRICW, TRANSPARENT, TextOutW,
};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetKeyValueW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, CreateMutexW, GetCurrentThreadId, OpenProcess, PROCESS_SYNCHRONIZE,
    ReleaseMutex, WaitForSingleObject,
};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Controls::{
    EM_GETSEL, EM_LIMITTEXT, EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL, SetScrollInfo, ShowScrollBar,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
    GetDpiForWindow, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::Ime::ImmAssociateContextEx;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, SetFocus, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END,
    VK_HOME, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use yiime_core::{
    ApiSettings, ComposeOptions, Composer, Engine, Flag, HISTORY_CAP, HISTORY_FILE, History,
    HistoryEntry, HostOwner, Hotkey, LEARNED_FILE, Learned, LetterRules, MAX_CODE_LEN, MixedInput,
    OptionStore, Options, Quotes, Sealer, Shuangpin, TranslationStyle, USER_WORDS_FILE,
    UserWordsReloader, WatchedHosts, history_label, parse_hotkey, parse_page_size, selection_index,
    single_line,
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
/// Label control IDs in the settings window (unique so the layout can move them).
const LBL_ENGINE: i32 = 240;
const LBL_GOOGLE_KEY: i32 = 241;
const LBL_AZURE_KEY: i32 = 242;
const LBL_AZURE_REGION: i32 = 243;
const LBL_DEEPL: i32 = 244;
const LBL_URL: i32 = 245;
const LBL_MODEL: i32 = 246;
const LBL_KEY: i32 = 247;
const LBL_STYLE: i32 = 248;
const LBL_CUSTOM: i32 = 249;
const LBL_HOTKEY: i32 = 250;
const LBL_PAGE_SIZE: i32 = 251;
const LBL_SCHEME: i32 = 252;
const LBL_HISTORY_KEY: i32 = 253;
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const TRAY_SETTINGS: usize = 1;
const TRAY_EXIT: usize = 2;
const TRAY_PLAIN: usize = 3;
const TRAY_TRANSLATE: usize = 5;
/// Polls the mode (also changed by IME shortcuts and the taskbar button) to refresh the tray
/// tooltip, and retries switching to the large lexicon if it arrived mid-composition.
const TRAY_TIMER: usize = 7;
/// The resident TSF service exits after this long without a session (it restarts on demand in
/// well under 0.1 s thanks to the precompiled dictionary index).
const IDLE_TIMER: usize = 8;
const IDLE_EXIT_MS: u32 = 30 * 60 * 1000;
/// Keeps the watchdog heartbeat going while the message loop is idle.
const HEARTBEAT_TIMER: usize = 9;
/// While the TSF box is shown: hides it when it is empty and does not have the keyboard.
const EMPTY_BOX_TIMER: usize = 10;
/// Window class of the TSF DLL's message-only receiver windows.
/// Receivers of DLLs speaking another protocol version have another class and are refused.
const TSF_RECEIVER_CLASS: &str = yiime_core::TSF_RECEIVER_CLASS;
/// Registry key shared with the TSF DLL.
const STATE_KEY: &str = "Software\\YiIME";
/// Settings key and %APPDATA% folder used by 0.1.x; migrated once (see [`migrate_settings`]).
const LEGACY_STATE_KEY: &str = "Software\\InputTranslation";
const LEGACY_SETTINGS_DIR: &str = "InputTranslation";
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
/// From the TSF DLL (wParam = receiver): the focus in that application moved (another text field,
/// the page, another window); lParam 1: the DLL ended the session (another text field).
const WM_TSF_FOCUS_MOVED: u32 = WM_APP + 8;
/// From the TSF DLL (wParam = receiver): that application switched to English (Shift tap,
/// Ctrl+Space, the mode button…); its session ends.
const WM_TSF_ENGLISH: u32 = WM_APP + 9;
/// From the TSF DLL (wParam = receiver, lParam = request ID): a commit landed. Sent
/// besides the COPYDATA reply, which can be lost to a timeout while the focus moves.
const COMMIT_TIMER: usize = 11;
const DICT_RETRY_TIMER: usize = 12;
const MAX_DICT_RETRIES: u8 = 3;
const WM_TSF_COMMIT_FAILED: u32 = WM_APP + 16;
const WM_TSF_COMMITTED: u32 = WM_APP + 10;
/// From the TSF DLL (wParam = receiver, lParam 1 = private): the session's field declared a
/// private InputScope (password, PIN, or a browser's private window). Nothing typed there is
/// learned, kept in the history or sent to a translation service.
const WM_TSF_PRIVATE: u32 = WM_APP + 11;
/// Host-exit watch failed (`WaitForSingleObject` did not return `WAIT_OBJECT_0`): do not drop
/// stashes or close a live host; mark watch failed so the next session re-registers.
const WM_TSF_WATCH_FAILED: u32 = WM_APP + 17;
/// Background user-dictionary import finished.
const WM_IMPORT_DONE: u32 = WM_APP + 18;
/// Background lexicon load failed (`wParam == 1` if missing / permanent, `0` if transient).
const WM_FULL_INPUT_ERROR: u32 = WM_APP + 19;
/// Tray (portable) and TSF editor instances use different titles so each can find its single
/// instance.
const TRAY_TITLE: &str = yiime_core::TRAY_WINDOW_TITLE;
/// Window classes of the input-history and settings windows (this EXE only).
const HISTORY_CLASS: &str = "YiIMEHistory";
const SETTINGS_CLASS: &str = "YiIMESettings";
const APP_NAME: &str = "YiIME 译";
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
static TRAY_V4: AtomicBool = AtomicBool::new(false);
/// `--self-test`: headless smoke test for CI (no tray icon, no background loading, no registry
/// writes); see [`self_test`].
static SELF_TEST: AtomicBool = AtomicBool::new(false);
/// Last message-loop beat in milliseconds since [`watch_start`] (0 = none yet).
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
fn watch_start() -> std::time::Instant {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}
fn watch_now_ms() -> u64 {
    watch_start().elapsed().as_millis() as u64
}
fn heartbeat() {
    HEARTBEAT.store(watch_now_ms().max(1), Ordering::Relaxed);
}
/// Diagnostic log of the service (events only, never typed text).
fn service_log(message: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("YiIME-service.log");
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
        let _ = writeln!(file, "{now} pid={} {message}", std::process::id());
    }
}
/// Exit code of a service ended by its watchdog.
const WATCHDOG_EXIT: i32 = 3;
/// The shared TSF service ends itself when its message loop stops beating (see
/// yiime_core::Watchdog), so the TSF DLL starts a fresh one instead of waiting on a hung window.
fn start_watchdog(hwnd: HWND, watchdog: yiime_core::Watchdog) {
    watch_start();
    unsafe {
        SetTimer(hwnd, HEARTBEAT_TIMER, 1000, None);
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let now = watch_now_ms();
            let last = HEARTBEAT.load(Ordering::Relaxed);
            if watchdog.stalled(last, now) {
                service_log(&format!(
                    "消息循环 {} ms 无响应，退出以便输入法重新启动",
                    now.saturating_sub(last.max(watchdog.grace_ms))
                ));
                std::process::exit(WATCHDOG_EXIT);
            }
        }
    });
}
const EN_CHANGE_CODE: u16 = 0x0300;
const SETTINGS_PAGE_SIZE: i32 = 223;
const SETTINGS_ENGINE: i32 = 224;
const SETTINGS_GOOGLE_KEY: i32 = 225;
const SETTINGS_AZURE_KEY: i32 = 226;
const SETTINGS_AZURE_REGION: i32 = 227;
const SETTINGS_DEEPL_KEY: i32 = 228;
const SETTINGS_FALLBACK: i32 = 231;
const SETTINGS_SCHEME: i32 = 232;
const SETTINGS_IMPORT: i32 = 233;
/// Message-font size used if `SPI_GETNONCLIENTMETRICS` is unavailable (12px at 96 DPI).
const SETTINGS_FONT_PX: i32 = 12;
/// Longest custom LLM instruction kept from the settings window, in characters.
const MAX_CUSTOM_LEN: usize = 500;
const PAD: i32 = 8;
const EDIT_H: i32 = 22;
const LINE_H: i32 = 22;
const GAP: i32 = 16;
const MIN_W: i32 = 280;
/// Drafts wrap beyond this width and then grow only in height.
const DRAFT_MAX_W: i32 = 480;
/// Draft lines shown before it scrolls inside the box.
const DRAFT_MAX_LINES: i32 = 8;
const HINT: &str = "Enter 翻译上屏 · Shift+Enter 中文上屏 · Shift 夹英文 · Esc 取消";
const ENGLISH_HINT: &str = "[英] 字母原样输入 · 单击 Shift 回中文 · Enter 翻译上屏";
const PLAIN_HINT: &str = "普通五笔模式 · 空格/数字 选字上屏 · Enter 上屏字母 · Esc 取消";
// COLORREF is 0x00BBGGRR.
const BLUE: u32 = 0x00C0_6000;
const MUTED: u32 = 0x0070_7070;
const RED: u32 = 0x0000_00C0;
const INK: u32 = 0x0020_2020;
const HIGHLIGHT: u32 = 0x00EB_EBEB;
const ACCENT: u32 = 0x005C_5C5C;
const BACKGROUND: u32 = 0x00F7_F7F7;
const BORDER: u32 = 0x00D8_D8D8;
const SEPARATOR: u32 = 0x00E0_E0E0;

/// Input-box state: composition lives in the core (`yiime_core::Composer`); this adds the
/// candidate click areas.
#[derive(Default)]
struct Composition {
    engine: Composer,
    /// Horizontal extent (client coordinates) of each candidate on this page, for mouse clicks.
    hits: Vec<(i32, i32, i32)>,
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

struct AwaitingCommit {
    identity: yiime_core::CommitIdentity,
    history: Option<(String, String)>,
    started: std::time::Instant,
    notified: bool,
    awaiting_receipt: bool,
    /// Text sent; a retry ("未上屏，重试") sends exactly this again under the same request id.
    text: String,
    /// Chinese draft committed back into an application the user left mid-translation. Kept
    /// until that write is confirmed; restored in that application's box if it fails or never
    /// confirms.
    stash: Option<String>,
    /// The DLL reported that the stashed write failed.
    failed: bool,
    /// Parked across fields inside the same application (never auto-restored into a new field).
    same_app: bool,
    /// Owner PID of `identity.receiver` when the stash was created (guards handle reuse).
    host_pid: Option<u32>,
}
/// Status shown when a commit got no receipt: the two choices the user has.
const UNCONFIRMED_STATUS: &str =
    "尚未收到上屏确认，请核对原输入框：已上屏按 Esc 清空草稿；未上屏按 Enter 重试";
/// Whether the commit of the current draft is still outstanding, and in which phase.
fn outstanding(state: &State) -> Option<yiime_core::Outstanding> {
    let receiver = state.tsf_receiver.get() as usize;
    let revision = state.generation.get();
    state
        .receipts
        .borrow()
        .iter()
        .find(|p| p.stash.is_none() && p.identity.owns_draft(receiver, revision))
        .map(|p| {
            if p.notified {
                yiime_core::Outstanding::Unconfirmed
            } else {
                yiime_core::Outstanding::Waiting
            }
        })
}
/// First request id of this process: not 1, so a restarted box does not reuse the ids the DLL
/// remembers from the previous one (see [`yiime_core::LandedRequests`]).
fn first_request_id() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    (std::process::id() << 16) ^ nanos
}
struct State {
    input: RefCell<Arc<MixedInput>>,
    full_input: Arc<OnceLock<Arc<MixedInput>>>,
    edit: HWND,
    font: Cell<HFONT>,
    small_font: Cell<HFONT>,
    /// Fill for the box and the draft edit (Win11 light gray).
    bg_brush: HBRUSH,
    /// Last DPI used to build `font` / `small_font`.
    font_dpi: Cell<u32>,
    /// Current draft height (grows with multi-line content).
    draft_h: Cell<i32>,
    composition: RefCell<Composition>,
    /// Status line: (text, is error).
    status: RefCell<Option<(String, bool)>>,
    settings: RefCell<ApiSettings>,
    /// Stamp of the settings files behind `settings` (see [`current_settings`]).
    settings_stamp: Cell<Option<yiime_core::SettingsStamp>>,
    settings_window: Cell<HWND>,
    /// Snapshot of settings when the settings window opened, for merge-on-save.
    dialog_base: RefCell<Option<ApiSettings>>,
    dialog_keys_unreadable: Cell<bool>,
    /// Dialog font for the settings window (message font, scaled to the window DPI).
    settings_font: Cell<HFONT>,
    settings_dpi: Cell<u32>,
    /// Minimum client size that fits every label (unscrolled content height).
    settings_min: Cell<(i32, i32)>,
    settings_scroll: Cell<i32>,
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
    /// TSF receiver window of the current session. The TSF-mode box is one resident service
    /// shared by every application: each session may come from another application's receiver.
    tsf_receiver: Cell<HWND>,
    /// Started by the TSF DLL (`--tsf-hwnd`): serves TSF sessions instead of the tray.
    tsf_service: bool,
    /// Started by an elevated / high-integrity application (`--private`): serves only that
    /// application's receiver and exits when its session or host ends.
    private: bool,
    /// Receivers whose host process is being watched (see [`watch_tsf_owner`]), keyed by handle
    /// and owner so reused window handles are watched again.
    watched_receivers: RefCell<WatchedHosts>,
    /// When the box was last shown (for the empty-box rule).
    shown_at: Cell<Option<std::time::Instant>>,
    /// user_words.txt as last loaded; checked when a session or a code starts.
    user_words: RefCell<UserWordsReloader>,
    finished: Cell<bool>,
    /// A TSF commit that got no reply, kept as a draft until the DLL confirms it landed.
    pending_history: RefCell<Option<(String, String)>>,
    receipts: RefCell<Vec<AwaitingCommit>>,
    next_request: Cell<u32>,
    /// The current session's field is private (WM_TSF_PRIVATE); reset at each session start.
    private_field: Cell<bool>,
    /// The plain-mode shortcut was pressed in the box: swallow the WM_CHAR it produces.
    swallow_char: Cell<bool>,
    /// Mode last shown in the tray tooltip.
    last_plain: Cell<bool>,
    /// Learned picks, saved DPAPI-encrypted in %APPDATA%\\YiIME\\learned.dat.
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
    /// Current session was opened by typing a key (blocks cross-app draft auto-restore).
    key_started: Cell<bool>,
    import_tx: Sender<Result<(usize, usize), String>>,
    import_rx: Receiver<Result<(usize, usize), String>>,
    import_busy: Cell<bool>,
    dict_retries: Cell<u8>,
    dict_words_path: RefCell<Option<PathBuf>>,
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
/// Settings switches in HKCU\\Software\\YiIME (shared with the TSF DLL).
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
    if SELF_TEST.load(Ordering::Relaxed) {
        return flag.default_on();
    }
    OPTIONS.flag(flag)
}
fn set_option(flag: Flag, on: bool) -> bool {
    OPTIONS.set_flag(flag, on)
}
/// 译/普通五笔, shared with packaged hosts through the session cell (see windows/plain_cell.rs).
fn plain_mode() -> bool {
    if SELF_TEST.load(Ordering::Relaxed) {
        return false;
    }
    crate::plain_cell::read(option(Flag::PlainMode), |plain| {
        set_option(Flag::PlainMode, plain)
    })
}
fn set_plain_mode(on: bool) -> bool {
    let ok = set_option(Flag::PlainMode, on);
    crate::plain_cell::wrote(on);
    ok
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
    if SELF_TEST.load(Ordering::Relaxed) {
        return ComposeOptions {
            page_size: yiime_core::PAGE_SIZE,
            traditional: Flag::Traditional.default_on(),
            learn: Flag::Learn.default_on(),
            fuzzy: Flag::FuzzyPinyin.default_on(),
            pinyin_first: Flag::PinyinFirst.default_on(),
            common_only: Flag::CommonOnly.default_on(),
            shuangpin: None,
        };
    }
    ComposeOptions {
        page_size: OPTIONS.page_size(),
        traditional: option(Flag::Traditional),
        learn: option(Flag::Learn),
        fuzzy: option(Flag::FuzzyPinyin),
        pinyin_first: option(Flag::PinyinFirst),
        common_only: option(Flag::CommonOnly),
        shuangpin: OPTIONS.shuangpin(),
    }
}
fn history_path() -> Option<PathBuf> {
    settings_dir().map(|dir| dir.join(HISTORY_FILE))
}
/// Input history, encrypted per user with DPAPI and kept on this machine only.
fn history() -> Option<History<'static>> {
    history_path().map(|path| History::new(path, &Dpapi))
}
fn load_history() -> Option<Vec<HistoryEntry>> {
    history().and_then(|history| history.try_load())
}
fn clear_history() -> bool {
    let cleared = history().is_none_or(|history| history.clear());
    if cleared {
        history::refresh_open_history();
    }
    cleared
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
    if SELF_TEST.load(Ordering::Relaxed)
        || !option(Flag::HistoryEnabled)
        || chinese.trim().is_empty()
    {
        return;
    }
    if let Some(history) = history() {
        history.remember(chinese, english, local_time());
    }
}
/// Loads history entry `index` into the draft (replacing it); the status line shows its time
/// and translation.
unsafe fn show_history(hwnd: HWND, state: &State, entries: &[HistoryEntry], index: usize) -> bool {
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
    if !option(Flag::HistoryEnabled) {
        state.recall_session.set(false);
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("输入记忆已关闭（可在设置里打开）".to_owned(), false)),
            );
        }
        return;
    }
    let Some(entries) = load_history() else {
        state.recall_session.set(false);
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("历史暂时无法读取，原文件已保留".to_owned(), true)),
            );
        }
        return;
    };
    let shown = unsafe { show_history(hwnd, state, &entries, 0) };
    state.recall_session.set(shown);
    if !shown {
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("还没有输入记忆（翻译上屏后会自动记住）".to_owned(), false)),
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
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        let mut ok = false;
        if !memory.is_null() {
            let target = GlobalLock(memory) as *mut u16;
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                GlobalUnlock(memory);
                // CF_UNICODETEXT = 13; on success the system owns the memory.
                ok = EmptyClipboard() != 0 && !SetClipboardData(13, memory).is_null();
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
    if !SELF_TEST.load(Ordering::Relaxed) && option(Flag::Learn) && !state.private_field.get() {
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
/// Shift tap in an empty TSF box: like a Shift tap in the application, switch it to English, and
/// close the box right away (end the session, give the focus back).
unsafe fn switch_to_english(hwnd: HWND, state: &State) {
    let receiver = state.tsf_receiver.get();
    service_log("session end: Shift tap in the empty box, switching to English");
    unsafe {
        cancel_session(hwnd, state);
        if !receiver.is_null() && !send_tsf_english(receiver, hwnd) {
            service_log("Shift 切换英文：输入法没有响应");
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
/// %APPDATA%\\YiIME; the 0.1.x folder %APPDATA%\\InputTranslation while it could not be moved yet.
fn settings_dir() -> Option<PathBuf> {
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    let dir = appdata.join("YiIME");
    let legacy = appdata.join(LEGACY_SETTINGS_DIR);
    Some(if !dir.exists() && legacy.is_dir() {
        legacy
    } else {
        dir
    })
}
/// Moves 0.1.x settings to the YiIME names once: %APPDATA%\\InputTranslation → %APPDATA%\\YiIME
/// (API settings, encrypted keys, history, learned data, user words; DPAPI blobs are not tied to
/// the path) and HKCU\\Software\\InputTranslation → HKCU\\Software\\YiIME (copied; the TSF DLL
/// does the same on activation, and old DLLs still loaded somewhere keep the old key).
fn migrate_settings() {
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let dir = appdata.join("YiIME");
        let legacy = appdata.join(LEGACY_SETTINGS_DIR);
        if !dir.exists() && legacy.is_dir() {
            // Fails while another 0.1.x process holds a file open; settings_dir() then keeps using
            // the old folder and the move is retried on the next start.
            let _ = std::fs::rename(&legacy, &dir);
        }
    }
    unsafe {
        let marker = wide(yiime_core::LEGACY_MIGRATION_MARKER);
        let mut migrated = 0u32;
        let mut marker_size = 4u32;
        if RegGetValueW(
            HKEY_CURRENT_USER,
            wide(STATE_KEY).as_ptr(),
            marker.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut migrated as *mut u32 as _,
            &mut marker_size,
        ) == 0
            && migrated == 1
        {
            return;
        }
        let mut old = null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(LEGACY_STATE_KEY).as_ptr(),
            0,
            KEY_READ,
            &mut old,
        ) != 0
        {
            return;
        }
        let mut new = null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(STATE_KEY).as_ptr(),
            0,
            null(),
            0,
            KEY_READ | KEY_WRITE,
            null(),
            &mut new,
            null_mut(),
        ) == 0
        {
            let mut index = 0u32;
            let mut all_ok = true;
            loop {
                let mut name_buf = vec![0u16; 256];
                let mut name_len = name_buf.len() as u32;
                let mut value_type = 0u32;
                let mut data_buf = vec![0u8; 1024];
                let mut data_len = data_buf.len() as u32;
                let mut status = RegEnumValueW(
                    old,
                    index,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    null_mut(),
                    &mut value_type,
                    data_buf.as_mut_ptr(),
                    &mut data_len,
                );
                if status == ERROR_MORE_DATA {
                    name_buf.resize(name_len as usize + 1, 0);
                    name_len = name_buf.len() as u32;
                    data_buf.resize(data_len as usize, 0);
                    status = RegEnumValueW(
                        old,
                        index,
                        name_buf.as_mut_ptr(),
                        &mut name_len,
                        null_mut(),
                        &mut value_type,
                        data_buf.as_mut_ptr(),
                        &mut data_len,
                    );
                }
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != 0 {
                    all_ok = false;
                    break;
                }
                name_buf.truncate(name_len as usize);
                name_buf.push(0);
                let exists = RegQueryValueExW(
                    new,
                    name_buf.as_ptr(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                );
                if exists == ERROR_FILE_NOT_FOUND
                    && RegSetValueExW(
                        new,
                        name_buf.as_ptr(),
                        0,
                        value_type,
                        data_buf.as_ptr(),
                        data_len,
                    ) != 0
                {
                    all_ok = false;
                }
                index += 1;
            }
            if all_ok {
                let done = 1u32;
                let _ = RegSetValueExW(
                    new,
                    marker.as_ptr(),
                    0,
                    REG_DWORD,
                    &done as *const u32 as _,
                    4,
                );
            }
            RegCloseKey(new);
        }
        RegCloseKey(old);
    }
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
/// The API settings as saved now. The settings window usually runs in the tray process while
/// this resident input box translates, so a saved change (another engine, a new key) is picked up
/// here by the file stamps instead of waiting for the box to restart.
fn current_settings(state: &State) -> ApiSettings {
    if let Some(dir) = settings_dir() {
        let stamp = yiime_core::SettingsStamp::of(&dir);
        if state.settings_stamp.get() != Some(stamp) || state.settings.borrow().keys_unreadable {
            let settings = ApiSettings::load(&dir, &Dpapi);
            service_log(&format!(
                "settings reloaded: engine={} fallback={}",
                settings.engine.id(),
                settings.allow_fallback
            ));
            *state.settings.borrow_mut() = settings;
            state.settings_stamp.set(Some(stamp));
        }
    }
    state.settings.borrow().clone()
}
fn save_settings_merged(
    settings: &ApiSettings,
    base: Option<&ApiSettings>,
) -> std::io::Result<ApiSettings> {
    let dir = settings_dir().ok_or_else(|| std::io::Error::other("找不到 APPDATA"))?;
    settings.save_merged(base, &dir, &Dpapi)
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
    state.shown_at.set(Some(std::time::Instant::now()));
    if state.tsf_service {
        unsafe { SetTimer(hwnd, EMPTY_BOX_TIMER, 250, None) };
    }
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
    unsafe {
        KillTimer(hwnd, IDLE_TIMER);
    }
    refresh_user_words(state);
    if state.tsf_service {
        service_log("session start");
    }
    state.target.set(target);
    state.finished.set(false);
    state.pending.set(false);
    state.recall_session.set(false);
    // Ordinary until the DLL reports a private InputScope (WM_TSF_PRIVATE). The report can come
    // late (Word answers the read only with the commit), so unknown must not block translation.
    state.private_field.set(false);
    state.history_pos.set(None);
    state.pending_history.borrow_mut().take();
    state.english.set(false);
    state.generation.set(state.generation.get() + 1);
    let placed = anchor.unwrap_or_else(|| unsafe { caret_of(target) });
    state.anchor.set(placed);
    if state.tsf_service {
        service_log(&format!(
            "draft box anchor=({},{},{},{}) source={}",
            placed.left,
            placed.top,
            placed.right,
            placed.bottom,
            if anchor.is_some() { "tsf" } else { "caret_of" }
        ));
    }
    *state.status.borrow_mut() = None;
    // Warm up the connection to the translation server while typing (no key or text; at most
    // once per 30 s).
    if !plain_mode() {
        let settings = current_settings(state);
        std::thread::spawn(move || yiime_core::prewarm(&settings));
    }
    unsafe {
        set_code(hwnd, state, code);
        show_box(hwnd, state);
    }
}
unsafe fn hide_box(hwnd: HWND, state: &State) {
    // Session over: write learning changes that were held back while typing.
    if let Ok(mut learned) = state.learned.try_borrow_mut() {
        learned.flush();
    }
    state.suppress_deactivate.set(true);
    unsafe {
        KillTimer(hwnd, EMPTY_BOX_TIMER);
        ShowWindow(hwnd, SW_HIDE);
        if state.tsf_service {
            SetTimer(hwnd, IDLE_TIMER, IDLE_EXIT_MS, None);
        }
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
/// Whether `hwnd` is a TSF receiver window (the class registered by the TSF DLL). The DLL in turn
/// accepts commits only from the process it found or started as the service.
fn is_tsf_receiver(hwnd: HWND) -> bool {
    let mut name = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    len > 0 && String::from_utf16_lossy(&name[..len as usize]) == TSF_RECEIVER_CLASS
}
/// Owner process and thread of `receiver`, None when the window is gone.
fn receiver_owner(receiver: HWND) -> Option<HostOwner> {
    if unsafe { IsWindow(receiver) } == 0 {
        return None;
    }
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(receiver, &mut pid) };
    (pid != 0 && thread != 0).then_some(HostOwner { pid, thread })
}
/// Watches the process owning `receiver` so its session ends when it exits. Checked again on
/// every session start: a reused window handle from another process is watched anew.
fn watch_receiver(hwnd: HWND, state: &State, receiver: HWND) {
    let owner = receiver_owner(receiver);
    if state
        .watched_receivers
        .borrow_mut()
        .session_started(receiver as usize, owner)
        && let Some(owner) = owner
        && !watch_tsf_owner(receiver, owner.pid, hwnd)
    {
        service_log(&format!("exit watch failed to open host pid={}", owner.pid));
        state
            .watched_receivers
            .borrow_mut()
            .watch_failed(receiver as usize, owner.pid);
    }
}
/// Another application starts a session: end the current one (telling its DLL), drop its draft
/// (text never moves between applications) and serve `receiver` from now on. A translation still
/// in flight commits its Chinese into the application being left (kept until confirmed, see
/// [`commit_left_draft`]); the result is dropped.
unsafe fn switch_receiver(hwnd: HWND, state: &State, receiver: HWND) {
    unsafe {
        if yiime_core::left_field(state.pending.get()) == yiime_core::LeftField::CommitSource {
            let old = state.tsf_receiver.get();
            let raw = get_text(state.edit);
            let wire = single_line(&raw);
            if !old.is_null() && !wire.is_empty() {
                commit_left_draft(hwnd, state, old, raw, wire);
            }
            state.pending.set(false);
            state.generation.set(state.generation.get() + 1);
            service_log("session moved: translation source back to the previous application");
        }
        if IsWindowVisible(hwnd) != 0 {
            deactivate(hwnd, state);
        } else {
            finish_tsf(hwnd, state);
        }
        clear_box(hwnd, state);
    }
    state.finished.set(true);
    state.tsf_receiver.set(receiver);
    service_log("session moved to another application");
    watch_receiver(hwnd, state, receiver);
}
/// Commits the Chinese draft of a translation still in flight into the application being left
/// (`old`), with its own request id so the DLL confirms it. Until it is confirmed the draft stays
/// stashed under `old`: a rejected or unconfirmed write comes back in that application's box
/// (see [`restore_left_draft`]) instead of being lost.
unsafe fn commit_left_draft(hwnd: HWND, state: &State, old: HWND, raw: String, wire: String) {
    let id = state.next_request.get().wrapping_add(1).max(1);
    state.next_request.set(id);
    let reply =
        yiime_core::CommitReply::from_wire(unsafe { ipc::send_commit(old, hwnd, &wire, id) });
    service_log(&format!(
        "session moved: translation source back to the previous application ({reply:?})"
    ));
    if reply == yiime_core::CommitReply::Landed {
        return;
    }
    let awaiting_receipt = reply == yiime_core::CommitReply::Waiting;
    state.receipts.borrow_mut().push(AwaitingCommit {
        identity: yiime_core::CommitIdentity {
            receiver: old as usize,
            request: id,
            revision: 0,
        },
        history: None,
        started: std::time::Instant::now(),
        notified: false,
        awaiting_receipt,
        text: wire,
        stash: Some(raw),
        failed: reply == yiime_core::CommitReply::Rejected,
        same_app: false,
        host_pid: receiver_owner(old).map(|o| o.pid),
    });
    if awaiting_receipt {
        unsafe {
            SetTimer(hwnd, COMMIT_TIMER, 1000, None);
        }
    }
}
unsafe fn apply_restored_draft(
    hwnd: HWND,
    state: &State,
    index: usize,
    how: Option<yiime_core::Restore>,
    draft: String,
    same_app: bool,
) {
    unsafe {
        set_text(state.edit, &draft);
        let end = GetWindowTextLengthW(state.edit) as usize;
        SendMessageW(state.edit, EM_SETSEL, end, end as isize);
    }
    // Setting the text counts as an edit; the restored draft is the revision from now on.
    let revision = state.generation.get();
    let status = match how {
        Some(yiime_core::Restore::Unconfirmed) => {
            // Becomes an ordinary unconfirmed commit of this draft: Enter retries it under the
            // same id, Esc confirms it landed.
            let mut receipts = state.receipts.borrow_mut();
            let p = &mut receipts[index];
            p.stash = None;
            p.identity.revision = revision;
            UNCONFIRMED_STATUS.to_owned()
        }
        _ => {
            state.receipts.borrow_mut().remove(index);
            if same_app {
                "已恢复切换输入框前的草稿".to_owned()
            } else {
                "切换应用时这段文字没能上屏，草稿已恢复".to_owned()
            }
        }
    };
    service_log("draft of the left application restored");
    unsafe { set_status(hwnd, state, Some((status, true))) };
}
/// Auto-restores a stashed draft only for cross-app returns with an empty draft/composition and
/// not started by typing a key. For same-app parked drafts, shows a Ctrl+R hint instead.
unsafe fn restore_left_draft(hwnd: HWND, state: &State) {
    let receiver = state.tsf_receiver.get() as usize;
    if receiver == 0
        || unsafe { IsWindowVisible(hwnd) } == 0
        || state.pending.get()
        || state.recall_session.get()
    {
        return;
    }
    let draft_empty = unsafe { GetWindowTextLengthW(state.edit) } == 0;
    let composition_empty = state.composition.borrow().code.is_empty();
    let key_started = state.key_started.get();
    let found = state.receipts.borrow().iter().rposition(|p| {
        p.identity.receiver == receiver
            && p.stash.is_some()
            && yiime_core::can_auto_restore(p.same_app, draft_empty, composition_empty, key_started)
            && yiime_core::restore_left_draft(p.failed, p.notified, draft_empty).is_some()
    });
    if let Some(i) = found {
        let (how, draft, same_app) = {
            let receipts = state.receipts.borrow();
            let p = &receipts[i];
            let how = yiime_core::restore_left_draft(p.failed, p.notified, draft_empty);
            (how, p.stash.clone().unwrap_or_default(), p.same_app)
        };
        unsafe { apply_restored_draft(hwnd, state, i, how, draft, same_app) };
        return;
    }
    if draft_empty
        && composition_empty
        && !key_started
        && state.status.borrow().is_none()
        && state.receipts.borrow().iter().any(|p| {
            p.identity.receiver == receiver
                && p.same_app
                && p.stash.is_some()
                && yiime_core::restore_left_draft(p.failed, p.notified, draft_empty).is_some()
        })
    {
        unsafe {
            set_status(
                hwnd,
                state,
                Some(("切换输入框前有未上屏草稿，按 Ctrl+R 恢复".into(), false)),
            );
        }
    }
}
/// Manual Ctrl+R restore for a stashed draft (including same-app parked fields).
unsafe fn restore_left_draft_manual(hwnd: HWND, state: &State) {
    let receiver = state.tsf_receiver.get() as usize;
    if receiver == 0
        || unsafe { IsWindowVisible(hwnd) } == 0
        || state.pending.get()
        || state.recall_session.get()
        || !state.composition.borrow().code.is_empty()
        || unsafe { GetWindowTextLengthW(state.edit) } != 0
    {
        return;
    }
    let found = state.receipts.borrow().iter().rposition(|p| {
        p.identity.receiver == receiver
            && p.stash.is_some()
            && yiime_core::restore_left_draft(p.failed, p.notified, true).is_some()
    });
    let Some(i) = found else {
        return;
    };
    let (how, draft, same_app) = {
        let receipts = state.receipts.borrow();
        let p = &receipts[i];
        let how = yiime_core::restore_left_draft(p.failed, p.notified, true);
        (how, p.stash.clone().unwrap_or_default(), p.same_app)
    };
    unsafe { apply_restored_draft(hwnd, state, i, how, draft, same_app) };
}
/// Tells the TSF DLL this input ended without inserting text.
unsafe fn finish_tsf(hwnd: HWND, state: &State) -> bool {
    let receiver = state.tsf_receiver.get();
    if receiver.is_null() || state.finished.get() {
        return true;
    }
    let ok = unsafe { send_tsf_text(receiver, hwnd, "") };
    if ok {
        state.finished.set(true);
    }
    ok
}
/// Esc (not composing): discard the draft and close the box. TSF is told explicitly so the IME
/// is not left stuck in an edit.
unsafe fn cancel_session(hwnd: HWND, state: &State) {
    if outstanding(state) == Some(yiime_core::Outstanding::Unconfirmed) {
        // "已上屏": the user found the text in the field. Count it as landed.
        let receiver = state.tsf_receiver.get() as usize;
        let revision = state.generation.get();
        let receipt = {
            let mut receipts = state.receipts.borrow_mut();
            receipts
                .iter()
                .position(|p| p.stash.is_none() && p.identity.owns_draft(receiver, revision))
                .map(|i| receipts.remove(i))
        };
        if let Some(receipt) = receipt {
            service_log("unconfirmed commit: user says it landed");
            if let Some((source, english)) = receipt.history
                && !state.private_field.get()
            {
                remember_translation(&source, &english);
            }
        }
    }
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
/// TSF box: hides it (ending the session) when it is empty and does not have the keyboard, see
/// [`yiime_core::hide_empty_box`]. `focus_moved`: the host just reported a focus change.
unsafe fn hide_if_empty(hwnd: HWND, state: &State, focus_moved: bool) {
    let foreground = unsafe { GetForegroundWindow() };
    let settings = state.settings_window.get();
    let view = yiime_core::BoxView {
        visible: unsafe { IsWindowVisible(hwnd) } != 0,
        empty: state.composition.borrow().code.is_empty()
            && unsafe { GetWindowTextLengthW(state.edit) } == 0
            && state.status.borrow().is_none(),
        foreground: foreground == hwnd || (!settings.is_null() && foreground == settings),
        busy: state.pending.get(),
        shown_for_ms: state
            .shown_at
            .get()
            .map_or(u64::MAX, |at| at.elapsed().as_millis() as u64),
    };
    if yiime_core::hide_empty_box(view, focus_moved) {
        service_log(if focus_moved {
            "box hidden: empty, host focus moved"
        } else {
            "box hidden: empty, not in the foreground"
        });
        unsafe { deactivate(hwnd, state) };
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
    // A commit of this draft is outstanding: never translate it again.
    match yiime_core::resubmit(outstanding(state)) {
        yiime_core::Resubmit::Fresh => {}
        yiime_core::Resubmit::Wait => {
            unsafe { set_status(hwnd, state, Some(("正在等待上屏确认…".into(), false))) };
            return;
        }
        yiime_core::Resubmit::Retry => {
            unsafe { retry_commit(hwnd, state) };
            return;
        }
    }
    let draft = unsafe { get_text(state.edit) };
    if draft.trim().is_empty() {
        unsafe {
            cancel_session(hwnd, state);
        }
        return;
    }
    // A private field never goes to a translation service: its Chinese is committed as is.
    if chinese || state.private_field.get() || (plain_mode() && !state.recall_session.get()) {
        unsafe {
            commit_to_target(hwnd, state, &draft);
        }
        return;
    }
    let settings = current_settings(state);
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
    if outstanding(state).is_some() {
        return;
    }
    unsafe { send_to_target(hwnd, state, text, None) };
}
/// "未上屏，重试": sends the unconfirmed commit of this draft again, same text, same request id
/// (the DLL confirms an id that already landed instead of inserting it twice).
unsafe fn retry_commit(hwnd: HWND, state: &State) {
    let receiver = state.tsf_receiver.get() as usize;
    let revision = state.generation.get();
    let retry = state
        .receipts
        .borrow()
        .iter()
        .find(|p| p.stash.is_none() && p.notified && p.identity.owns_draft(receiver, revision))
        .map(|p| (p.identity.request, p.text.clone()));
    if let Some((id, text)) = retry {
        service_log("unconfirmed commit: user retries it");
        unsafe { send_to_target(hwnd, state, &text, Some(id)) };
    }
}
/// `retry`: request id of an unconfirmed commit of this draft being sent again.
unsafe fn send_to_target(hwnd: HWND, state: &State, text: &str, retry: Option<u32>) {
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
    } else if !state.tsf_receiver.get().is_null() {
        let receiver = state.tsf_receiver.get();
        let revision = state.generation.get();
        if let Some(id) = retry {
            if let Some(p) = state
                .receipts
                .borrow_mut()
                .iter_mut()
                .find(|p| p.stash.is_none() && p.identity.matches_reply(receiver as usize, id))
            {
                p.started = std::time::Instant::now();
                p.notified = false;
                p.awaiting_receipt = true;
            }
            unsafe {
                set_status(hwnd, state, None);
                SetTimer(hwnd, COMMIT_TIMER, 1000, None);
            }
            match yiime_core::CommitReply::from_wire(unsafe {
                ipc::send_commit(receiver, hwnd, &line, id)
            }) {
                yiime_core::CommitReply::Landed => unsafe {
                    finish_receipt(hwnd, state, receiver as usize, id, true)
                },
                yiime_core::CommitReply::Rejected => unsafe {
                    finish_receipt(hwnd, state, receiver as usize, id, false)
                },
                yiime_core::CommitReply::Waiting => {}
            }
            return;
        }
        let id = state.next_request.get().wrapping_add(1).max(1);
        state.next_request.set(id);
        let history = state
            .pending_history
            .borrow_mut()
            .take()
            .filter(|(source, english)| {
                english == text
                    && !state.private_field.get()
                    && source == &unsafe { get_text(state.edit) }
            });
        state.receipts.borrow_mut().push(AwaitingCommit {
            identity: yiime_core::CommitIdentity {
                receiver: receiver as usize,
                request: id,
                revision,
            },
            history,
            started: std::time::Instant::now(),
            notified: false,
            awaiting_receipt: true,
            text: line.clone(),
            stash: None,
            failed: false,
            same_app: false,
            host_pid: None,
        });
        unsafe {
            SetTimer(hwnd, COMMIT_TIMER, 1000, None);
        }
        match yiime_core::CommitReply::from_wire(unsafe {
            ipc::send_commit(receiver, hwnd, &line, id)
        }) {
            yiime_core::CommitReply::Landed => unsafe {
                finish_receipt(hwnd, state, receiver as usize, id, true)
            },
            yiime_core::CommitReply::Rejected => unsafe {
                finish_receipt(hwnd, state, receiver as usize, id, false)
            },
            // Queued or the transport timed out; the DLL posts the definitive result.
            yiime_core::CommitReply::Waiting => {}
        }
        return;
    } else if unsafe { send_unicode(&line) } {
        // UIPI may silently block SendInput (e.g. to an elevated window); that cannot be
        // detected here.
        Ok(())
    } else {
        Err("提交未完成，请检查原应用中的文字")
    };
    match result {
        Ok(()) => unsafe {
            record_confirmed_history(state, text);
            clear_box(hwnd, state)
        },
        Err(error) => unsafe {
            service_log(&format!("commit failed: {error}"));
            *state.status.borrow_mut() = Some((format!("{error}，草稿已保留"), true));
            show_box(hwnd, state);
        },
    }
}
unsafe fn finish_receipt(hwnd: HWND, state: &State, receiver: usize, id: u32, landed: bool) {
    let receipt = {
        let mut pending = state.receipts.borrow_mut();
        let Some(i) = pending
            .iter()
            .position(|p| p.identity.matches_reply(receiver, id))
        else {
            return;
        };
        if pending[i].stash.is_some() {
            // Chinese draft committed into an application the user left.
            if landed {
                pending.remove(i);
                if !pending.iter().any(|p| p.awaiting_receipt) {
                    unsafe { KillTimer(hwnd, COMMIT_TIMER) };
                }
            } else {
                service_log("previous application rejected the draft; kept for its next session");
                pending[i].failed = true;
                pending[i].awaiting_receipt = false;
                if !pending.iter().any(|p| p.awaiting_receipt) {
                    unsafe { KillTimer(hwnd, COMMIT_TIMER) };
                }
                drop(pending);
                unsafe { restore_left_draft(hwnd, state) };
            }
            return;
        }
        let removed = pending.remove(i);
        if !pending.iter().any(|p| p.awaiting_receipt) {
            unsafe { KillTimer(hwnd, COMMIT_TIMER) };
        }
        removed
    };
    if landed
        && let Some((source, english)) = receipt.history
        && !state.private_field.get()
    {
        remember_translation(&source, &english);
    }
    if !receipt
        .identity
        .owns_draft(state.tsf_receiver.get() as usize, state.generation.get())
    {
        return;
    }
    if landed {
        state.finished.set(true);
        unsafe {
            clear_box(hwnd, state);
            hide_box(hwnd, state);
        }
    } else {
        unsafe {
            set_status(hwnd, state, Some(("上屏失败，草稿已保留".into(), true)));
            show_box(hwnd, state);
        }
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
        // The snapshot was built with the user words of startup; bring it up to date.
        state.user_words.borrow_mut().invalidate();
        refresh_user_words(state);
    }
}
/// Reloads user_words.txt into the dictionaries in use when it changed since the last load (an
/// import from any settings window, or an edit in Notepad). Cheap when nothing changed (one
/// metadata call); runs when a session or a code starts.
fn refresh_user_words(state: &State) {
    let Some(contents) = state.user_words.borrow_mut().check_path(user_words_path()) else {
        return;
    };
    state.input.borrow().set_user_words(&contents);
    if let Some(full) = state.full_input.get()
        && !Arc::ptr_eq(full, &state.input.borrow())
    {
        full.set_user_words(&contents);
    }
}
fn record_confirmed_history(state: &State, text: &str) {
    if let Some((source, english)) = state.pending_history.borrow_mut().take()
        && !state.private_field.get()
        && text == english
        && source == unsafe { get_text(state.edit) }
    {
        remember_translation(&source, &english);
    }
}
unsafe fn on_translated(hwnd: HWND, state: &State, response: TranslationResult) {
    if response.generation != state.generation.get() || !state.pending.get() {
        return;
    }
    state.pending.set(false);
    if !state.composition.borrow().code.is_empty()
        || response.source != unsafe { get_text(state.edit) }
    {
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
            if !state.private_field.get() {
                *state.pending_history.borrow_mut() = Some((response.source, english.clone()));
            }
            commit_to_target(hwnd, state, &english);
        },
        Err(error) => unsafe {
            // The reason's category is in YiIME-translate.log; provider text is not logged.
            service_log("translation failed");
            set_status(hwnd, state, Some((format!("翻译失败：{error}"), true)));
        },
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // wParam of the TSF messages (and WM_COPYDATA's sender) is a window of the DLL's process,
    // which may be a 32-bit application: compare it in canonical form.
    let w = if matches!(
        msg,
        WM_COPYDATA
            | WM_TSF_ACTIVATE
            | WM_TSF_EXIT
            | WM_TSF_WATCH_FAILED
            | WM_TSF_FOCUS_MOVED
            | WM_TSF_ENGLISH
            | WM_TSF_COMMITTED
            | WM_TSF_COMMIT_FAILED
            | WM_TSF_PRIVATE
    ) {
        yiime_core::handle_from_wire(w as u64) as WPARAM
    } else {
        w
    };
    let state = unsafe { state_of(hwnd) };
    if let Some(state) = state
        && !state.tsf_service
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
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            if let Some(state) = state {
                let hdc = w as HDC;
                unsafe {
                    SetBkColor(hdc, BACKGROUND);
                    SetTextColor(hdc, INK);
                }
                state.bg_brush as LRESULT
            } else {
                unsafe { DefWindowProcW(hwnd, msg, w, l) }
            }
        }
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
        WM_DPICHANGED => {
            if let Some(state) = state {
                unsafe {
                    relayout(hwnd, state);
                }
            }
            0
        }
        WM_MOUSEACTIVATE => MA_ACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            if let Some(state) = state {
                let x = (l & 0xffff) as i16 as i32;
                let y = ((l >> 16) & 0xffff) as i16 as i32;
                let hit = {
                    let composition = state.composition.borrow();
                    let height = scaled_line_h(state);
                    composition.hits.iter().position(|&(left, right, line)| {
                        let top = composition.row_top + line * height;
                        y >= top && y < top + height && x >= left - 3 && x <= right + 3
                    })
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
                state.pending.set(false);
                state.generation.set(state.generation.get().wrapping_add(1));
                *status = None;
                drop(status);
                unsafe {
                    relayout(hwnd, state);
                }
            }
            0
        }
        WM_TSF_ACTIVATE => {
            // lParam = 1: history shortcut. 0: Ctrl+Alt+T / 译 mode with no first letter.
            // A–Z or a–z: first key of the session. Previously only uppercase was handled, so
            // an already-running service ignored Ctrl+Alt+T (lParam 0).
            if let Some(state) = state
                && !state.tsf_receiver.get().is_null()
                && l == 1
            {
                let anchor = state.pending_anchor.take();
                state.key_started.set(false);
                unsafe {
                    activate(hwnd, state, w as HWND, anchor, String::new());
                    recall_latest(hwnd, state);
                    notify_tsf_ready(state.tsf_receiver.get(), hwnd);
                }
            } else if let Some(state) = state
                && !state.tsf_receiver.get().is_null()
                && (l == 0
                    || (0x41..=0x5a).contains(&(l as usize))
                    || (0x61..=0x7a).contains(&(l as usize)))
            {
                let anchor = state.pending_anchor.take();
                state.key_started.set(l != 0);
                let code = if l == 0 {
                    String::new()
                } else {
                    (l as u8 as char).to_ascii_lowercase().to_string()
                };
                unsafe {
                    activate(hwnd, state, w as HWND, anchor, code);
                    if l == 0 {
                        restore_left_draft(hwnd, state);
                    }
                    notify_tsf_ready(state.tsf_receiver.get(), hwnd);
                }
            }
            0
        }
        WM_COPYDATA => unsafe {
            let Some(state) = state else {
                return 0;
            };
            if l == 0 || !state.tsf_service {
                return 0;
            }
            let packet = &*(l as *const COPYDATASTRUCT);
            // A session from another application's receiver starts with its caret packet
            // (dwData 3); everything else must come from the current session's receiver.
            if w as HWND != state.tsf_receiver.get() {
                if state.private || packet.dwData != 3 || !is_tsf_receiver(w as HWND) {
                    return 0;
                }
                switch_receiver(hwnd, state, w as HWND);
            }
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
                // During a session: the caret read asynchronously by the DLL; move the box there.
                3 if IsWindowVisible(hwnd) != 0 && !state.finished.get() => {
                    if let Some(rect) = (packet.cbData as usize == size_of::<RECT>()
                        && !packet.lpData.is_null())
                    .then(|| std::ptr::read_unaligned(packet.lpData as *const RECT))
                    .filter(valid_rect)
                    {
                        state.anchor.set(rect);
                        relayout(hwnd, state);
                    }
                    1
                }
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
                state.dict_retries.set(0);
                unsafe { KillTimer(hwnd, DICT_RETRY_TIMER) };
                adopt_full_input(state);
            }
            0
        }
        WM_FULL_INPUT_ERROR => {
            if let Some(state) = state {
                if w == 1 {
                    service_log("full dictionary missing on disk; keeping bundled dictionary");
                } else {
                    let retry = state.dict_retries.get();
                    if retry < MAX_DICT_RETRIES {
                        let next = retry + 1;
                        state.dict_retries.set(next);
                        let delay_ms = 2000u32 << retry;
                        service_log(&format!(
                            "full dictionary load failed transiently; scheduling retry {next}/{MAX_DICT_RETRIES} in {delay_ms} ms"
                        ));
                        unsafe { SetTimer(hwnd, DICT_RETRY_TIMER, delay_ms, None) };
                    } else {
                        service_log(
                            "full dictionary load failed after retries; keeping bundled dictionary",
                        );
                    }
                }
            }
            0
        }
        WM_TIMER if w == DICT_RETRY_TIMER => {
            unsafe { KillTimer(hwnd, DICT_RETRY_TIMER) };
            if let Some(state) = state
                && state.full_input.get().is_none()
                && let Some(path) = state.dict_words_path.borrow().clone()
            {
                let (_, user_words) = UserWordsReloader::load(user_words_path());
                dictionary::load_full_in_background(
                    path,
                    user_words,
                    state.tsf_service,
                    state.full_input.clone(),
                    hwnd as usize,
                    WM_FULL_INPUT,
                    WM_FULL_INPUT_ERROR,
                );
            }
            0
        }
        WM_IMPORT_DONE => {
            if let Some(state) = state {
                state.import_busy.set(false);
                let settings_hwnd = state.settings_window.get();
                if !settings_hwnd.is_null() {
                    unsafe {
                        let btn = GetDlgItem(settings_hwnd, SETTINGS_IMPORT);
                        if !btn.is_null() {
                            EnableWindow(btn, 1);
                        }
                    }
                }
                while let Ok(result) = state.import_rx.try_recv() {
                    refresh_user_words(state);
                    let text = match result {
                        Ok((added, total)) => format!(
                            "已导入 {added} 个新词（文件中共 {total} 个），立即生效（从下一次输入起）。"
                        ),
                        Err(error) => format!("导入失败：{error}"),
                    };
                    let parent = if !settings_hwnd.is_null() {
                        settings_hwnd
                    } else {
                        hwnd
                    };
                    unsafe { message(parent, &text) };
                }
            }
            0
        }
        WM_TSF_WATCH_FAILED => {
            if let Some(state) = state
                && w != 0
                && l != 0
            {
                service_log(&format!("exit watch failed for host pid={}", l as u32));
                state
                    .watched_receivers
                    .borrow_mut()
                    .watch_failed(w, l as u32);
            }
            0
        }
        // The application owning receiver `w` deactivated the IME (from its DLL, lParam 0) or
        // exited (from the exit watch, lParam = its process id): end its session. An exit notice
        // about an earlier owner of a reused handle is ignored BEFORE touching stashes or session.
        WM_TSF_EXIT => {
            if let Some(state) = state
                && w != 0
            {
                let exited = l == 0
                    || state
                        .watched_receivers
                        .borrow_mut()
                        .host_exited(w, l as u32);
                if exited {
                    if l != 0 {
                        let pid = l as u32;
                        state.receipts.borrow_mut().retain(|p| {
                            p.stash.is_none()
                                || p.identity.receiver != w
                                || (p.host_pid.is_some() && p.host_pid != Some(pid))
                        });
                    }
                    if w as HWND == state.tsf_receiver.get() {
                        service_log(if l == 0 {
                            "session end: IME deactivated in the host"
                        } else {
                            "session end: host exited"
                        });
                        state.finished.set(true);
                        state.tsf_receiver.set(null_mut());
                        unsafe {
                            clear_box(hwnd, state);
                            hide_box(hwnd, state);
                            if state.private {
                                DestroyWindow(hwnd);
                            }
                        }
                    }
                }
            }
            0
        }
        WM_TIMER if w == EMPTY_BOX_TIMER => {
            if let Some(state) = state {
                unsafe { hide_if_empty(hwnd, state, false) };
            }
            0
        }
        WM_TSF_ENGLISH => {
            if let Some(state) = state
                && state.tsf_service
                && w != 0
                && w as HWND == state.tsf_receiver.get()
                && yiime_core::english_ends_session(yiime_core::BoxView {
                    visible: unsafe { IsWindowVisible(hwnd) } != 0,
                    busy: state.pending.get(),
                    ..Default::default()
                })
            {
                // Hide and end the session (a draft is kept for next time, as when clicking
                // away). While a translation is in flight the box stays so the result lands.
                service_log("session end: host switched to English");
                unsafe { deactivate(hwnd, state) };
            }
            0
        }
        WM_TSF_PRIVATE => {
            if let Some(state) = state
                && state.tsf_service
                && w != 0
                && w as HWND == state.tsf_receiver.get()
                && state.private_field.replace(l != 0) != (l != 0)
            {
                if l != 0 {
                    for receipt in state
                        .receipts
                        .borrow_mut()
                        .iter_mut()
                        .filter(|p| p.identity.receiver == w)
                    {
                        receipt.history = None;
                    }
                }
                service_log(if l != 0 {
                    "session: private field (no learning, history or translation)"
                } else {
                    "session: normal field"
                });
            }
            0
        }
        WM_TSF_COMMITTED | WM_TSF_COMMIT_FAILED => {
            if let Some(state) = state
                && state.tsf_service
            {
                unsafe {
                    finish_receipt(hwnd, state, w, l as u32, msg == WM_TSF_COMMITTED);
                }
            }
            0
        }
        WM_TIMER if w == COMMIT_TIMER => {
            if let Some(state) = state {
                let warn = {
                    let mut pending = state.receipts.borrow_mut();
                    let mut warn = false;
                    for p in pending.iter_mut() {
                        let elapsed = p.started.elapsed().as_secs();
                        if !p.notified && elapsed >= yiime_core::RECEIPT_WARN_SECS {
                            p.notified = true;
                            warn |= p.stash.is_none()
                                && p.identity.owns_draft(
                                    state.tsf_receiver.get() as usize,
                                    state.generation.get(),
                                );
                        }
                        if p.awaiting_receipt && elapsed >= yiime_core::RECEIPT_GIVEUP_SECS {
                            p.awaiting_receipt = false;
                        }
                    }
                    // Stashed drafts survive receipt timeout; only non-stashed timed-out entries
                    // are dropped once receipt waiting ends.
                    pending.retain(|p| {
                        yiime_core::retain_receipt(p.awaiting_receipt, p.stash.is_some())
                    });
                    if !pending.iter().any(|p| p.awaiting_receipt) {
                        unsafe {
                            KillTimer(hwnd, COMMIT_TIMER);
                        }
                    }
                    warn
                };
                if warn {
                    unsafe {
                        set_status(hwnd, state, Some((UNCONFIRMED_STATUS.into(), true)));
                        show_box(hwnd, state);
                    }
                }
                unsafe { restore_left_draft(hwnd, state) };
            }
            0
        }
        WM_TSF_FOCUS_MOVED => {
            if let Some(state) = state
                && state.tsf_service
                && w != 0
                && l != 0
                && w as HWND != state.tsf_receiver.get()
                && is_tsf_receiver(w as HWND)
            {
                // A parked field of an application this box no longer serves: nothing to commit,
                // but release the id, or that application's next commit would go to the old field.
                service_log("session end: parked field of another application released");
                unsafe { send_tsf_held(w as HWND, hwnd, l as u64, 0, "") };
            } else if let Some(state) = state
                && state.tsf_service
                && w != 0
                && w as HWND == state.tsf_receiver.get()
            {
                if l != 0 {
                    // The DLL parked this text field (lParam is that id) and may already have
                    // opened the next session. Commit a translation's Chinese there and drop the
                    // draft either way, so neither follows the new field. An empty commit would
                    // end the next session, so the DLL is not told with one.
                    let translating = yiime_core::left_field(state.pending.get())
                        == yiime_core::LeftField::CommitSource;
                    let raw = if translating {
                        unsafe { get_text(state.edit) }
                    } else {
                        String::new()
                    };
                    let wire = single_line(&raw);
                    service_log(if translating {
                        "session end: translation source back to the previous field"
                    } else {
                        "session end: host focus moved to another field"
                    });
                    let receiver = state.tsf_receiver.get();
                    let id = if translating && !wire.is_empty() {
                        let id = state.next_request.get().wrapping_add(1).max(1);
                        state.next_request.set(id);
                        id
                    } else {
                        0
                    };
                    // Always: an empty packet releases the parked field, so a later commit in the
                    // new session is not inserted into the old one.
                    let reply = yiime_core::CommitReply::from_wire(unsafe {
                        send_tsf_held(receiver, hwnd, l as u64, id, &wire)
                    });
                    if translating && !wire.is_empty() && reply != yiime_core::CommitReply::Landed {
                        service_log(&format!("previous field parked draft ({reply:?})"));
                        let awaiting_receipt = reply == yiime_core::CommitReply::Waiting;
                        state.receipts.borrow_mut().push(AwaitingCommit {
                            identity: yiime_core::CommitIdentity {
                                receiver: receiver as usize,
                                request: id,
                                revision: 0,
                            },
                            history: None,
                            started: std::time::Instant::now(),
                            notified: false,
                            awaiting_receipt,
                            text: wire,
                            stash: Some(raw),
                            failed: reply == yiime_core::CommitReply::Rejected,
                            same_app: true,
                            host_pid: receiver_owner(receiver).map(|o| o.pid),
                        });
                        if awaiting_receipt {
                            unsafe {
                                SetTimer(hwnd, COMMIT_TIMER, 1000, None);
                            }
                        }
                    }
                    state.pending.set(false);
                    state.finished.set(true);
                    unsafe {
                        clear_box(hwnd, state);
                        hide_box(hwnd, state);
                    }
                } else {
                    unsafe { hide_if_empty(hwnd, state, true) };
                }
            }
            0
        }
        WM_TIMER if w == HEARTBEAT_TIMER => {
            heartbeat();
            0
        }
        WM_TIMER if w == IDLE_TIMER => {
            if let Some(state) = state
                && unsafe { IsWindowVisible(hwnd) } == 0
                && state.settings_window.get().is_null()
                && !history::is_history_open()
                // A draft kept for an application left mid-translation would be lost.
                && !state.receipts.borrow().iter().any(|p| p.stash.is_some())
            {
                unsafe { DestroyWindow(hwnd) };
            }
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
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
                if ptr.is_null() || !(*ptr).tsf_service {
                    let data = tray_data(hwnd);
                    Shell_NotifyIconW(NIM_DELETE, &data);
                }
                if !ptr.is_null() {
                    if !(*ptr).tsf_receiver.get().is_null() && !(*ptr).finished.get() {
                        send_tsf_text((*ptr).tsf_receiver.get(), hwnd, "");
                    }
                    let settings = (*ptr).settings_window.get();
                    if !settings.is_null() {
                        DestroyWindow(settings);
                    }
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let state = Box::from_raw(ptr);
                    let settings_font = state.settings_font.get();
                    if !settings_font.is_null() {
                        DeleteObject(settings_font as _);
                    }
                    DeleteObject(state.font.get() as _);
                    DeleteObject(state.small_font.get() as _);
                    DeleteObject(state.bg_brush as _);
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
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(null_mut(), |value| {
                yiime_core::handle_from_wire(value) as HWND
            })
    };
    let tsf_receiver = handle("--tsf-hwnd");
    let target = handle("--target-hwnd");
    let input = MixedInput::bundled();
    // User words: %APPDATA%\\YiIME\\user_words.txt, `word code` per line, ranked
    // first under that code.
    // Reloaded in place when the file changes (an import, or an edit in Notepad), so the resident
    // service shows new words without a restart.
    let self_test = SELF_TEST.load(Ordering::Relaxed);
    let (user_words_reloader, user_words) =
        UserWordsReloader::load(if self_test { None } else { user_words_path() });
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
        // Same 14/11 YaHei sizes as the inline candidate bar (document-like, compact).
        let font = create_font(14);
        let small_font = create_font(11);
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
        let (import_tx, import_rx) = std::sync::mpsc::channel();
        let state = Box::new(State {
            input: RefCell::new(input),
            full_input,
            edit,
            font: Cell::new(font),
            small_font: Cell::new(small_font),
            bg_brush: CreateSolidBrush(BACKGROUND),
            font_dpi: Cell::new(96),
            draft_h: Cell::new(EDIT_H),
            composition: RefCell::new(Composition::default()),
            status: RefCell::new(None),
            settings_stamp: Cell::new(
                settings_dir().map(|dir| yiime_core::SettingsStamp::of(&dir)),
            ),
            settings: RefCell::new(load_settings()),
            settings_window: Cell::new(null_mut()),
            dialog_base: RefCell::new(None),
            dialog_keys_unreadable: Cell::new(false),
            settings_font: Cell::new(null_mut()),
            settings_dpi: Cell::new(0),
            settings_min: Cell::new((0, 0)),
            settings_scroll: Cell::new(0),
            anchor: Cell::new(caret_of(target)),
            pending_anchor: Cell::new(None),
            target: Cell::new(target),
            generation: Cell::new(0),
            pending: Cell::new(false),
            translation_tx,
            translation_rx,
            suppress_deactivate: Cell::new(false),
            tsf_receiver: Cell::new(tsf_receiver),
            tsf_service: !tsf_receiver.is_null(),
            private: !tsf_receiver.is_null() && std::env::args().any(|arg| arg == "--private"),
            watched_receivers: RefCell::new(WatchedHosts::default()),
            user_words: RefCell::new(user_words_reloader),
            shown_at: Cell::new(None),
            finished: Cell::new(false),
            pending_history: RefCell::new(None),
            receipts: RefCell::new(Vec::new()),
            next_request: Cell::new(first_request_id()),
            private_field: Cell::new(false),
            swallow_char: Cell::new(false),
            last_plain: Cell::new(plain_mode()),
            learned: RefCell::new(if self_test {
                Learned::default()
            } else {
                Learned::load(learned_path(), Box::new(Dpapi))
            }),
            quotes: Cell::new(Quotes::default()),
            english: Cell::new(false),
            shift_tap: Cell::new(yiime_core::ShiftTap::default()),
            numpad: Cell::new(false),
            key_started: Cell::new(false),
            import_tx,
            import_rx,
            import_busy: Cell::new(false),
            dict_retries: Cell::new(0),
            dict_words_path: RefCell::new(words_path.clone()),
            recall_session: Cell::new(false),
            history_pos: Cell::new(None),
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
        });
        let standalone = tsf_receiver.is_null();
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        if let Some(path) = words_path.filter(|_| !self_test) {
            dictionary::load_full_in_background(
                path,
                user_words,
                warm,
                full_ready,
                hwnd as usize,
                WM_FULL_INPUT,
                WM_FULL_INPUT_ERROR,
            );
        }
        if self_test {
            return 0;
        }
        if standalone {
            add_tray(
                hwnd,
                // Show the balloon only on a first launch without arguments (not for
                // --settings/--history).
                (std::env::args().count() == 1).then_some(
                    "用 Win+空格 切换到“YiIME 译（五笔）”后直接打字即可。单击或右键托盘图标可打开菜单。",
                ),
            );
            SetTimer(hwnd, TRAY_TIMER, 2000, None);
        }
    }
    0
}
/// What leaves the machine and what is stored locally, shown once per user on the first start of
/// the tray/settings program (details in SECURITY.md).
const PRIVACY_NOTICE: &str = "YiIME 译 的隐私说明（只显示一次）：

• 翻译：按 Enter 时，草稿里的中文会发送到你在设置里选择的翻译服务（默认 Azure 翻译；未填 Azure 密钥时改用 Google 免费翻译接口）。Google Cloud、DeepL、大模型需要你自己填写密钥。打字、选字本身不联网。
• 备用服务：默认关闭。只有勾选“失败时允许改用其他翻译服务”后，失败时才会把同一段文字发给另一个服务。
• 输入记忆：默认开启，在本机保存翻译过的中文和译文（最多 500 条，Windows DPAPI 加密，%APPDATA%\\YiIME\\history.dat），可在设置中关闭并清除。
• 其他本机文件：API 密钥（DPAPI 加密）、自学习记录 learned.dat（默认关，加密）、用户词库 user_words.txt（明文），以及只含诊断事件、不含输入内容的 %TEMP%\\YiIME-tsf.log、YiIME-translate.log。

完整说明见安装目录或项目主页的 SECURITY.md。";
fn show_privacy_notice_once(hwnd: HWND) {
    if SELF_TEST.load(Ordering::Relaxed) || OPTIONS.0.get_u32("PrivacyNoticeShown") == Some(1) {
        return;
    }
    unsafe {
        MessageBoxW(
            hwnd,
            wide(PRIVACY_NOTICE).as_ptr(),
            wide(APP_NAME).as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
    OPTIONS.0.set_u32("PrivacyNoticeShown", 1);
}
/// Types scripted keys into a hidden input box through the real key handler and checks the draft.
/// Returns the process exit code (0 = passed).
unsafe fn self_test(hwnd: HWND) -> i32 {
    let Some(state) = (unsafe { state_of(hwnd) }) else {
        return 2;
    };
    let mut failures = 0;
    let mut check = |keys: &str, expected: &str| {
        unsafe { set_text(state.edit, "") };
        for ch in keys.chars() {
            unsafe { handle_char(hwnd, state, ch as u32) };
        }
        let draft = unsafe { get_text(state.edit) };
        if draft != expected {
            failures += 1;
            eprintln!("YiIME self-test: {keys:?} gave {draft:?}, expected {expected:?}");
        }
    };
    // Pinyin + space, then Wubi; Esc clears a code; Backspace edits it.
    check("nihao ", "你好");
    check("nihao\u{1b}", "");
    check("nihaox\u{8} ", "你好");
    check("wqvb ", "你好");
    check("nihao,", "你好，");
    check("nihao1", "你好");
    // Learning is off by default, so nothing is written.
    failures
}
pub fn run() {
    // qingjian server `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` and weasel
    // WeaselServer / WeaselPanel: the EXE must use the same physical screen coordinates as a
    // Per-Monitor V2 host (Grok Bot, Chromium) or SetWindowPos treats a TSF caret as logical
    // pixels and the box lands far from the insertion point.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().collect();
    // Test of the watchdog: a main thread that hangs for 10 s must end the process with
    // WATCHDOG_EXIT after about 2 s.
    if args.iter().any(|arg| arg == "--watchdog-test") {
        start_watchdog(
            null_mut(),
            yiime_core::Watchdog {
                limit_ms: 2000,
                grace_ms: 0,
            },
        );
        heartbeat();
        std::thread::sleep(std::time::Duration::from_secs(10));
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--self-test") {
        SELF_TEST.store(true, Ordering::Relaxed);
    } else {
        migrate_settings();
    }
    let tsf_mode = args.iter().any(|arg| arg == "--tsf-hwnd");
    let wants_settings = args.iter().any(|arg| arg == "--settings");
    let wants_history = args.iter().any(|arg| arg == "--history");
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = wide(yiime_core::APP_WINDOW_CLASS);
        let smoke_test = SELF_TEST.load(Ordering::Relaxed);
        let tray_mutex = if !tsf_mode && !smoke_test {
            ensure_tip_in_language_list();
            let mutex = CreateMutexW(null(), 0, wide(r"Local\YiIME-TrayStart-1").as_ptr());
            if !mutex.is_null() {
                WaitForSingleObject(mutex, 5000);
            }
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
                if !mutex.is_null() {
                    ReleaseMutex(mutex);
                    CloseHandle(mutex);
                }
                return;
            }
            mutex
        } else {
            null_mut()
        };
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
        let history_class = wide(HISTORY_CLASS);
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
        let settings_class = wide(SETTINGS_CLASS);
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
            wide(&if tsf_mode {
                // One shared service per integrity level; a private box has its own title so no
                // other application's DLL finds it.
                yiime_core::service_window_title(
                    crate::process_security::process_security(0)
                        .map_or(yiime_core::MEDIUM_INTEGRITY, |own| own.integrity),
                    args.iter().any(|arg| arg == "--private"),
                )
            } else {
                TRAY_TITLE.to_owned()
            })
            .as_ptr(),
            WS_POPUP,
            0,
            0,
            MIN_W,
            80,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if !tray_mutex.is_null() {
            ReleaseMutex(tray_mutex);
            CloseHandle(tray_mutex);
        }
        if smoke_test {
            let code = self_test(hwnd);
            DestroyWindow(hwnd);
            std::process::exit(code);
        }
        let Some(state) = state_of(hwnd) else {
            return;
        };
        let argument = |name: &str| {
            args.windows(2)
                .find(|pair| pair[0] == name)
                .map(|pair| pair[1].as_str())
        };
        if !state.tsf_service {
            show_privacy_notice_once(hwnd);
        }
        let receiver = state.tsf_receiver.get();
        if !receiver.is_null() && IsWindow(receiver) == 0 {
            // The application that asked for this service is already gone: just stay resident
            // (a private box has nobody else to serve and exits).
            state.tsf_receiver.set(null_mut());
            state.finished.set(true);
            if state.private {
                DestroyWindow(hwnd);
            } else {
                SetTimer(hwnd, IDLE_TIMER, IDLE_EXIT_MS, None);
            }
        } else if !receiver.is_null() {
            watch_receiver(hwnd, state, receiver);
            let initial: String = argument("--initial")
                .unwrap_or("")
                .chars()
                .filter(char::is_ascii_alphabetic)
                .map(|ch| ch.to_ascii_lowercase())
                .take(MAX_CODE_LEN)
                .collect();
            let anchor = argument("--caret").and_then(parse_rect);
            state.key_started.set(!initial.is_empty());
            activate(hwnd, state, state.target.get(), anchor, initial);
            if args.iter().any(|arg| arg == "--recall") {
                recall_latest(hwnd, state);
            }
            notify_tsf_ready(receiver, hwnd);
        } else if wants_history {
            open_history();
        } else if wants_settings || current_settings(state).validate().is_err() {
            open_settings(hwnd);
        }
        if state.tsf_service && !SELF_TEST.load(Ordering::Relaxed) {
            start_watchdog(hwnd, yiime_core::Watchdog::default());
        }
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            heartbeat();
            let settings = state_of(hwnd).map_or(null_mut(), |state| state.settings_window.get());
            if !settings.is_null() && IsDialogMessageW(settings, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
