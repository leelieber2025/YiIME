#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("此界面仅支持 Windows");
}

/// Floating input box at the text cursor: compose a Chinese draft with Wubi/pinyin, press Enter to
/// translate and send the English back to the original application. Settings open in a separate
/// window from the tray menu.
#[cfg(windows)]
mod app {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, ClientToScreen,
        CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DT_CALCRECT,
        DT_EDITCONTROL, DT_NOPREFIX, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FillRect,
        GetDC, GetMonitorInfoW, GetStockObject, GetTextExtentPoint32W, GetTextMetricsW, HDC, HFONT,
        InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromPoint,
        OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
        TEXTMETRICW, TRANSPARENT, TextOutW, WHITE_BRUSH,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleHandleW, GetProcAddress, LoadLibraryW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
        RegSetKeyValueW,
    };
    use windows_sys::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Controls::{
        EM_GETSEL, EM_LIMITTEXT, EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL,
    };
    use windows_sys::Win32::UI::Input::Ime::ImmAssociateContextEx;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
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
        HistoryEntry, Hotkey, LEARNED_FILE, Learned, LetterAction, LetterRules, MAX_CODE_LEN,
        MixedInput, OptionStore, Options, Quotes, Sealer, TranslationStyle, USER_WORDS_FILE,
        classify_letter, history_label, parse_hotkey, parse_page_size, selection_index,
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
    const OPTION_BOXES: [(i32, &str, Flag); 10] = [
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
    ];
    /// Each extra row of switches moves the controls below it down.
    const OPTION_SHIFT: i32 = (OPTION_BOXES.len() as i32 + 1) / 2 * 26 - 3 * 26;
    const ID_OK: i32 = 1;
    const ID_CANCEL: i32 = 2;
    const TRAY_SETTINGS: usize = 1;
    const TRAY_EXIT: usize = 2;
    const TRAY_PLAIN: usize = 3;
    const TRAY_TRANSLATE: usize = 5;
    /// The tray instance polls the mode (also changed by IME shortcuts and the taskbar button) to
    /// refresh its tooltip.
    const TRAY_TIMER: usize = 7;
    /// Registry key shared with the TSF DLL.
    const STATE_KEY: &str = "Software\\InputTranslation";
    const WM_TRANSLATED: u32 = WM_APP + 1;
    const WM_TSF_ACTIVATE: u32 = WM_APP + 2;
    const WM_TSF_EXIT: u32 = WM_APP + 3;
    const WM_TRAY: u32 = WM_APP + 4;
    /// Asks the running tray instance to open settings (`--settings`, or a second launch).
    const WM_OPEN_SETTINGS: u32 = WM_APP + 5;
    /// Asks the running tray instance to open input history (`--history`).
    const WM_OPEN_HISTORY: u32 = WM_APP + 6;
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
    /// Height of the Translation Engine row at the top of the settings window; the other controls
    /// move down by this much.
    const ENGINE_SHIFT: i32 = 112;
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
        input: std::sync::Arc<MixedInput>,
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
    fn watch_tsf_owner(receiver: HWND, editor: HWND) {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(receiver, &mut pid);
        }
        if pid == 0 {
            return;
        }
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if process.is_null() {
            return;
        }
        let process = process as usize;
        let editor = editor as usize;
        std::thread::spawn(move || unsafe {
            WaitForSingleObject(process as _, u32::MAX);
            CloseHandle(process as _);
            PostMessageW(editor as HWND, WM_TSF_EXIT, 0, 0);
        });
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
            let Some(function) = GetProcAddress(library, c"InstallLayoutOrTip".as_ptr().cast())
            else {
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
    /// Reads the large lexicon next to the EXE (installed) or in data\\frost (portable); None if
    /// absent.
    fn frequency_words() -> Option<String> {
        let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        [
            dir.join("InputTranslation-words.txt"),
            dir.join("data")
                .join("frost")
                .join("InputTranslation-words.txt"),
        ]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
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
    fn tray_tip() -> String {
        if plain_mode() {
            format!(
                "{APP_NAME} {} · 普通五笔模式（不翻译）",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            format!(
                "{APP_NAME} {} · 翻译模式（单击或右键：菜单）",
                env!("CARGO_PKG_VERSION")
            )
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
    unsafe fn update_tray_tip(hwnd: HWND) {
        let mut data = unsafe { tray_data(hwnd) };
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        copy_wide(&mut data.szTip, &tray_tip());
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
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
    unsafe fn send_tsf_text(receiver: HWND, sender: HWND, text: &str) -> bool {
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        let packet = COPYDATASTRUCT {
            dwData: 0,
            cbData: (utf16.len() * 2) as u32,
            lpData: utf16.as_mut_ptr() as _,
        };
        let mut accepted = 0usize;
        unsafe {
            SendMessageTimeoutW(
                receiver,
                WM_COPYDATA,
                sender as usize,
                &packet as *const _ as isize,
                SMTO_ABORTIFHUNG,
                3000,
                &mut accepted,
            ) != 0
                && accepted != 0
        }
    }
    unsafe fn notify_tsf_ready(receiver: HWND, sender: HWND) {
        let packet = COPYDATASTRUCT {
            dwData: 1,
            cbData: 0,
            lpData: null_mut(),
        };
        for _ in 0..20 {
            let mut accepted = 0usize;
            if unsafe {
                SendMessageTimeoutW(
                    receiver,
                    WM_COPYDATA,
                    sender as usize,
                    &packet as *const _ as isize,
                    SMTO_ABORTIFHUNG,
                    200,
                    &mut accepted,
                )
            } != 0
                && accepted != 0
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
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
    unsafe fn create_font(height: i32) -> HFONT {
        unsafe {
            CreateFontW(
                -height,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as _,
                OUT_DEFAULT_PRECIS as _,
                CLIP_DEFAULT_PRECIS as _,
                CLEARTYPE_QUALITY as _,
                0,
                wide("Microsoft YaHei UI").as_ptr(),
            )
        }
    }
    unsafe fn text_width(hdc: HDC, text: &str) -> i32 {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE { cx: 0, cy: 0 };
        unsafe {
            GetTextExtentPoint32W(hdc, units.as_ptr(), units.len() as i32, &mut size);
        }
        size.cx
    }
    unsafe fn draw_text(hdc: HDC, x: i32, y: i32, color: u32, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            SetTextColor(hdc, color);
            TextOutW(hdc, x, y, units.as_ptr(), units.len() as i32);
        }
    }
    /// Below the cursor, or above it if it does not fit; always inside the monitor's work area.
    unsafe fn place(anchor: RECT, width: i32, height: i32) -> (i32, i32) {
        let work = unsafe { work_area(anchor) };
        let mut y = anchor.bottom + 4;
        if y + height > work.bottom {
            y = anchor.top - height - 4;
        }
        let y = y.min(work.bottom - height).max(work.top);
        let x = anchor.left.min(work.right - width).max(work.left);
        (x, y)
    }
    /// Work area of the monitor containing the cursor.
    unsafe fn work_area(anchor: RECT) -> RECT {
        let point = POINT {
            x: anchor.left,
            y: anchor.bottom,
        };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        unsafe {
            if GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &mut info) != 0 {
                info.rcWork
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: GetSystemMetrics(SM_CXSCREEN),
                    bottom: GetSystemMetrics(SM_CYSCREEN),
                }
            }
        }
    }
    /// Resizes and repositions the box for the draft, code and candidates.
    unsafe fn relayout(hwnd: HWND, state: &State) {
        let draft = unsafe { get_text(state.edit) };
        let status = state.status.borrow().clone();
        let mut composition = state.composition.borrow_mut();
        let composing = !composition.code.is_empty();
        let hdc = unsafe { GetDC(hwnd) };
        let old = unsafe { SelectObject(hdc, state.font as _) };
        let mut width = (unsafe { text_width(hdc, &draft) } + 24).min(DRAFT_MAX_W) + 2 * PAD;
        composition.hits.clear();
        if composing {
            let mut x = PAD;
            let labels: Vec<(String, String)> = composition
                .page_items()
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    (
                        format!("{}.{}", index + 1, candidate.text),
                        composition.hint(candidate),
                    )
                })
                .collect();
            for (label, hint) in labels {
                let mut w = unsafe { text_width(hdc, &label) };
                if !hint.is_empty() {
                    unsafe {
                        SelectObject(hdc, state.small_font as _);
                        w += text_width(hdc, &hint) + 1;
                        SelectObject(hdc, state.font as _);
                    }
                }
                composition.hits.push((x, x + w));
                x += w + GAP;
            }
            width = width.max(x + PAD);
            let code = unsafe { text_width(hdc, &composition.code) };
            unsafe {
                SelectObject(hdc, state.small_font as _);
            }
            let pages = unsafe { text_width(hdc, &composition.page_label()) };
            width = width.max(code + pages + 3 * PAD + GAP);
        } else {
            unsafe {
                SelectObject(hdc, state.small_font as _);
            }
            let line = status.map_or(hint(state).to_owned(), |(text, _)| text);
            width = width.max(unsafe { text_width(hdc, &line) } + 2 * PAD);
        }
        // Wubi hints can make the row wider than MAX_W: allow up to the work-area width so the 8th
        // and 9th candidates are not cut off.
        let max_w = unsafe {
            let work = work_area(state.anchor.get());
            (work.right - work.left - 16).max(MAX_W)
        };
        let width = width.clamp(MIN_W, max_w) + 2;
        // Multi-line draft: wrap at the edit-box width to compute its height (excluding the edit
        // control's padding).
        let edit_w = width - 2 - 2 * PAD;
        let draft_h = unsafe {
            SelectObject(hdc, state.font as _);
            let mut metrics: TEXTMETRICW = std::mem::zeroed();
            GetTextMetricsW(hdc, &mut metrics);
            let line = metrics.tmHeight.max(1);
            let mut units: Vec<u16> = draft.encode_utf16().collect();
            if units.is_empty() {
                units.push(' ' as u16);
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: edit_w - 10,
                bottom: 0,
            };
            DrawTextW(
                hdc,
                units.as_mut_ptr(),
                units.len() as i32,
                &mut rect,
                DT_CALCRECT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
            );
            let lines = ((rect.bottom - rect.top + line - 1) / line).clamp(1, DRAFT_MAX_LINES);
            (lines * line + 4).max(EDIT_H)
        };
        unsafe {
            SelectObject(hdc, old);
            ReleaseDC(hwnd, hdc);
        }
        state.draft_h.set(draft_h);
        let lines = if composing { 2 } else { 1 };
        let height = PAD + draft_h + 3 + lines * LINE_H + PAD + 2;
        composition.row_top = PAD + draft_h + 3 + LINE_H;
        drop(composition);
        let (x, y) = unsafe { place(state.anchor.get(), width, height) };
        unsafe {
            SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
            MoveWindow(state.edit, PAD, PAD, edit_w, draft_h, 1);
            // Keep the caret visible (scroll to it when the draft exceeds the maximum lines).
            SendMessageW(state.edit, EM_SCROLLCARET, 0, 0);
            InvalidateRect(hwnd, null(), 1);
        }
    }
    unsafe fn paint(hwnd: HWND, state: &State) {
        let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        let mut client = RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut client);
            FillRect(hdc, &client, GetStockObject(WHITE_BRUSH) as _);
            let separator = CreateSolidBrush(SEPARATOR);
            let line = RECT {
                left: PAD,
                top: PAD + state.draft_h.get() + 1,
                right: client.right - PAD,
                bottom: PAD + state.draft_h.get() + 2,
            };
            FillRect(hdc, &line, separator);
            DeleteObject(separator as _);
            SetBkMode(hdc, TRANSPARENT as _);
        }
        let top = PAD + state.draft_h.get() + 3;
        let composition = state.composition.borrow();
        let old = unsafe { SelectObject(hdc, state.font as _) };
        if !composition.code.is_empty() {
            unsafe {
                draw_text(hdc, PAD, top + 2, BLUE, &composition.code);
            }
            let row = composition.row_top;
            for (index, (candidate, (left, right))) in composition
                .page_items()
                .iter()
                .zip(composition.hits.iter().copied())
                .enumerate()
            {
                if index == 0 {
                    let highlight = unsafe { CreateSolidBrush(HIGHLIGHT) };
                    let rect = RECT {
                        left: left - 3,
                        top: row,
                        right: right + 3,
                        bottom: row + LINE_H - 2,
                    };
                    unsafe {
                        FillRect(hdc, &rect, highlight);
                        DeleteObject(highlight as _);
                    }
                }
                let color = if candidate.complete { INK } else { GRAY };
                let label = format!("{}.{}", index + 1, candidate.text);
                unsafe {
                    draw_text(hdc, left, row + 2, color, &label);
                }
                let hint = composition.hint(candidate);
                if !hint.is_empty() {
                    unsafe {
                        let x = left + text_width(hdc, &label) + 1;
                        let previous = SelectObject(hdc, state.small_font as _);
                        draw_text(hdc, x, row + 6, GRAY, &hint);
                        SelectObject(hdc, previous);
                    }
                }
            }
            if composition.page_count() > 1 {
                unsafe {
                    SelectObject(hdc, state.small_font as _);
                    let label = composition.page_label();
                    let w = text_width(hdc, &label);
                    draw_text(hdc, client.right - PAD - w, top + 5, GRAY, &label);
                }
            }
        } else {
            let status = state.status.borrow();
            let (text, color) = match status.as_ref() {
                Some((text, true)) => (text.as_str(), RED),
                Some((text, false)) => (text.as_str(), BLUE),
                None => (hint(state), GRAY),
            };
            unsafe {
                SelectObject(hdc, state.small_font as _);
                draw_text(hdc, PAD, top + 5, color, text);
            }
        }
        unsafe {
            SelectObject(hdc, old);
            EndPaint(hwnd, &paint);
        }
    }
    unsafe fn set_status(hwnd: HWND, state: &State, status: Option<(String, bool)>) {
        *state.status.borrow_mut() = status;
        unsafe {
            relayout(hwnd, state);
        }
    }
    unsafe fn set_code(hwnd: HWND, state: &State, code: String) {
        state.composition.borrow_mut().set_code(
            &state.input,
            code,
            &compose_options(),
            &state.learned.borrow(),
        );
        unsafe {
            relayout(hwnd, state);
        }
    }
    unsafe fn insert_text(state: &State, text: &str) {
        unsafe {
            SendMessageW(state.edit, EM_REPLACESEL, 1, wide(text).as_ptr() as isize);
        }
    }
    /// Writes the picked candidate or raw letters into the draft and ends this composition.
    unsafe fn commit_into_draft(hwnd: HWND, state: &State, text: &str) {
        unsafe {
            set_code(hwnd, state, String::new());
            insert_text(state, text);
        }
    }
    /// Picks candidate `index` on this page; false if there is none. A leading-syllable candidate
    /// (中 in zhonggou) leaves the remaining code for the next pick.
    unsafe fn pick(hwnd: HWND, state: &State, index: usize) -> bool {
        let pick = state.composition.borrow().candidate_at(index);
        match pick {
            Some(yiime_core::Pick { code, text, rest }) => {
                remember_pick(state, &code, &text);
                unsafe {
                    if rest.is_empty() {
                        commit_into_draft(hwnd, state, &text);
                    } else {
                        insert_text(state, &text);
                        set_code(hwnd, state, rest);
                    }
                }
                true
            }
            None => false,
        }
    }
    /// `[` `]`: commit the first/last character of the first candidate (a partial candidate leaves
    /// the remaining syllables).
    unsafe fn pick_char_of_word(hwnd: HWND, state: &State, last: bool) -> bool {
        let pick = state.composition.borrow().candidate_at(0);
        let Some(pick) = pick else {
            return false;
        };
        let Some(text) = yiime_core::char_of_word(&pick.text, last) else {
            return false;
        };
        unsafe {
            if pick.rest.is_empty() {
                commit_into_draft(hwnd, state, &text);
            } else {
                insert_text(state, &text);
                set_code(hwnd, state, pick.rest);
            }
        }
        true
    }
    /// Space: pick the first candidate (a partial candidate leaves the remaining syllables), or
    /// commit the raw letters if there is none.
    unsafe fn pick_first_or_raw(hwnd: HWND, state: &State) {
        if unsafe { !pick(hwnd, state, 0) } {
            let raw = state.composition.borrow().code.clone();
            unsafe {
                commit_into_draft(hwnd, state, &raw);
            }
        }
    }
    /// Punctuation, Shift+letter, Ctrl+Enter: commit the whole code, picking the first candidate
    /// segment by segment until it is used up.
    unsafe fn flush_composition(hwnd: HWND, state: &State) {
        for _ in 0..MAX_CODE_LEN {
            if state.composition.borrow().code.is_empty() {
                break;
            }
            unsafe {
                pick_first_or_raw(hwnd, state);
            }
        }
    }
    unsafe fn turn_page(hwnd: HWND, state: &State, forward: bool) {
        state.composition.borrow_mut().turn_page(forward);
        unsafe {
            relayout(hwnd, state);
        }
    }
    /// Character before the caret, to keep `.` `:` half-width after digits (`3.5`, `10:30`).
    unsafe fn char_before_caret(edit: HWND) -> Option<u16> {
        let mut start = 0u32;
        unsafe {
            SendMessageW(edit, EM_GETSEL, &mut start as *mut u32 as usize, 0);
        }
        let text: Vec<u16> = unsafe { get_text(edit) }.encode_utf16().collect();
        (start as usize)
            .checked_sub(1)
            .and_then(|index| text.get(index).copied())
    }
    /// Character after the caret, to skip over a closing mark when auto-closing.
    unsafe fn char_after_caret(edit: HWND) -> Option<u16> {
        let mut end = 0u32;
        unsafe {
            SendMessageW(edit, EM_GETSEL, 0, &mut end as *mut u32 as isize);
        }
        let text: Vec<u16> = unsafe { get_text(edit) }.encode_utf16().collect();
        text.get(end as usize).copied()
    }
    /// Moves the caret by `delta` UTF-16 units (unchecked).
    unsafe fn move_caret(edit: HWND, delta: isize) {
        let mut end = 0u32;
        unsafe {
            SendMessageW(edit, EM_GETSEL, 0, &mut end as *mut u32 as isize);
            let len = GetWindowTextLengthW(edit) as isize;
            let at = (end as isize + delta).clamp(0, len) as usize;
            SendMessageW(edit, EM_SETSEL, at, at as isize);
        }
    }
    unsafe fn insert_punctuation(state: &State, ch: char) -> bool {
        if option(Flag::PunctEnglish) {
            return false;
        }
        let mut quotes = state.quotes.get();
        let after_digit = || {
            unsafe { char_before_caret(state.edit) }
                .is_some_and(|unit| (b'0' as u16..=b'9' as u16).contains(&unit))
        };
        let text = quotes
            .next(ch)
            .or_else(|| yiime_core::full_width(ch, after_digit()));
        state.quotes.set(quotes);
        let Some(text) = text else {
            return false;
        };
        if option(Flag::AutoPair) {
            // Auto-close: insert the opening and closing marks with the caret between them; a
            // closing mark right after the caret is skipped over.
            if yiime_core::is_closing(text)
                && unsafe { char_after_caret(state.edit) } == text.encode_utf16().next()
            {
                unsafe {
                    move_caret(state.edit, 1);
                }
                return true;
            }
            if let Some(close) = yiime_core::closing_pair(text) {
                unsafe {
                    insert_text(state, &format!("{text}{close}"));
                    move_caret(state.edit, -(close.encode_utf16().count() as isize));
                }
                return true;
            }
        }
        unsafe {
            insert_text(state, text);
        }
        true
    }
    /// Character input in the box. True means handled, so the EDIT control does not see it.
    unsafe fn handle_char(hwnd: HWND, state: &State, unit: u32) -> bool {
        let Some(ch) = char::from_u32(unit) else {
            return false;
        };
        if ch == '\n' {
            // Ctrl+Enter: commit the Chinese draft without translating. A pending code first puts
            // its first candidate (or raw letters) into the draft. The box never inserts a newline.
            unsafe {
                flush_composition(hwnd, state);
                submit(hwnd, state, true);
            }
            return true;
        }
        // Numpad digits do not pick candidates: commit the code (segment by segment), then type the
        // digit.
        if state.numpad.replace(false) && ch.is_ascii_digit() {
            unsafe {
                flush_composition(hwnd, state);
            }
            return false;
        }
        // English segment: printable characters and space go to the edit box as typed (no
        // composition, no full-width punctuation).
        if state.english.get()
            && state.composition.borrow().code.is_empty()
            && yiime_core::is_literal_in_english(ch)
        {
            return false;
        }
        let composing = !state.composition.borrow().code.is_empty();
        let caps = unsafe { GetKeyState(VK_CAPITAL as i32) } & 1 != 0;
        match classify_letter(ch, caps, composing) {
            // Caps Lock or Shift+letter: type the letter as is (as in Microsoft Wubi), committing
            // the first candidate (or raw code) first if composing.
            Some(LetterAction::Raw { flush_first }) => {
                if flush_first {
                    unsafe {
                        flush_composition(hwnd, state);
                    }
                }
                return false;
            }
            // Code letter. Fifth-letter, empty-code and unique-four-letter rules live in the core.
            // In plain mode a commit produced here goes out together with the next one (flushing
            // now would end the session and lose the new letter).
            Some(LetterAction::Compose(letter)) => {
                let rules = LetterRules {
                    push_fifth: option(Flag::PushFifth),
                    auto_commit4: option(Flag::AutoCommit4),
                };
                let picks = {
                    let learned = state.learned.borrow();
                    state.composition.borrow_mut().type_letter(
                        &state.input,
                        letter,
                        rules,
                        &compose_options(),
                        &learned,
                    )
                };
                for pick in picks {
                    remember_pick(state, &pick.code, &pick.text);
                    unsafe {
                        insert_text(state, &pick.text);
                    }
                }
                unsafe {
                    relayout(hwnd, state);
                }
                return true;
            }
            None => {}
        }
        if !composing {
            return match ch {
                '\r' => {
                    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                    unsafe {
                        submit(hwnd, state, shift);
                    }
                    true
                }
                '\u{1b}' => {
                    unsafe {
                        cancel_session(hwnd, state);
                    }
                    true
                }
                '\t' => true,
                _ if !caps => unsafe { insert_punctuation(state, ch) },
                _ => false,
            };
        }
        // `;` picks the 2nd candidate and `'` the 3rd (Microsoft Wubi); `'` still separates
        // syllables when the code may be pinyin.
        if option(Flag::SemicolonSelect) {
            let code = state.composition.borrow().code.clone();
            if let Some(index) = selection_index(&state.input, &code, ch)
                && unsafe { pick(hwnd, state, index) }
            {
                return true;
            }
        }
        match ch {
            '\'' => {
                let mut code = state.composition.borrow().code.clone();
                if !code.ends_with('\'') && code.len() < MAX_CODE_LEN {
                    code.push('\'');
                    unsafe {
                        set_code(hwnd, state, code);
                    }
                }
            }
            ' ' => unsafe { pick_first_or_raw(hwnd, state) },
            '1'..='9' => unsafe {
                pick(hwnd, state, ch as usize - '1' as usize);
            },
            '\r' => {
                let raw = state.composition.borrow().code.clone();
                unsafe {
                    commit_into_draft(hwnd, state, &raw);
                }
            }
            '\u{1b}' => unsafe { set_code(hwnd, state, String::new()) },
            '\u{8}' => {
                let mut code = state.composition.borrow().code.clone();
                code.pop();
                unsafe {
                    set_code(hwnd, state, code);
                }
            }
            '-' | '=' | ',' | '.'
                if yiime_core::paging_direction(ch, option(Flag::CommaPeriodPaging)).is_some() =>
            unsafe {
                turn_page(hwnd, state, ch == '=' || ch == '.');
            },
            // `[` first character, `]` last character of the first candidate; with a
            // single-character candidate they are plain punctuation.
            '[' | ']'
                if option(Flag::BracketCharPick)
                    && unsafe { pick_char_of_word(hwnd, state, ch == ']') } => {}
            _ if (ch as u32) < 0x20 => {}
            _ => {
                // Other punctuation: commit the first candidate, then type the (full-width) mark,
                // as common Chinese IMEs do.
                unsafe {
                    flush_composition(hwnd, state);
                }
                return !caps && unsafe { insert_punctuation(state, ch) };
            }
        }
        true
    }
    unsafe extern "system" fn edit_proc(edit: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        let previous = unsafe { GetWindowLongPtrW(edit, GWLP_USERDATA) };
        let call_previous = || unsafe {
            CallWindowProcW(
                std::mem::transmute::<isize, WNDPROC>(previous),
                edit,
                msg,
                w,
                l,
            )
        };
        let hwnd = unsafe { GetParent(edit) };
        let Some(state) = (unsafe { state_of(hwnd) }) else {
            return call_previous();
        };
        // Shift tap (pressed and released alone) toggles the English segment.
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            state.numpad.set(yiime_core::is_numpad_digit(w as u32));
            let held = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
            let mut tap = state.shift_tap.get();
            tap.key_down(
                w as u32,
                held(VK_CONTROL) || held(VK_MENU) || held(VK_LWIN) || held(VK_RWIN),
                std::time::Instant::now(),
            );
            state.shift_tap.set(tap);
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            let mut tap = state.shift_tap.get();
            let tapped = tap.key_up(w as u32, std::time::Instant::now());
            state.shift_tap.set(tap);
            if tapped && !state.pending.get() {
                unsafe {
                    toggle_inline_english(hwnd, state);
                }
            }
        }
        match msg {
            WM_GETDLGCODE => return call_previous() | DLGC_WANTALLKEYS as isize,
            WM_CHAR => {
                if state.swallow_char.replace(false) && w <= 0x20 {
                    return 0;
                }
                if unsafe { handle_char(hwnd, state, w as u32) } {
                    unsafe {
                        flush_plain(hwnd, state);
                    }
                    return 0;
                }
                let result = call_previous();
                unsafe {
                    flush_plain(hwnd, state);
                }
                return result;
            }
            WM_KEYDOWN
                if unsafe { GetKeyState(VK_CONTROL as i32) } < 0
                    && unsafe { GetKeyState(VK_MENU as i32) } >= 0
                    && (w == 0xBE
                        || (w == b'F' as usize && unsafe { GetKeyState(VK_SHIFT as i32) } < 0)) =>
            {
                // Ctrl+.: Chinese/English punctuation; Ctrl+Shift+F: Simplified/Traditional output
                // (Microsoft Wubi shortcuts).
                let (option_key, on_text, off_text) = if w == 0xBE {
                    (Flag::PunctEnglish, "英文标点", "中文标点")
                } else {
                    (Flag::Traditional, "繁体输出", "简体输出")
                };
                let on = !option(option_key);
                set_option(option_key, on);
                state.swallow_char.set(true);
                let code = state.composition.borrow().code.clone();
                unsafe {
                    set_code(hwnd, state, code);
                    set_status(
                        hwnd,
                        state,
                        Some(((if on { on_text } else { off_text }).to_owned(), false)),
                    );
                }
                return 0;
            }
            // Ctrl+Shift+digit: forget the learned pick of that candidate under the current code
            // (Microsoft Pinyin convention).
            WM_KEYDOWN
                if (0x31..=0x39).contains(&w)
                    && unsafe { GetKeyState(VK_CONTROL as i32) } < 0
                    && unsafe { GetKeyState(VK_SHIFT as i32) } < 0
                    && unsafe { GetKeyState(VK_MENU as i32) } >= 0
                    && !state.composition.borrow().code.is_empty() =>
            {
                state.swallow_char.set(true);
                let pick = state.composition.borrow().candidate_at(w - 0x31);
                if let Some(pick) = pick {
                    let forgot = state.learned.borrow_mut().forget(&pick.code, &pick.text);
                    let code = state.composition.borrow().code.clone();
                    let text = if forgot {
                        format!("已删除“{}”的学习记录", pick.text)
                    } else {
                        format!("“{}”没有学习记录（词库词不删除）", pick.text)
                    };
                    unsafe {
                        set_code(hwnd, state, code);
                        set_status(hwnd, state, Some((text, false)));
                    }
                }
                return 0;
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if is_plain_toggle(w as u16) => {
                // The mode can also be toggled inside the box; the code and draft stay and the hint
                // line updates.
                set_plain_mode(!plain_mode());
                // Only Ctrl combinations (without Alt) produce a following WM_CHAR (space or
                // control character).
                state.swallow_char.set(unsafe {
                    GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_MENU as i32) >= 0
                });
                unsafe {
                    relayout(hwnd, state);
                    flush_plain(hwnd, state);
                }
                return 0;
            }
            // With no code and an empty draft (or a recalled entry unchanged): ↑/PgUp shows older
            // history, ↓/PgDn newer.
            WM_KEYDOWN
                if state.composition.borrow().code.is_empty()
                    && [VK_UP, VK_DOWN, VK_PRIOR, VK_NEXT].contains(&(w as u16))
                    && option(Flag::HistoryEnabled) =>
            {
                let draft = unsafe { get_text(state.edit) };
                let current = state.history_pos.get();
                let untouched = draft.is_empty()
                    || current.is_some_and(|index| {
                        load_history()
                            .get(index)
                            .is_some_and(|entry| entry.chinese == draft)
                    });
                if untouched {
                    let older = [VK_UP, VK_PRIOR].contains(&(w as u16));
                    let next = match (current, older) {
                        (None, true) => Some(0),
                        (None, false) => None,
                        (Some(index), true) => Some(index + 1),
                        (Some(index), false) => index.checked_sub(1),
                    };
                    if let Some(index) = next
                        && unsafe { show_history(hwnd, state, index) }
                    {
                        state.recall_session.set(true);
                    }
                    return 0;
                }
            }
            WM_KEYDOWN if !state.composition.borrow().code.is_empty() => {
                let key = w as u16;
                if key == VK_PRIOR || key == VK_NEXT {
                    unsafe {
                        turn_page(hwnd, state, key == VK_NEXT);
                    }
                    return 0;
                }
                if [
                    VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_HOME, VK_END, VK_DELETE,
                ]
                .contains(&key)
                {
                    return 0;
                }
            }
            _ => {}
        }
        call_previous()
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
    unsafe fn activate(
        hwnd: HWND,
        state: &State,
        target: HWND,
        anchor: Option<RECT>,
        code: String,
    ) {
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
        let window = hwnd as usize;
        std::thread::spawn(move || {
            let outcome = yiime_core::translate(&settings, &draft);
            for attempt in &outcome.attempts {
                translate_log(attempt);
            }
            let result = outcome.result;
            let response = Box::new(TranslationResult {
                generation,
                source: draft,
                result,
            });
            let pointer = Box::into_raw(response);
            if unsafe { PostMessageW(window as HWND, WM_TRANSLATED, 0, pointer as isize) } == 0 {
                drop(unsafe { Box::from_raw(pointer) });
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
    fn copy_wide(target: &mut [u16], text: &str) {
        let units: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
        target[..units.len()].copy_from_slice(&units);
        target[units.len()] = 0;
    }
    unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data
    }
    unsafe fn add_tray(hwnd: HWND, balloon: Option<&str>) {
        let mut data = unsafe { tray_data(hwnd) };
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = unsafe { app_icon(GetSystemMetrics(SM_CXSMICON)) };
        copy_wide(&mut data.szTip, &tray_tip());
        if let Some(text) = balloon {
            data.uFlags |= NIF_INFO;
            data.dwInfoFlags = NIIF_INFO;
            copy_wide(&mut data.szInfoTitle, APP_NAME);
            copy_wide(&mut data.szInfo, text);
        }
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &data);
            // Version 4: right-click sends WM_CONTEXTMENU, left-click NIN_SELECT, with the message
            // in the low 16 bits of lParam.
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            TRAY_V4.store(
                Shell_NotifyIconW(NIM_SETVERSION, &data) != 0,
                Ordering::Relaxed,
            );
        }
    }
    unsafe fn tray_menu(hwnd: HWND) {
        unsafe {
            let menu = CreatePopupMenu();
            let checked = if plain_mode() {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
            // The same shortcut toggles both ways.
            let toggle = plain_toggle_text();
            let translate_checked = if plain_mode() {
                MF_UNCHECKED
            } else {
                MF_CHECKED
            };
            AppendMenuW(
                menu,
                MF_STRING | translate_checked,
                TRAY_TRANSLATE,
                wide(&format!("翻译模式（译）\t{toggle}")).as_ptr(),
            );
            AppendMenuW(
                menu,
                MF_STRING | checked,
                TRAY_PLAIN,
                wide(&format!("普通五笔模式（中，不翻译）\t{toggle}")).as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(
                menu,
                MF_STRING,
                TRAY_HISTORY,
                wide(&format!("输入记忆…\t{}", history_key_text())).as_ptr(),
            );
            AppendMenuW(menu, MF_STRING, TRAY_SETTINGS, wide("设置…").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, TRAY_EXIT, wide("退出").as_ptr());
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                point.x,
                point.y,
                0,
                hwnd,
                null(),
            );
            DestroyMenu(menu);
            PostMessageW(hwnd, WM_NULL, 0, 0);
            match command as usize {
                TRAY_SETTINGS => open_settings(hwnd),
                TRAY_HISTORY => open_history(),
                TRAY_PLAIN | TRAY_TRANSLATE => {
                    set_plain_mode(command as usize == TRAY_PLAIN);
                    if let Some(state) = state_of(hwnd) {
                        state.last_plain.set(plain_mode());
                    }
                    update_tray_tip(hwnd);
                }
                TRAY_EXIT => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
        }
    }
    thread_local! {
        /// History window and the entries behind its list (the tray process is single-threaded).
        static HISTORY_WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
        static HISTORY_VIEW: RefCell<Vec<HistoryEntry>> = const { RefCell::new(Vec::new()) };
    }
    unsafe fn fill_history(window: HWND) {
        let entries = load_history();
        unsafe {
            let list = GetDlgItem(window, HISTORY_LIST);
            SendMessageW(list, LB_RESETCONTENT, 0, 0);
            for entry in &entries {
                SendMessageW(
                    list,
                    LB_ADDSTRING,
                    0,
                    wide(&history_label(entry)).as_ptr() as isize,
                );
            }
            if !entries.is_empty() {
                SendMessageW(list, LB_SETCURSEL, 0, 0);
            }
            set_text(
                GetDlgItem(window, HISTORY_STATUS),
                &if !option(Flag::HistoryEnabled) {
                    "输入记忆已关闭（设置里可打开）".to_owned()
                } else if entries.is_empty() {
                    "还没有输入记忆：翻译上屏后会自动记住。".to_owned()
                } else {
                    format!(
                        "共 {} 条（最多 {HISTORY_CAP} 条，仅存本机、DPAPI 加密）。“再次使用”后在应用里按 {} 即可找回编辑。",
                        entries.len(),
                        history_key_text()
                    )
                },
            );
        }
        HISTORY_VIEW.with(|view| *view.borrow_mut() = entries);
    }
    /// Tray menu "输入记忆…": lists recent translations; copy the Chinese or English, or queue one as
    /// the next recall.
    unsafe fn open_history() {
        let existing = HISTORY_WINDOW.with(Cell::get);
        if !existing.is_null() {
            unsafe {
                fill_history(existing);
                ShowWindow(existing, SW_SHOWNORMAL);
                SetForegroundWindow(existing);
            }
            return;
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_TOPMOST | WS_EX_APPWINDOW;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 640,
            bottom: 380,
        };
        unsafe {
            AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        }
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let window = unsafe {
            CreateWindowExW(
                ex_style,
                wide("InputTranslationHistory").as_ptr(),
                wide("YiIME 译 · 输入记忆").as_ptr(),
                style,
                (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
                (GetSystemMetrics(SM_CYSCREEN) - height) / 3,
                width,
                height,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        if window.is_null() {
            return;
        }
        HISTORY_WINDOW.with(|cell| cell.set(window));
        unsafe {
            fill_history(window);
            ShowWindow(window, SW_SHOWNORMAL);
            SetForegroundWindow(window);
        }
    }
    unsafe extern "system" fn history_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        let selected = |hwnd: HWND| -> Option<HistoryEntry> {
            let index = unsafe { SendMessageW(GetDlgItem(hwnd, HISTORY_LIST), LB_GETCURSEL, 0, 0) };
            let index = usize::try_from(index).ok()?;
            HISTORY_VIEW.with(|view| view.borrow().get(index).cloned())
        };
        match msg {
            WM_CREATE => unsafe {
                control(
                    hwnd,
                    "LISTBOX",
                    "",
                    (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32
                        | WS_VSCROLL
                        | WS_BORDER
                        | WS_TABSTOP,
                    12,
                    12,
                    616,
                    282,
                    HISTORY_LIST,
                );
                for (index, (id, text)) in [
                    (HISTORY_COPY_ZH, "复制中文"),
                    (HISTORY_COPY_EN, "复制译文"),
                    (HISTORY_REUSE, "再次使用"),
                    (HISTORY_CLOSE, "关闭"),
                ]
                .into_iter()
                .enumerate()
                {
                    control(
                        hwnd,
                        "BUTTON",
                        text,
                        WS_TABSTOP,
                        12 + index as i32 * 100,
                        304,
                        92,
                        28,
                        id,
                    );
                }
                control(hwnd, "STATIC", "", 0, 12, 342, 616, 32, HISTORY_STATUS);
                0
            },
            WM_COMMAND => unsafe {
                let id = (w & 0xffff) as i32;
                let notification = ((w >> 16) & 0xffff) as u32;
                let status = GetDlgItem(hwnd, HISTORY_STATUS);
                match id {
                    HISTORY_COPY_ZH | HISTORY_COPY_EN => {
                        if let Some(entry) = selected(hwnd) {
                            let text = if id == HISTORY_COPY_ZH {
                                &entry.chinese
                            } else {
                                &entry.english
                            };
                            set_text(
                                status,
                                if set_clipboard(hwnd, text) {
                                    "已复制到剪贴板。"
                                } else {
                                    "复制失败。"
                                },
                            );
                        }
                    }
                    HISTORY_REUSE => {
                        reuse_history(hwnd, selected(hwnd));
                    }
                    HISTORY_LIST if notification == LBN_DBLCLK => {
                        reuse_history(hwnd, selected(hwnd));
                    }
                    HISTORY_CLOSE => {
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                0
            },
            WM_CLOSE => unsafe {
                DestroyWindow(hwnd);
                0
            },
            WM_DESTROY => {
                HISTORY_WINDOW.with(|cell| cell.set(null_mut()));
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
        }
    }
    /// "再用一次": move the entry to the front (the next recall loads it) and copy its Chinese.
    unsafe fn reuse_history(window: HWND, entry: Option<HistoryEntry>) {
        let Some(entry) = entry else {
            return;
        };
        let mut entries = load_history();
        entries.retain(|item| item.chinese != entry.chinese);
        entries.insert(0, entry.clone());
        let saved = option(Flag::HistoryEnabled) && save_history(&entries);
        unsafe {
            let copied = set_clipboard(window, &entry.chinese);
            fill_history(window);
            set_text(
                GetDlgItem(window, HISTORY_STATUS),
                &format!(
                    "{}{}",
                    if saved {
                        format!(
                            "已设为下一次找回的草稿：回到应用按 {} 即可编辑、重新翻译。",
                            history_key_text()
                        )
                    } else {
                        String::new()
                    },
                    if copied { "中文已复制。" } else { "" }
                ),
            );
        }
    }
    /// Opens the settings window (or brings it forward).
    unsafe fn open_settings(owner: HWND) {
        let Some(state) = (unsafe { state_of(owner) }) else {
            return;
        };
        let existing = state.settings_window.get();
        if !existing.is_null() {
            unsafe {
                ShowWindow(existing, SW_SHOWNORMAL);
                SetForegroundWindow(existing);
            }
            return;
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_DLGMODALFRAME;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 440,
            bottom: 524 + OPTION_SHIFT + ENGINE_SHIFT,
        };
        unsafe {
            AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        }
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let (x, y) = unsafe {
            (
                (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
                (GetSystemMetrics(SM_CYSCREEN) - height) / 3,
            )
        };
        let window = unsafe {
            CreateWindowExW(
                ex_style,
                wide("InputTranslationSettings").as_ptr(),
                wide(concat!("YiIME 译 · 设置 · v", env!("CARGO_PKG_VERSION"))).as_ptr(),
                style,
                x,
                y,
                width,
                height,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                owner as *const std::ffi::c_void,
            )
        };
        if window.is_null() {
            return;
        }
        state.settings_window.set(window);
        unsafe {
            ShowWindow(window, SW_SHOWNORMAL);
            SetForegroundWindow(window);
            let url = GetDlgItem(window, SETTINGS_URL);
            SetFocus(url);
            SendMessageW(url, EM_SETSEL, 0, -1);
        }
    }
    unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        let owner = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as HWND;
        match msg {
            WM_CREATE => unsafe {
                let create = &*(l as *const CREATESTRUCTW);
                let owner = create.lpCreateParams as HWND;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, owner as isize);
                let Some(state) = state_of(owner) else {
                    return -1;
                };
                let settings = state.settings.borrow().clone();
                label(
                    hwnd,
                    "大模型 API 地址（OpenAI 兼容；选 Google 时作备用，可不填）",
                    16,
                    12,
                    408,
                );
                control(
                    hwnd,
                    "EDIT",
                    &settings.url,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    16,
                    34,
                    408,
                    24,
                    SETTINGS_URL,
                );
                label(hwnd, "模型", 16, 68, 196);
                control(
                    hwnd,
                    "EDIT",
                    &settings.model,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    16,
                    90,
                    196,
                    24,
                    SETTINGS_MODEL,
                );
                label(hwnd, "API 密钥（Windows 加密保存）", 228, 68, 196);
                control(
                    hwnd,
                    "EDIT",
                    &settings.key,
                    ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    228,
                    90,
                    196,
                    24,
                    SETTINGS_KEY,
                );
                label(hwnd, "翻译风格", 16, 124, 196);
                let combo = control(
                    hwnd,
                    "COMBOBOX",
                    "",
                    CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                    16,
                    146,
                    196,
                    220,
                    SETTINGS_STYLE,
                );
                for style in TranslationStyle::ALL {
                    SendMessageW(
                        combo,
                        CB_ADDSTRING,
                        0,
                        wide(style.label()).as_ptr() as isize,
                    );
                }
                let selected = TranslationStyle::ALL
                    .iter()
                    .position(|style| *style == settings.style)
                    .unwrap_or(0);
                SendMessageW(combo, CB_SETCURSEL, selected, 0);
                label(
                    hwnd,
                    "自定义要求（可选，追加到翻译提示词，例如“使用英式拼写”）",
                    16,
                    182,
                    408,
                );
                control(
                    hwnd,
                    "EDIT",
                    &settings.custom,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    16,
                    204,
                    408,
                    24,
                    SETTINGS_CUSTOM,
                );
                let plain = control(
                    hwnd,
                    "BUTTON",
                    "普通五笔模式（不翻译，选字直接上屏）",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                    16,
                    242,
                    408,
                    24,
                    SETTINGS_PLAIN,
                );
                SendMessageW(plain, BM_SETCHECK, plain_mode() as usize, 0);
                label(
                    hwnd,
                    "切换快捷键（如 Ctrl+Shift+Space；重新切到本输入法后生效）",
                    16,
                    272,
                    408,
                );
                control(
                    hwnd,
                    "EDIT",
                    &plain_toggle_text(),
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    16,
                    294,
                    196,
                    24,
                    SETTINGS_HOTKEY,
                );
                label(hwnd, "每页候选数（1–9）", 228, 298, 128);
                control(
                    hwnd,
                    "EDIT",
                    &OPTIONS.page_size().to_string(),
                    ES_NUMBER as u32 | WS_TABSTOP,
                    360,
                    294,
                    48,
                    24,
                    SETTINGS_PAGE_SIZE,
                );
                SendMessageW(GetDlgItem(hwnd, SETTINGS_PAGE_SIZE), EM_LIMITTEXT, 2, 0);
                for (index, (id, text, option_key)) in OPTION_BOXES.iter().enumerate() {
                    let x = if index % 2 == 0 { 16 } else { 228 };
                    let y = 330 + (index as i32 / 2) * 26;
                    let checkbox = control(
                        hwnd,
                        "BUTTON",
                        text,
                        BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                        x,
                        y,
                        204,
                        24,
                        *id,
                    );
                    // "中文标点" checked = PunctEnglish off.
                    let on = option(*option_key) != (*option_key == Flag::PunctEnglish);
                    SendMessageW(checkbox, BM_SETCHECK, on as usize, 0);
                }
                let history = control(
                    hwnd,
                    "BUTTON",
                    "输入记忆：保存翻译过的中文，快捷键找回重新编辑/翻译",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                    16,
                    412 + OPTION_SHIFT,
                    408,
                    24,
                    SETTINGS_HISTORY,
                );
                SendMessageW(
                    history,
                    BM_SETCHECK,
                    option(Flag::HistoryEnabled) as usize,
                    0,
                );
                label(hwnd, "找回快捷键", 16, 446 + OPTION_SHIFT, 80);
                control(
                    hwnd,
                    "EDIT",
                    &history_key_text(),
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    100,
                    442 + OPTION_SHIFT,
                    150,
                    24,
                    SETTINGS_HISTORY_KEY,
                );
                control(
                    hwnd,
                    "BUTTON",
                    "清除输入记忆",
                    WS_TABSTOP,
                    272,
                    440 + OPTION_SHIFT,
                    152,
                    28,
                    SETTINGS_CLEAR_HISTORY,
                );
                control(
                    hwnd,
                    "BUTTON",
                    "用户词库…",
                    WS_TABSTOP,
                    16,
                    482 + OPTION_SHIFT,
                    100,
                    28,
                    SETTINGS_USER_WORDS,
                );
                control(
                    hwnd,
                    "BUTTON",
                    "保存",
                    BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
                    256,
                    482 + OPTION_SHIFT,
                    80,
                    28,
                    ID_OK,
                );
                control(
                    hwnd,
                    "BUTTON",
                    "取消",
                    WS_TABSTOP,
                    344,
                    482 + OPTION_SHIFT,
                    80,
                    28,
                    ID_CANCEL,
                );
                // Insert the Translation Engine row at the top: move the existing controls down and
                // put the new ones first in tab order.
                let mut child = GetWindow(hwnd, GW_CHILD);
                while !child.is_null() {
                    let mut rect: RECT = std::mem::zeroed();
                    GetWindowRect(child, &mut rect);
                    MapWindowPoints(null_mut(), hwnd, &mut rect as *mut RECT as *mut POINT, 2);
                    SetWindowPos(
                        child,
                        null_mut(),
                        rect.left,
                        rect.top + ENGINE_SHIFT,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    child = GetWindow(child, GW_HWNDNEXT);
                }
                label(hwnd, "翻译引擎（失败时自动改用大模型）", 16, 12, 196);
                let engine = control(
                    hwnd,
                    "COMBOBOX",
                    "",
                    CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                    16,
                    34,
                    196,
                    160,
                    SETTINGS_ENGINE,
                );
                for item in Engine::ALL {
                    SendMessageW(
                        engine,
                        CB_ADDSTRING,
                        0,
                        wide(item.label()).as_ptr() as isize,
                    );
                }
                let selected = Engine::ALL
                    .iter()
                    .position(|item| *item == settings.engine)
                    .unwrap_or(0);
                SendMessageW(engine, CB_SETCURSEL, selected, 0);
                label(hwnd, "Google Cloud 密钥（可选，加密保存）", 228, 12, 196);
                let google_key = control(
                    hwnd,
                    "EDIT",
                    &settings.google_key,
                    ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    228,
                    34,
                    196,
                    24,
                    SETTINGS_GOOGLE_KEY,
                );
                label(hwnd, "Azure 翻译密钥（加密保存）", 16, 68, 196);
                let azure_key = control(
                    hwnd,
                    "EDIT",
                    &settings.azure_key,
                    ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    16,
                    90,
                    196,
                    24,
                    SETTINGS_AZURE_KEY,
                );
                label(hwnd, "Azure 区域（如 eastus，全局可留空）", 228, 68, 196);
                let azure_region = control(
                    hwnd,
                    "EDIT",
                    &settings.azure_region,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    228,
                    90,
                    196,
                    24,
                    SETTINGS_AZURE_REGION,
                );
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                SetWindowPos(engine, HWND_TOP, 0, 0, 0, 0, flags);
                SetWindowPos(google_key, engine, 0, 0, 0, 0, flags);
                SetWindowPos(azure_key, google_key, 0, 0, 0, 0, flags);
                SetWindowPos(azure_region, azure_key, 0, 0, 0, 0, flags);
                0
            },
            WM_COMMAND => unsafe {
                match (w & 0xffff) as i32 {
                    ID_OK => {
                        let Some(state) = state_of(owner) else {
                            DestroyWindow(hwnd);
                            return 0;
                        };
                        let field = |id| get_text(GetDlgItem(hwnd, id)).trim().to_owned();
                        let index =
                            SendMessageW(GetDlgItem(hwnd, SETTINGS_STYLE), CB_GETCURSEL, 0, 0);
                        let engine_index =
                            SendMessageW(GetDlgItem(hwnd, SETTINGS_ENGINE), CB_GETCURSEL, 0, 0);
                        let settings = ApiSettings {
                            engine: usize::try_from(engine_index)
                                .ok()
                                .and_then(|index| Engine::ALL.get(index).copied())
                                .unwrap_or_default(),
                            google_key: field(SETTINGS_GOOGLE_KEY),
                            azure_key: field(SETTINGS_AZURE_KEY),
                            azure_region: field(SETTINGS_AZURE_REGION),
                            url: field(SETTINGS_URL),
                            model: field(SETTINGS_MODEL),
                            key: field(SETTINGS_KEY),
                            style: usize::try_from(index)
                                .ok()
                                .and_then(|index| TranslationStyle::ALL.get(index).copied())
                                .unwrap_or_default(),
                            custom: field(SETTINGS_CUSTOM)
                                .chars()
                                .take(MAX_CUSTOM_LEN)
                                .collect(),
                        };
                        let plain =
                            SendMessageW(GetDlgItem(hwnd, SETTINGS_PLAIN), BM_GETCHECK, 0, 0) == 1; // BST_CHECKED
                        let hotkey = field(SETTINGS_HOTKEY);
                        // Page size must be a number; values outside 1–9 are clamped and written
                        // back to the field.
                        let page = match parse_page_size(&field(SETTINGS_PAGE_SIZE)) {
                            Ok(page) => page,
                            Err(error) => {
                                message(hwnd, error);
                                return 0;
                            }
                        };
                        SetWindowTextW(
                            GetDlgItem(hwnd, SETTINGS_PAGE_SIZE),
                            wide(&page.to_string()).as_ptr(),
                        );
                        let history_key = field(SETTINGS_HISTORY_KEY);
                        let history_on =
                            SendMessageW(GetDlgItem(hwnd, SETTINGS_HISTORY), BM_GETCHECK, 0, 0)
                                == 1;
                        // Turning history off: confirm, then delete saved history; on cancel the
                        // settings are not saved.
                        let history_has_data = history_path().is_some_and(|path| path.exists());
                        if !history_on
                            && history_has_data
                            && MessageBoxW(
                                hwnd,
                                wide("关闭输入记忆会删除已保存的全部记忆，确定吗？").as_ptr(),
                                wide(APP_NAME).as_ptr(),
                                MB_YESNO | MB_ICONQUESTION,
                            ) != IDYES
                        {
                            return 0;
                        }
                        // Plain Wubi mode never translates, so API settings are not required. With
                        // Google selected the LLM is an optional fallback, but once a key is
                        // entered the whole LLM configuration must be valid.
                        let api_check = if plain && settings.key.is_empty() {
                            Ok(())
                        } else if settings.engine != Engine::Llm && !settings.key.trim().is_empty()
                        {
                            settings.validate().and_then(|_| settings.validate_llm())
                        } else {
                            settings.validate()
                        };
                        if parse_hotkey(&hotkey).is_none() || parse_hotkey(&history_key).is_none() {
                            message(
                                hwnd,
                                "快捷键无效：用 + 连接 Ctrl/Shift/Alt 与 Space、字母、数字或 F1～F12，且须含 Ctrl 或 Alt（F 键除外）",
                            );
                        } else if let Err(error) = api_check {
                            message(hwnd, error);
                        } else if !set_plain_mode(plain)
                            || !OPTIONS.set_plain_toggle_key(&hotkey)
                            || !set_option(Flag::HistoryEnabled, history_on)
                            || !OPTIONS.set_page_size(page)
                            || !OPTIONS.set_history_key(&history_key)
                            || (!history_on && !clear_history())
                            || !OPTION_BOXES.iter().all(|(id, _, option_key)| {
                                let checked =
                                    SendMessageW(GetDlgItem(hwnd, *id), BM_GETCHECK, 0, 0) == 1;
                                set_option(
                                    *option_key,
                                    checked != (*option_key == Flag::PunctEnglish),
                                )
                            })
                        {
                            message(hwnd, "保存模式设置失败");
                        } else if let Err(error) = save_settings(&settings) {
                            message(hwnd, &format!("保存设置失败：{error}"));
                        } else {
                            *state.settings.borrow_mut() = settings;
                            if state
                                .status
                                .borrow()
                                .as_ref()
                                .is_some_and(|(_, error)| *error)
                            {
                                set_status(
                                    owner,
                                    state,
                                    Some(("设置已保存，按 Enter 翻译".into(), false)),
                                );
                            }
                            DestroyWindow(hwnd);
                        }
                    }
                    ID_CANCEL => {
                        DestroyWindow(hwnd);
                    }
                    SETTINGS_CLEAR_HISTORY => {
                        if MessageBoxW(
                            hwnd,
                            wide("删除全部输入记忆？").as_ptr(),
                            wide(APP_NAME).as_ptr(),
                            MB_YESNO | MB_ICONQUESTION,
                        ) == IDYES
                        {
                            message(
                                hwnd,
                                if clear_history() {
                                    "输入记忆已清除"
                                } else {
                                    "清除失败"
                                },
                            );
                        }
                    }
                    SETTINGS_USER_WORDS => {
                        // Open user words in Notepad (creating the file with a short header if
                        // missing); changes apply the next time the box opens.
                        if let Some(path) = user_words_path() {
                            if !path.exists() {
                                let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
                                let _ = std::fs::write(
                                    &path,
                                    "# YiIME 用户词库：每行「词 编码」（空格或 Tab 分隔），排在该五笔编码的第一位。\r\n# 例如：\r\n# 中英翻译 khyc\r\n",
                                );
                            }
                            let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
                        }
                    }
                    _ => {}
                }
                0
            },
            WM_CLOSE => unsafe {
                DestroyWindow(hwnd);
                0
            },
            WM_DESTROY => unsafe {
                if let Some(state) = state_of(owner) {
                    state.settings_window.set(null_mut());
                    if IsWindowVisible(owner) != 0 {
                        SetForegroundWindow(owner);
                        SetFocus(state.edit);
                    }
                }
                0
            },
            _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
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
            WM_TSF_EXIT => {
                if let Some(state) = state {
                    state.finished.set(true);
                }
                unsafe {
                    DestroyWindow(hwnd);
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
            WM_TRANSLATED => {
                let response = unsafe { Box::from_raw(l as *mut TranslationResult) };
                if let Some(state) = state {
                    unsafe {
                        on_translated(hwnd, state, *response);
                    }
                }
                0
            }
            WM_TIMER if w == TRAY_TIMER => {
                if let Some(state) = state
                    && state.last_plain.get() != plain_mode()
                {
                    state.last_plain.set(plain_mode());
                    unsafe {
                        update_tray_tip(hwnd);
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
                    WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT
                        if TRAY_V4.load(Ordering::Relaxed) =>
                    unsafe { tray_menu(hwnd) },
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
        // The large lexicon (derived from rime-frost, GPL-3.0) ships as a separate file read at
        // runtime, not compiled into the EXE.
        let input = match frequency_words() {
            Some(words) => input.with_frequency_words(words),
            None => input,
        };
        // User words: %APPDATA%\\InputTranslation\\user_words.txt, `word code` per line, ranked
        // first under that code.
        let input = match user_words_path().and_then(|path| std::fs::read_to_string(path).ok()) {
            Some(words) => input.with_user_words(&words),
            None => input,
        };
        // Build the initials index (about 5 MB, 0.1–0.2 s) in the background so the box opens
        // without delay; the tray instance does not need it.
        let input = std::sync::Arc::new(input);
        if !tsf_receiver.is_null() {
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
            let state = Box::new(State {
                input,
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
            } else {
                if wants_history {
                    open_history();
                } else if wants_settings || state.settings.borrow().validate().is_err() {
                    open_settings(hwnd);
                }
            }
            let mut msg = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                let settings =
                    state_of(hwnd).map_or(null_mut(), |state| state.settings_window.get());
                if !settings.is_null() && IsDialogMessageW(settings, &msg) != 0 {
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    app::run();
}
