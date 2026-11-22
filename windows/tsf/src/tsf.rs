//! Windows TSF text service. Typing happens in the companion input box; the result is inserted
//! through a TSF edit session.
#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, HINSTANCE, HMODULE, RECT, S_FALSE,
    S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IClassFactory, IClassFactory_Impl,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::System::Variant::{VARIANT, VT_I4, VariantClear};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_COMPARTMENT_EMPTYCONTEXT,
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
    GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, GUID_LBI_INPUTMODE, GUID_PROP_INPUTSCOPE,
    GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_COMLESS, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    INSERT_TEXT_AT_SELECTION_FLAGS, IS_ALPHANUMERIC_PIN, IS_ALPHANUMERIC_PIN_SET, IS_NUMERIC_PIN,
    IS_PASSWORD, ITfCategoryMgr, ITfCompartmentEventSink, ITfCompartmentEventSink_Impl,
    ITfCompartmentMgr, ITfContext, ITfDocumentMgr, ITfEditRecord, ITfEditSession,
    ITfEditSession_Impl, ITfInputProcessorProfiles, ITfInputScope, ITfInsertAtSelection,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItem_Impl,
    ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemMgr, ITfLangBarItemSink,
    ITfMenu, ITfSource, ITfSource_Impl, ITfTextEditSink, ITfTextEditSink_Impl,
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, TF_AE_END, TF_ANCHOR_END, TF_CONVERSIONMODE_NATIVE,
    TF_DEFAULT_SELECTION, TF_E_NOOBJECT, TF_ES_ASYNC, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
    TF_LANGBARITEMINFO, TF_LBI_CLK_RIGHT, TF_LBI_ICON, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_TEXT,
    TF_LBI_TOOLTIP, TF_MOD_ALT, TF_MOD_CONTROL, TF_MOD_SHIFT, TF_PRESERVEDKEY, TF_SD_READONLY,
    TF_SELECTION, TF_SELECTIONSTYLE, TfLBIClick,
};
use windows::core::{
    BOOL, BSTR, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result, implement,
};
use windows_registry::{CLASSES_ROOT, CURRENT_USER, LOCAL_MACHINE};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_sys::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    ES_PASSWORD, GWL_STYLE, GWLP_USERDATA, GetClassNameW, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowLongW, GetWindowThreadProcessId, HWND_MESSAGE, IMAGE_ICON,
    IsWindow, KillTimer, LoadImageW, PostMessageW, RegisterClassW, SM_CXSMICON, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, SetTimer, SetWindowLongPtrW, WM_APP, WM_COPYDATA, WM_CREATE, WM_TIMER,
    WNDCLASSW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowExW, FindWindowW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx,
};
use yiime_core::{Flag, OptionStore, Options, Quotes, full_width, us_key_char};

mod key_sink;
mod lang_bar;
mod receiver;
mod registration;
use lang_bar::{
    InputModeButton, LangBarSink, chinese_punctuation, history_key_text, option, parse_hotkey,
    plain_mode, plain_toggle_text, refresh_sink, set_plain_mode, toggle_option,
};
use registration::{register, unregister};

const CLSID: GUID = GUID::from_u128(0xa72091c2_7459_4c47_85f4_4626ae42a6c8);
const CLSID_TEXT: &str = "{A72091C2-7459-4C47-85F4-4626AE42A6C8}";
const PROFILE: GUID = GUID::from_u128(0x54a39d73_0625_4560_b878_94660908c69d);
const TOGGLE_KEY: GUID = GUID::from_u128(0x39c04f7e_c659_4d9e_ad8e_55929249bb77);
const TOGGLE_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x20,
    uModifiers: TF_MOD_CONTROL,
};
/// Preserved key for toggling plain Wubi mode (no translation); the chord comes from
/// HKCU\\Software\\YiIME\\PlainToggleKey.
const PLAIN_KEY: GUID = GUID::from_u128(0x6b1f2d0e_93c4_4e1a_9f6d_2a7c51e0b8d3);
/// Ctrl+.: Chinese/English punctuation; Ctrl+Shift+F: Simplified/Traditional output (as in
/// Microsoft Wubi).
const PUNCT_KEY: GUID = GUID::from_u128(0x0d6c5e21_7a43_4f8e_b1d2_6e9a3c4f5b71);
const PUNCT_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0xBE,
    uModifiers: TF_MOD_CONTROL,
};
/// Input history: open the box with the most recently translated Chinese (default Ctrl+Shift+Z,
/// HKCU …\\HistoryKey).
const HISTORY_KEY: GUID = GUID::from_u128(0x2f4b8c6a_51d3_4e07_9a2c_7b1e0d3f6a58);
/// A first key of 1 sent to the editor means "recall history", not a letter.
const RECALL: u8 = 1;
const TRAD_KEY: GUID = GUID::from_u128(0x5a8e7f13_2c64_4b9d_8e1f_3d7b6a2c9e04);
const TRAD_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: b'F' as u32,
    uModifiers: TF_MOD_CONTROL | TF_MOD_SHIFT,
};
/// Settings key shared with the EXE (HKCU) and the installer's state key (HKLM, CurrentDir).
const STATE_KEY: &str = "Software\\YiIME";
/// Key used by 0.1.x: settings are copied from it once (see [`migrate_settings_key`]); old DLLs
/// still loaded somewhere keep reading it.
const LEGACY_STATE_KEY: &str = "Software\\InputTranslation";
/// The input-box/tray EXE, and its name in 0.1.x.
const EDITOR_EXE: &str = "YiIME.exe";
const LEGACY_EDITOR_EXE: &str = "InputTranslation-Portable.exe";
const LANG_ZH_CN: u16 = 0x0804;
const DESCRIPTION: &str = "YiIME 译（五笔）";
/// Versioned, so input boxes of another protocol version ignore this receiver.
const WINDOW_CLASS: &str = yiime_core::TSF_RECEIVER_CLASS;
/// Timers: 1 = retry forwarding buffered keys to the editor; 2 = give up waiting for the editor.
const FLUSH_TIMER: usize = 1;
const READY_TIMER: usize = 2;
const READY_TIMEOUT_MS: u32 = 5000;
type ScopeCache = Option<(ITfContext, Rc<Cell<Option<bool>>>)>;
static MODULE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn module_path() -> Result<String> {
    let module = HMODULE(MODULE.load(Ordering::SeqCst));
    let mut buffer = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if len == 0 || len >= buffer.len() {
        return Err(E_FAIL.into());
    }
    Ok(String::from_utf16_lossy(&buffer[..len]))
}

fn running_in_editor() -> bool {
    static EDITOR: OnceLock<bool> = OnceLock::new();
    *EDITOR.get_or_init(|| {
        let mut buffer = [0u16; 1024];
        let len = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
        len > 0
            && len < buffer.len()
            && std::path::Path::new(&String::from_utf16_lossy(&buffer[..len]))
                .file_name()
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case(EDITOR_EXE)
                        || name.eq_ignore_ascii_case(LEGACY_EDITOR_EXE)
                })
    })
}

/// Copies the switches from `HKCU\Software\InputTranslation` (0.1.x) to
/// `HKCU\Software\YiIME` the first time; the EXE does the same, whichever runs first.
fn migrate_settings_key() {
    if CURRENT_USER.open(STATE_KEY).is_ok() {
        return;
    }
    let Ok(old) = CURRENT_USER.open(LEGACY_STATE_KEY) else {
        return;
    };
    let Ok(new) = CURRENT_USER.create(STATE_KEY) else {
        return;
    };
    if let Ok(values) = old.values() {
        for (name, value) in values {
            let _ = new.set_value(&name, &value);
        }
    }
}

fn com_scope(f: impl FnOnce() -> Result<()>) -> Result<()> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let result = f();
    if initialized {
        unsafe {
            CoUninitialize();
        }
    }
    result
}

/// Prevent a Rust unwind from crossing a COM ABI boundary into a host application.
fn com_guard<T>(entry: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log(&format!("panic in {entry}")));
            Err(E_FAIL.into())
        }
    }
}

fn absent_is_ok(result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error)
            if [
                TF_E_NOOBJECT,
                HRESULT(0x80070002u32 as i32),
                HRESULT(0x80070003u32 as i32),
            ]
            .contains(&error.code()) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Registered TIP categories. SYSTRAYSUPPORT is required for the Win8+ taskbar input indicator to
/// show this IME's mode icon (GUID_LBI_INPUTMODE) and forward clicks to it; without it only the
/// brand icon appears. IMMERSIVESUPPORT is not registered: UWP/AppContainer hosts cannot launch the
/// input-box EXE, write HKCU or message across integrity levels, so claiming support would break
/// those apps (Microsoft documents SYSTRAYSUPPORT for IMEs that are not immersive but work in the
/// system tray).
const CATEGORIES: [GUID; 4] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_COMLESS,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];
/// Small diagnostic log: %TEMP%\YiIME-tsf.log (rotated to .old over 512 KB); ignored where it
/// cannot be written (AppContainer etc.).
fn log(message: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("YiIME-tsf.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 512 * 1024) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let mut module = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(None, &mut module) } as usize;
    let host = String::from_utf16_lossy(&module[..len.min(module.len())]);
    let host = host.rsplit('\\').next().unwrap_or("").to_owned();
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(
            file,
            "{elapsed} pid={} {host}: {message}",
            std::process::id()
        );
    }
}
#[implement(IClassFactory)]
struct ClassFactory;
impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> Result<()> {
        com_guard("CreateInstance", || {
            if !outer.is_null() {
                return CLASS_E_NOAGGREGATION.ok();
            }
            if iid.is_null() || out.is_null() {
                return E_FAIL.ok();
            }
            let object: IUnknown = TextService::new().into();
            unsafe { object.query(iid, out).ok() }
        })
    }
    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        com_guard("LockServer", || Ok(()))
    }
}

#[implement(ITfEditSession)]
struct CommitEdit {
    context: ITfContext,
    text: String,
}
impl ITfEditSession_Impl for CommitEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("DoEditSession", || {
            let insert: ITfInsertAtSelection = self.context.cast()?;
            let utf16: Vec<u16> = self.text.encode_utf16().collect();
            let range = unsafe {
                insert.InsertTextAtSelection(cookie, INSERT_TEXT_AT_SELECTION_FLAGS(0), &utf16)?
            };
            let caret = unsafe { range.Clone()? };
            unsafe {
                caret.Collapse(cookie, TF_ANCHOR_END)?;
            }
            let selection = TF_SELECTION {
                range: ManuallyDrop::new(Some(caret)),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_END,
                    fInterimChar: false.into(),
                },
            };
            let result = unsafe {
                self.context
                    .SetSelection(cookie, std::slice::from_ref(&selection))
            };
            drop(ManuallyDrop::into_inner(selection.range));
            result
        })
    }
}

#[implement(ITfEditSession)]
struct ScopeCheck {
    context: ITfContext,
    result: Rc<Cell<Option<bool>>>,
}
impl ITfEditSession_Impl for ScopeCheck_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("DoEditSession", || {
            let mut selection: TF_SELECTION = unsafe { std::mem::zeroed() };
            let mut fetched = 0;
            let selected = unsafe {
                self.context.GetSelection(
                    cookie,
                    TF_DEFAULT_SELECTION,
                    std::slice::from_mut(&mut selection),
                    &mut fetched,
                )
            };
            if selected.is_err() || fetched == 0 {
                // If the selection cannot be read, treat it as a normal text field; otherwise the IME
                // would never activate here.
                drop(ManuallyDrop::into_inner(selection.range));
                unsafe {
                    OutputDebugStringW(wide("YiIME: 读取选区失败，按非密码输入框处理").as_ptr());
                }
                self.result.set(Some(false));
                return Ok(());
            }
            let mut password = false;
            let range = ManuallyDrop::into_inner(selection.range);
            if let Some(range) = range
                && let Ok(property) = unsafe { self.context.GetAppProperty(&GUID_PROP_INPUTSCOPE) }
                && let Ok(mut value) = unsafe { property.GetValue(cookie, &range) }
            {
                let inner = unsafe { &*value.Anonymous.Anonymous };
                if inner.vt.0 == 13 {
                    let unknown = unsafe { &*inner.Anonymous.punkVal };
                    if let Some(unknown) = unknown.as_ref()
                        && let Ok(scope) = unknown.cast::<ITfInputScope>()
                    {
                        let mut ptr = null_mut();
                        let mut count = 0;
                        let read = unsafe { scope.GetInputScopes(&mut ptr, &mut count) }.is_ok();
                        if read && !ptr.is_null() {
                            let scopes = unsafe { std::slice::from_raw_parts(ptr, count as usize) };
                            password = scopes.iter().any(|scope| {
                                [
                                    IS_PASSWORD,
                                    IS_NUMERIC_PIN,
                                    IS_ALPHANUMERIC_PIN,
                                    IS_ALPHANUMERIC_PIN_SET,
                                ]
                                .contains(scope)
                            });
                        }
                        if !ptr.is_null() {
                            unsafe {
                                CoTaskMemFree(Some(ptr as _));
                            }
                        }
                    }
                }
                let _ = unsafe { VariantClear(&mut value) };
            }
            self.result.set(Some(password));
            Ok(())
        })
    }
}

/// Reads the screen rectangle of the insertion point so the editor can place its box at the
/// target's cursor.
#[implement(ITfEditSession)]
struct CaretQuery {
    context: ITfContext,
    result: Rc<Cell<Option<RECT>>>,
}
impl ITfEditSession_Impl for CaretQuery_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("DoEditSession", || {
            let mut selection: TF_SELECTION = unsafe { std::mem::zeroed() };
            let mut fetched = 0;
            let selected = unsafe {
                self.context.GetSelection(
                    cookie,
                    TF_DEFAULT_SELECTION,
                    std::slice::from_mut(&mut selection),
                    &mut fetched,
                )
            };
            let range = ManuallyDrop::into_inner(selection.range);
            selected?;
            let Some(range) = range.filter(|_| fetched > 0) else {
                return Ok(());
            };
            let view = unsafe { self.context.GetActiveView()? };
            let mut rect = RECT::default();
            let mut clipped = BOOL::default();
            unsafe {
                range.Collapse(cookie, TF_ANCHOR_END)?;
                view.GetTextExt(cookie, &range, &mut rect, &mut clipped)?;
            }
            if rect.bottom > rect.top && rect.right >= rect.left {
                self.result.set(Some(rect));
            }
            Ok(())
        })
    }
}

/// Chinese/English mode of the thread (GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, 1 = Chinese).
fn keyboard_mode(manager: &ITfThreadMgr) -> Option<bool> {
    let compartment = unsafe {
        manager
            .cast::<ITfCompartmentMgr>()
            .ok()?
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)
            .ok()?
    };
    let mut value = unsafe { compartment.GetValue() }.ok()?;
    let inner = unsafe { &*value.Anonymous.Anonymous };
    let result = (inner.vt == VT_I4).then_some(unsafe { inner.Anonymous.lVal != 0 });
    let _ = unsafe { VariantClear(&mut value) };
    result
}
fn set_keyboard_mode(manager: &ITfThreadMgr, client_id: u32, chinese: bool) -> bool {
    let Ok(compartment) = (unsafe {
        manager
            .cast::<ITfCompartmentMgr>()
            .and_then(|manager| manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE))
    }) else {
        return false;
    };
    let mut value = VARIANT::default();
    let inner = unsafe { &mut *value.Anonymous.Anonymous };
    inner.vt = VT_I4;
    inner.Anonymous.lVal = i32::from(chinese);
    let ok = unsafe { compartment.SetValue(client_id, &value).is_ok() };
    // Also keep the NATIVE bit of the conversion-mode compartment in sync (other bits untouched):
    // the taskbar and touch keyboard read Chinese/English from it.
    if let Some(mode) = conversion_mode(manager) {
        let native = TF_CONVERSIONMODE_NATIVE as i32;
        let next = if chinese {
            mode | native
        } else {
            mode & !native
        };
        if next != mode {
            set_conversion_mode(manager, client_id, next);
        }
    }
    ok
}
fn conversion_compartment(
    manager: &ITfThreadMgr,
) -> Option<windows::Win32::UI::TextServices::ITfCompartment> {
    unsafe {
        manager
            .cast::<ITfCompartmentMgr>()
            .ok()?
            .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)
            .ok()
    }
}
/// Conversion mode; unset counts as Chinese (NATIVE).
fn conversion_mode(manager: &ITfThreadMgr) -> Option<i32> {
    let compartment = conversion_compartment(manager)?;
    let mut value = unsafe { compartment.GetValue() }.ok()?;
    let inner = unsafe { &*value.Anonymous.Anonymous };
    let result = if inner.vt == VT_I4 {
        unsafe { inner.Anonymous.lVal }
    } else {
        TF_CONVERSIONMODE_NATIVE as i32
    };
    let _ = unsafe { VariantClear(&mut value) };
    Some(result)
}
fn set_conversion_mode(manager: &ITfThreadMgr, client_id: u32, mode: i32) {
    let Some(compartment) = conversion_compartment(manager) else {
        return;
    };
    let mut value = VARIANT::default();
    let inner = unsafe { &mut *value.Anonymous.Anonymous };
    inner.vt = VT_I4;
    inner.Anonymous.lVal = mode;
    let _ = unsafe { compartment.SetValue(client_id, &value) };
}
/// Input-box/tray EXE next to this DLL, or else in the current version directory recorded by the
/// installer.
fn companion_exe() -> Option<std::path::PathBuf> {
    let path = module_path().ok()?;
    let editor = std::path::Path::new(&path).parent()?.join(EDITOR_EXE);
    if editor.is_file() {
        return Some(editor);
    }
    LOCAL_MACHINE
        .open(STATE_KEY)
        .and_then(|key| key.get_string("CurrentDir"))
        .ok()
        .map(|dir| std::path::PathBuf::from(dir).join(EDITOR_EXE))
        .filter(|path| path.is_file())
}
/// Process id owning `window` (0 if none).
fn window_pid(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    pid
}
/// Full path of a process's executable.
fn process_image(pid: u32) -> Option<std::path::PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
                != 0;
        CloseHandle(process);
        ok.then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize])))
    }
}
thread_local! {
    /// Receiver window of this thread's text service, so the input-mode button (which sees every
    /// OPENCLOSE change: Shift tap, Ctrl+Space, the button, the taskbar) can reach it.
    static SESSION_RECEIVER: Cell<HWND> = const { Cell::new(null_mut()) };
}
/// The application switched to English (from any source): tell this thread's text service,
/// which tells the input box (asynchronously, never from inside the compartment callback).
pub(super) fn notify_english_mode() {
    let receiver = SESSION_RECEIVER.with(Cell::get);
    if !receiver.is_null() {
        unsafe {
            let _ = PostMessageW(receiver, WM_APP + 9, 0, 0);
        }
    }
}
impl TextService {
    /// English is on: an input box still showing for this application ends the session (it is
    /// hidden; a draft is kept for next time). WM_APP + 9, wParam = receiver.
    fn notify_box_english(&self) {
        let editor_window = self.editor_window.get();
        let receiver = self.receiver.get();
        if !editor_window.is_null() && !receiver.is_null() {
            log("切到英文：通知输入框结束会话");
            unsafe {
                let _ = PostMessageW(editor_window, WM_APP + 9, receiver as usize, 0);
            }
        }
    }
    /// Ends this application's input session on the DLL side because the focus moved to another
    /// text field, and tells the box to close (WM_APP + 8 with lParam 1). The context is kept, so
    /// a translation still in flight lands where it was started unless a new session begins
    /// (the box then drops the stale result).
    fn drop_stale_session(&self, reason: &str) {
        if !self.buffering() {
            return;
        }
        log(&format!("结束旧的输入会话：{reason}"));
        self.kill_timers();
        self.editor_open.set(false);
        self.pending_keys.borrow_mut().clear();
        self.first_key.set(None);
        self.flush_attempts.set(0);
        let editor_window = self.editor_window.get();
        let receiver = self.receiver.get();
        if !editor_window.is_null() && !receiver.is_null() {
            unsafe {
                let _ = PostMessageW(editor_window, WM_APP + 8, receiver as usize, 1);
            }
        }
    }
    /// Tells the shared input box that the focus in this application moved (another text field,
    /// the page, another window), so an empty box is hidden at once instead of lingering
    /// (WM_APP + 8, wParam = receiver; the box ignores it when it has content or the keyboard).
    fn notify_focus_moved(&self) {
        let editor_window = self.editor_window.get();
        let receiver = self.receiver.get();
        if !editor_window.is_null() && !receiver.is_null() {
            unsafe {
                let _ = PostMessageW(editor_window, WM_APP + 8, receiver as usize, 0);
            }
        }
    }
}
/// A running shared input-box service started from `exe` that this application may use: its
/// window and process id. Windows of other executables (another version, or anything imitating
/// the title) and services of another user, session or integrity level, or elevated ones, are
/// ignored (see yiime_core::may_share_service).
fn find_editor_service(
    exe: &std::path::Path,
    host: &yiime_core::ProcessSecurity,
) -> Option<(HWND, u32)> {
    let class = wide("InputTranslationWindow");
    let title = wide(&yiime_core::service_window_title(host.integrity, false));
    let expected = exe.to_string_lossy().to_lowercase();
    let mut window: HWND = null_mut();
    loop {
        window = unsafe { FindWindowExW(null_mut(), window, class.as_ptr(), title.as_ptr()) };
        if window.is_null() {
            return None;
        }
        let pid = window_pid(window);
        if pid != 0
            && pid != std::process::id()
            && process_image(pid)
                .is_some_and(|image| image.to_string_lossy().to_lowercase() == expected)
            && yiime_core::may_share_service(
                host,
                crate::process_security::process_security(pid).as_ref(),
            )
        {
            return Some((window, pid));
        }
    }
}
/// Ends a hung editor service: only process `pid` running `exe` (the same check as
/// [`find_editor_service`]), never anything else.
fn terminate_service(pid: u32, exe: &std::path::Path) -> bool {
    let expected = exe.to_string_lossy().to_lowercase();
    if pid == 0
        || pid == std::process::id()
        || process_image(pid).is_none_or(|image| image.to_string_lossy().to_lowercase() != expected)
    {
        return false;
    }
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return false;
        }
        let ok = TerminateProcess(process, 1) != 0;
        CloseHandle(process);
        ok
    }
}
/// Asks the tray instance to open settings (WM_APP+5) or history (WM_APP+6), starting the EXE with
/// an argument if the tray is not running.
fn open_tray_window(history: bool) {
    unsafe {
        AllowSetForegroundWindow(u32::MAX); // ASFW_ANY: the menu click gave us the foreground right; pass it to the tray process.
    }
    let tray = unsafe {
        FindWindowW(
            wide("InputTranslationWindow").as_ptr(),
            wide("YiIME 译 · 托盘").as_ptr(),
        )
    };
    if !tray.is_null() {
        unsafe {
            PostMessageW(tray, WM_APP + if history { 6 } else { 5 }, 0, 0);
        }
        return;
    }
    if let Some(exe) = companion_exe() {
        let directory = exe.parent().map(std::path::Path::to_path_buf);
        let mut command = std::process::Command::new(&exe);
        command.arg(if history { "--history" } else { "--settings" });
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        if let Err(error) = command.spawn() {
            log(&format!("启动托盘失败：{error}"));
        }
    }
}
/// Longest Shift hold that counts as a Chinese/English tap.
const SHIFT_TAP_MAX: std::time::Duration = std::time::Duration::from_millis(600);

fn is_shift(key: usize) -> bool {
    matches!(key, 0x10 | 0xA0 | 0xA1)
}
fn caps_lock() -> bool {
    (unsafe { GetKeyState(VK_CAPITAL as i32) } & 1) != 0
}
/// Buffered letters keep their case (Shift XOR Caps Lock): lowercase composes, uppercase goes into
/// the draft as is.
fn typed_letter(key: u8) -> char {
    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
    let letter = key as char;
    if shift != caps_lock() {
        letter.to_ascii_uppercase()
    } else {
        letter.to_ascii_lowercase()
    }
}
const LANGBAR_SINK_COOKIE: u32 = 0x4954_4c42;
/// Mode button added to the language bar, with its OPENCLOSE compartment advise cookies.
type ModeButton = Option<(ITfLangBarItemButton, Vec<(ITfSource, u32)>, LangBarSink)>;

#[implement(
    ITfTextInputProcessor,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfTextEditSink
)]
struct TextService {
    thread_manager: RefCell<Option<ITfThreadMgr>>,
    context: RefCell<Option<ITfContext>>,
    client_id: Cell<u32>,
    receiver: Cell<HWND>,
    editor_open: Cell<bool>,
    /// The editor process this instance started (None when it uses a service found running).
    editor: RefCell<Option<std::process::Child>>,
    /// Process id of the editor service in use: commits are accepted only from it.
    editor_pid: Cell<u32>,
    editor_window: Cell<HWND>,
    pending_keys: RefCell<String>,
    first_key: Cell<Option<char>>,
    flush_attempts: Cell<u8>,
    /// Timeouts of the shared service and restart backoff (see yiime_core::ServiceHealth).
    health: RefCell<yiime_core::ServiceHealth>,
    /// This application is elevated / high integrity (or its token is unreadable): it uses a
    /// private input box, never the shared service. Computed on first use.
    private: Cell<Option<bool>>,
    scope_cache: RefCell<ScopeCache>,
    manager_sink_cookie: Cell<Option<u32>>,
    edit_sink: RefCell<Option<(ITfContext, u32)>>,
    tested_key: RefCell<Option<(ITfContext, usize, bool)>>,
    mode_button: RefCell<ModeButton>,
    /// When Shift went down alone; cleared by any other key. If still set and short enough on
    /// release, toggle Chinese/English.
    shift_down: Cell<Option<std::time::Instant>>,
    /// Plain-mode preserved key registered at activation (unregistering needs the same chord).
    plain_chord: Cell<Option<TF_PRESERVEDKEY>>,
    history_chord: Cell<Option<TF_PRESERVEDKEY>>,
    /// Whether the last key passed to the application was a digit (keeps `.` `:` half-width).
    after_digit: Cell<bool>,
    quotes: Cell<Quotes>,
    /// Plain-mode state last shown by the button; refreshed when the tray or another app changes
    /// it.
    shown_plain: Cell<bool>,
}
impl TextService {
    fn new() -> Self {
        Self {
            thread_manager: RefCell::new(None),
            context: RefCell::new(None),
            client_id: Cell::new(0),
            receiver: Cell::new(null_mut()),
            editor_open: Cell::new(false),
            editor: RefCell::new(None),
            editor_pid: Cell::new(0),
            editor_window: Cell::new(null_mut()),
            pending_keys: RefCell::new(String::new()),
            first_key: Cell::new(None),
            flush_attempts: Cell::new(0),
            health: RefCell::new(yiime_core::ServiceHealth::default()),
            private: Cell::new(None),
            scope_cache: RefCell::new(None),
            manager_sink_cookie: Cell::new(None),
            edit_sink: RefCell::new(None),
            tested_key: RefCell::new(None),
            mode_button: RefCell::new(None),
            shift_down: Cell::new(None),
            plain_chord: Cell::new(None),
            history_chord: Cell::new(None),
            after_digit: Cell::new(false),
            quotes: Cell::new(Quotes::default()),
            shown_plain: Cell::new(false),
        }
    }
    fn refresh_editor(&self) {
        let exited = self
            .editor
            .borrow_mut()
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some());
        // A shared service started by another application may have exited (idle timeout).
        let window = self.editor_window.get();
        let gone =
            self.editor.borrow().is_none() && !window.is_null() && unsafe { IsWindow(window) } == 0;
        if exited || gone {
            self.reset_editor();
        }
    }
    fn reset_editor(&self) {
        self.kill_timers();
        self.editor_open.set(false);
        self.editor.borrow_mut().take();
        self.editor_pid.set(0);
        self.editor_window.set(null_mut());
        self.pending_keys.borrow_mut().clear();
        self.first_key.set(None);
        self.flush_attempts.set(0);
        self.context.borrow_mut().take();
    }
    fn kill_timers(&self) {
        if !self.receiver.get().is_null() {
            unsafe {
                KillTimer(self.receiver.get(), FLUSH_TIMER);
                KillTimer(self.receiver.get(), READY_TIMER);
            }
        }
    }
    /// An input session is running (editor starting or box shown): letters typed in the host are
    /// buffered and forwarded to the box, appended to its code, instead of reactivating and
    /// clearing it on every key. The session closes only when the editor reports that it ended
    /// (commit, cancel, focus loss).
    fn buffering(&self) -> bool {
        self.editor_open.get()
    }
    /// Lets the editor bring its box to the foreground (only the current foreground process, the
    /// host, can grant this).
    fn allow_editor_foreground(&self) {
        let pid = self.editor_pid.get();
        if pid != 0 {
            unsafe {
                let _ = AllowSetForegroundWindow(pid);
            }
        }
    }
    /// Reads the insertion point's screen rectangle synchronously; only where the document lock is
    /// available, e.g. key handling.
    fn caret_rect(&self, context: &ITfContext) -> Option<RECT> {
        let result = Rc::new(Cell::new(None));
        let session: ITfEditSession = CaretQuery {
            context: context.clone(),
            result: result.clone(),
        }
        .into();
        unsafe {
            context.RequestEditSession(self.client_id.get(), &session, TF_ES_SYNC | TF_ES_READ)
        }
        .and_then(|status| status.ok())
        .ok()
        .and_then(|_| result.get())
    }
    fn compartment_on(manager: &ITfCompartmentMgr, id: &GUID) -> bool {
        let Ok(compartment) = (unsafe { manager.GetCompartment(id) }) else {
            return false;
        };
        let Ok(value) = (unsafe { compartment.GetValue() }) else {
            return false;
        };
        let inner = unsafe { &*value.Anonymous.Anonymous };
        inner.vt.0 == 3 && unsafe { inner.Anonymous.lVal != 0 }
    }
    fn disabled(context: &ITfContext) -> bool {
        unsafe { context.GetStatus() }
            .is_ok_and(|status| status.dwDynamicFlags & TF_SD_READONLY != 0)
            || context.cast::<ITfCompartmentMgr>().is_ok_and(|manager| {
                Self::compartment_on(&manager, &GUID_COMPARTMENT_KEYBOARD_DISABLED)
                    || Self::compartment_on(&manager, &GUID_COMPARTMENT_EMPTYCONTEXT)
            })
    }
    fn password_scope(&self, context: &ITfContext) -> bool {
        let focus = unsafe { GetFocus() };
        if !focus.is_null() {
            let mut class = [0u16; 16];
            let len =
                unsafe { GetClassNameW(focus, class.as_mut_ptr(), class.len() as i32) } as usize;
            if String::from_utf16_lossy(&class[..len]).eq_ignore_ascii_case("Edit")
                && unsafe { GetWindowLongW(focus, GWL_STYLE) } & ES_PASSWORD != 0
            {
                return true;
            }
        }
        self.scope_cache
            .borrow()
            .as_ref()
            .filter(|(cached, _)| cached == context)
            .and_then(|(_, result)| result.get())
            .unwrap_or(true)
    }
    fn wants_key(&self, context: &ITfContext, key: usize) -> bool {
        if running_in_editor() {
            return false;
        }
        self.refresh_editor();
        let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
        if !(0x41..=0x5a).contains(&key) || Self::disabled(context) {
            return false;
        }
        if !self.keyboard_open() {
            return false;
        }
        let same_context = self
            .context
            .borrow()
            .as_ref()
            .is_some_and(|active| active == context);
        match yiime_core::key_route(self.buffering(), same_context) {
            yiime_core::KeyRoute::Buffer => return true,
            // The box still belongs to another text field: drop that session so this letter
            // starts a fresh one instead of being refused.
            yiime_core::KeyRoute::StaleSession => self.drop_stale_session("按键来自另一个输入框"),
            yiime_core::KeyRoute::NewSession => {}
        }
        // No session: plain letters start one. Shift+letter and letters under Caps Lock go to the
        // application as usual (capitals typed directly, no box, as in Microsoft Wubi).
        !caps_lock()
            && !modifier(VK_CONTROL)
            && !modifier(VK_MENU)
            && !modifier(VK_LWIN)
            && !modifier(VK_RWIN)
            && !modifier(VK_SHIFT)
    }
    /// The Chinese punctuation this key produces when the box is closed, Chinese mode is on and
    /// Chinese punctuation is enabled.
    fn punctuation(&self, context: &ITfContext, key: usize) -> Option<String> {
        let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
        if running_in_editor()
            || self.buffering()
            || modifier(VK_CONTROL)
            || modifier(VK_MENU)
            || modifier(VK_LWIN)
            || modifier(VK_RWIN)
            || !self.keyboard_open()
            || option(Flag::PunctEnglish)
            || Self::disabled(context)
            || self.password_scope(context)
        {
            return None;
        }
        let shift = modifier(VK_SHIFT);
        if key == 0xDE {
            return self
                .quotes
                .get()
                .peek(if shift { '"' } else { '\'' })
                .map(str::to_owned);
        }
        chinese_punctuation(key, shift, self.after_digit.get()).map(str::to_owned)
    }
    /// Inserts text straight into the document (without the box).
    fn insert_direct(&self, context: &ITfContext, text: String) -> bool {
        let session: ITfEditSession = CommitEdit {
            context: context.clone(),
            text,
        }
        .into();
        unsafe { context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE) }
            .is_ok()
    }
    fn should_consume(&self, context: &ITfContext, key: usize) -> bool {
        self.wants_key(context, key) && (self.buffering() || !self.password_scope(context))
    }
    fn keyboard_open(&self) -> bool {
        let manager = self.thread_manager.borrow();
        let Some(manager) = manager.as_ref() else {
            return true;
        };
        let Ok(manager) = manager.cast::<ITfCompartmentMgr>() else {
            return true;
        };
        let Ok(compartment) =
            (unsafe { manager.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) })
        else {
            return true;
        };
        let Ok(mut value) = (unsafe { compartment.GetValue() }) else {
            return true;
        };
        let inner = unsafe { &*value.Anonymous.Anonymous };
        let open = inner.vt.0 != 3 || unsafe { inner.Anonymous.lVal != 0 };
        let _ = unsafe { VariantClear(&mut value) };
        open
    }
    /// Always start in Chinese mode when switching to this IME. The OPENCLOSE compartment is shared
    /// by all IMEs on the thread; inheriting another IME's English state would send keys straight
    /// to the application and never open the box.
    fn ensure_keyboard_mode(manager: &ITfThreadMgr, client_id: u32) {
        set_keyboard_mode(manager, client_id, true);
    }
    fn toggle_keyboard_mode(&self) -> bool {
        let manager = self.thread_manager.borrow();
        let Some(manager) = manager.as_ref() else {
            return false;
        };
        // The mode button listens to the compartment and refreshes itself.
        set_keyboard_mode(manager, self.client_id.get(), !self.keyboard_open())
    }
    /// Tracks Shift pressed alone: remember when it went down; any other key cancels.
    fn track_key_down(&self, key: usize) {
        if is_shift(key) {
            let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
            if modifier(VK_CONTROL) || modifier(VK_MENU) || modifier(VK_LWIN) || modifier(VK_RWIN) {
                self.shift_down.set(None);
            } else if self.shift_down.get().is_none() {
                // Auto-repeat does not reset the time.
                self.shift_down.set(Some(std::time::Instant::now()));
            }
        } else {
            self.shift_down.set(None);
        }
    }
    /// Shift released: if no other key was pressed and it was short enough, toggle Chinese/English
    /// (through the OPENCLOSE compartment, like Ctrl+Space and the language-bar button). The box's
    /// draft and code are unaffected.
    fn track_key_up(&self, key: usize) {
        if !is_shift(key) {
            return;
        }
        if let Some(start) = self.shift_down.take()
            && start.elapsed() <= SHIFT_TAP_MAX
            && !running_in_editor()
        {
            let ok = self.toggle_keyboard_mode();
            log(&format!(
                "Shift 单击切换中/英：{ok}，现在中文={}",
                self.keyboard_open()
            ));
        }
    }
    /// Chinese → English (OPENCLOSE off), for a Shift tap in the empty input box. True when the
    /// application is in English afterwards.
    fn switch_to_english(&self) -> bool {
        let ok = !self.keyboard_open() || self.toggle_keyboard_mode();
        log(&format!("输入框里 Shift 单击切换到英文：{ok}"));
        ok
    }
    fn refresh_mode_button(&self) {
        if let Some((_, _, sink)) = self.mode_button.borrow().as_ref() {
            refresh_sink(sink);
        }
    }
    /// Plain/translation mode may have changed elsewhere (tray menu, shortcut in another app):
    /// refresh the button.
    fn sync_plain_mode(&self) {
        let plain = plain_mode();
        if self.shown_plain.replace(plain) != plain {
            self.refresh_mode_button();
        }
    }
    fn toggle_plain_mode(&self) -> bool {
        let plain = !plain_mode();
        if !set_plain_mode(plain) {
            return false;
        }
        self.shown_plain.set(plain);
        // Plain mode is also Chinese input: switching to it from English turns Chinese on.
        if !self.keyboard_open()
            && let Some(manager) = self.thread_manager.borrow().as_ref()
        {
            set_keyboard_mode(manager, self.client_id.get(), true);
        }
        self.refresh_mode_button();
        true
    }
    fn add_mode_button(&self, manager: &ITfThreadMgr, client_id: u32) {
        let sink: LangBarSink = Rc::new(RefCell::new(None));
        let button: ITfLangBarItemButton = InputModeButton {
            manager: manager.clone(),
            client_id,
            sink: sink.clone(),
        }
        .into();
        // Watch both OPENCLOSE (Ctrl+Space, Shift, click) and conversion mode (taskbar, touch
        // keyboard).
        let mut advices = Vec::new();
        if let Ok(event_sink) = button.cast::<ITfCompartmentEventSink>()
            && let Ok(compartments) = manager.cast::<ITfCompartmentMgr>()
        {
            for guid in [
                GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
                GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
            ] {
                if let Ok(source) = unsafe { compartments.GetCompartment(&guid) }
                    .and_then(|compartment| compartment.cast::<ITfSource>())
                    && let Ok(cookie) =
                        unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, &event_sink) }
                {
                    advices.push((source, cookie));
                }
            }
        }
        match manager
            .cast::<ITfLangBarItemMgr>()
            .and_then(|items| unsafe { items.AddItem(&button) })
        {
            Ok(()) => {
                log("模式按钮已加入语言栏");
                *self.mode_button.borrow_mut() = Some((button, advices, sink));
                self.shown_plain.set(plain_mode());
            }
            Err(error) => {
                log(&format!("模式按钮加入语言栏失败：{error}"));
                Self::unadvise_mode(advices);
            }
        }
    }
    fn unadvise_mode(advices: Vec<(ITfSource, u32)>) {
        for (source, cookie) in advices {
            let _ = unsafe { source.UnadviseSink(cookie) };
        }
    }
    fn remove_mode_button(&self, manager: &ITfThreadMgr) {
        if let Some((button, advices, _)) = self.mode_button.borrow_mut().take() {
            if let Ok(items) = manager.cast::<ITfLangBarItemMgr>() {
                let _ = unsafe { items.RemoveItem(&button) };
            }
            Self::unadvise_mode(advices);
        }
    }
    /// Whether this application needs a private input box (elevated or high integrity).
    fn private_editor(&self) -> bool {
        if let Some(private) = self.private.get() {
            return private;
        }
        let private =
            yiime_core::needs_private_editor(crate::process_security::process_security(0).as_ref());
        if private {
            log("提升权限的程序：使用独立输入框，不连接共享服务");
        }
        self.private.set(Some(private));
        private
    }
    fn open_editor(&self, context: ITfContext, first_key: u8) -> bool {
        let target = unsafe { GetForegroundWindow() } as usize;
        // Keys buffered for the previous session do not carry over.
        self.pending_keys.borrow_mut().clear();
        let caret = self.caret_rect(&context);
        // The input box is one resident service shared by all applications: reuse the one this
        // instance already talks to, else one found running (same EXE), else start it.
        let Some(editor) = companion_exe() else {
            return false;
        };
        let mut window = self.editor_window.get();
        if window.is_null()
            || unsafe { IsWindow(window) } == 0
            || window_pid(window) != self.editor_pid.get()
        {
            window = null_mut();
            let host = crate::process_security::process_security(0);
            if self.editor.borrow().is_none()
                && !self.private_editor()
                && let Some(host) = host.as_ref()
                && let Some((found, pid)) = find_editor_service(&editor, host)
            {
                self.editor_pid.set(pid);
                self.editor_window.set(found);
                window = found;
            }
        }
        // A service that timed out earlier is restarted now (with backoff); while backing off the
        // input box is not opened and keys go to the application.
        let running = (!window.is_null()).then(|| window_pid(window));
        let decision = self
            .health
            .borrow_mut()
            .before_activation(running, std::time::Instant::now());
        match decision {
            yiime_core::Activation::Use => {}
            yiime_core::Activation::Wait => {
                log("输入框服务无响应，等待后再重启");
                return false;
            }
            yiime_core::Activation::Restart { kill } => {
                let owned = self
                    .editor
                    .borrow()
                    .as_ref()
                    .is_some_and(|child| child.id() == kill);
                let killed = if owned {
                    self.editor
                        .borrow_mut()
                        .as_mut()
                        .is_some_and(|child| child.kill().is_ok())
                } else {
                    terminate_service(kill, &editor)
                };
                log(&format!("输入框服务无响应，已结束并重启：{killed}"));
                self.reset_editor();
                window = null_mut();
            }
        }
        if !window.is_null() {
            let pid = window_pid(window);
            self.allow_editor_foreground();
            *self.context.borrow_mut() = Some(context.clone());
            self.first_key.set(
                first_key
                    .is_ascii_alphabetic()
                    .then(|| (first_key as char).to_ascii_lowercase()),
            );
            self.editor_open.set(true);
            self.editor_window.set(null_mut());
            // Send the cursor rectangle synchronously first (an empty packet if unavailable, so the
            // editor positions itself); it also makes this receiver the service's current one.
            // Then activate asynchronously.
            let mut rect = caret.map(|rect| [rect.left, rect.top, rect.right, rect.bottom]);
            let packet = COPYDATASTRUCT {
                dwData: 3,
                cbData: if rect.is_some() { 16 } else { 0 },
                lpData: rect
                    .as_mut()
                    .map_or(null_mut(), |rect| rect.as_mut_ptr() as _),
            };
            let mut result = 0usize;
            let answered = unsafe {
                SendMessageTimeoutW(
                    window,
                    WM_COPYDATA,
                    self.receiver.get() as usize,
                    &packet as *const _ as isize,
                    SMTO_ABORTIFHUNG,
                    yiime_core::REQUEST_TIMEOUT_MS,
                    &mut result,
                ) != 0
            };
            if !answered {
                // Hung or busy: do not block the application; restart it on the next activation.
                self.health.borrow_mut().timed_out(pid);
                log("输入框服务请求超时");
                self.reset_editor();
                return false;
            }
            self.health.borrow_mut().responded(pid);
            let accepted = result != 0;
            if accepted
                && unsafe { PostMessageW(window, WM_APP + 2, target, first_key as isize) } != 0
            {
                self.start_ready_timer();
                return true;
            }
            if let Some(child) = self.editor.borrow_mut().as_mut() {
                let _ = child.kill();
            }
            self.reset_editor();
        }
        let Ok(path) = module_path() else {
            return false;
        };
        let Some(directory) = std::path::Path::new(&path).parent() else {
            return false;
        };
        let directory = editor.parent().unwrap_or(directory).to_path_buf();
        let mut command = std::process::Command::new(editor);
        command
            .args([
                "--tsf-hwnd",
                &format!("{}", self.receiver.get() as usize),
                "--target-hwnd",
                &format!("{target}"),
                "--initial",
                &if first_key == RECALL {
                    "-".to_owned()
                } else {
                    format!("{}", first_key as char)
                },
            ])
            // Do not inherit the host's working directory or standard handles.
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if first_key == RECALL {
            command.arg("--recall");
        }
        if self.private_editor() {
            // Serves only this application and exits with it.
            command.arg("--private");
        }
        if let Some(rect) = caret {
            command.args([
                "--caret".to_owned(),
                format!("{},{},{},{}", rect.left, rect.top, rect.right, rect.bottom),
            ]);
        }
        if let Ok(child) = command.spawn() {
            *self.context.borrow_mut() = Some(context);
            self.first_key.set(
                first_key
                    .is_ascii_alphabetic()
                    .then(|| (first_key as char).to_ascii_lowercase()),
            );
            self.editor_pid.set(child.id());
            *self.editor.borrow_mut() = Some(child);
            self.allow_editor_foreground();
            self.editor_open.set(true);
            self.start_ready_timer();
            true
        } else {
            false
        }
    }
    fn start_ready_timer(&self) {
        unsafe {
            SetTimer(self.receiver.get(), READY_TIMER, READY_TIMEOUT_MS, None);
        }
    }
    fn commit(&self, text: String) -> bool {
        if text.is_empty() {
            self.finish_editor();
            return true;
        }
        let context = self.context.borrow().clone();
        let Some(context) = context else {
            return false;
        };
        let session: ITfEditSession = CommitEdit {
            context: context.clone(),
            text,
        }
        .into();
        if unsafe { context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE) }
            .and_then(|status| status.ok())
            .is_err()
        {
            return false;
        }
        self.finish_editor();
        true
    }
    fn finish_editor(&self) {
        self.editor_open.set(false);
        self.context.borrow_mut().take();
        self.pending_keys.borrow_mut().clear();
        self.first_key.set(None);
        self.flush_attempts.set(0);
        self.kill_timers();
    }
    fn editor_ready(&self, hwnd: HWND) -> bool {
        self.editor_window.set(hwnd);
        if !self.receiver.get().is_null() {
            unsafe {
                KillTimer(self.receiver.get(), READY_TIMER);
            }
        }
        true
    }
    fn flush_keys(&self) -> bool {
        let hwnd = self.editor_window.get();
        if hwnd.is_null() {
            return true;
        }
        let pid = window_pid(hwnd);
        if self.health.borrow().is_unhealthy(pid) {
            // Timed out before: let the flush timer give the keys back instead of waiting again.
            self.flush_attempts.set(u8::MAX);
            return false;
        }
        let keys = std::mem::take(&mut *self.pending_keys.borrow_mut());
        if keys.is_empty() {
            return true;
        }
        let mut units: Vec<u16> = keys.encode_utf16().collect();
        let packet = COPYDATASTRUCT {
            dwData: 2,
            cbData: (units.len() * 2) as u32,
            lpData: units.as_mut_ptr() as _,
        };
        let mut result = 0usize;
        let answered = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                self.receiver.get() as usize,
                &packet as *const _ as isize,
                SMTO_ABORTIFHUNG,
                yiime_core::REQUEST_TIMEOUT_MS,
                &mut result,
            ) != 0
        };
        if answered {
            self.health.borrow_mut().responded(pid);
        } else {
            self.health.borrow_mut().timed_out(pid);
            self.flush_attempts.set(u8::MAX);
            log("输入框服务请求超时（按键）");
        }
        let sent = answered && result != 0;
        if !sent {
            self.pending_keys.borrow_mut().insert_str(0, &keys);
        } else {
            self.flush_attempts.set(0);
        }
        sent
    }
    fn restore_pending_keys(&self) {
        let mut keys = std::mem::take(&mut *self.pending_keys.borrow_mut());
        if let Some(first) = self.first_key.take() {
            keys.insert(0, first);
        }
        if keys.is_empty() {
            return;
        }
        // Clone before starting the edit session: it may run synchronously and re-enter
        // finish_editor (borrow_mut).
        let context = self.context.borrow().clone();
        if let Some(context) = context {
            let session: ITfEditSession = CommitEdit {
                context: context.clone(),
                text: keys,
            }
            .into();
            let _ = unsafe {
                context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE)
            };
        }
    }
}

impl TextService_Impl {
    fn probe_scope(&self, context: ITfContext) {
        let result = self
            .scope_cache
            .borrow()
            .as_ref()
            .filter(|(old, _)| old == &context)
            .map(|(_, result)| result.clone())
            .unwrap_or_else(|| Rc::new(Cell::new(None)));
        *self.scope_cache.borrow_mut() = Some((context.clone(), result.clone()));
        let session: ITfEditSession = ScopeCheck {
            context: context.clone(),
            result: result.clone(),
        }
        .into();
        let requested = unsafe {
            context.RequestEditSession(self.client_id.get(), &session, TF_ES_ASYNC | TF_ES_READ)
        }
        .and_then(|status| status.ok())
        .is_ok();
        if !requested && result.get().is_none() {
            let mut path = [0u16; 1024];
            let len = unsafe { GetModuleFileNameW(None, &mut path) } as usize;
            let host = String::from_utf16_lossy(&path[..len.min(path.len())]);
            unsafe {
                OutputDebugStringW(
                    wide(&format!(
                        "YiIME: InputScope 查询失败；此输入框中输入法暂不可用。宿主：{host}"
                    ))
                    .as_ptr(),
                );
            }
        }
        let mut attached = self.edit_sink.borrow_mut();
        if attached.as_ref().is_some_and(|(old, _)| old == &context) {
            return;
        }
        if let Some((old, cookie)) = attached.take()
            && let Ok(source) = old.cast::<ITfSource>()
        {
            let _ = unsafe { source.UnadviseSink(cookie) };
        }
        if let Ok(source) = context.cast::<ITfSource>() {
            let sink: ITfTextEditSink = self.to_interface();
            if let Ok(cookie) = unsafe { source.AdviseSink(&ITfTextEditSink::IID, &sink) } {
                *attached = Some((context, cookie));
            }
        }
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, manager: Ref<ITfThreadMgr>, client_id: u32) -> Result<()> {
        com_guard("Activate", || {
            let manager = manager.ok()?.clone();
            // Do the fallible cast before creating the receiver window that holds the self pointer, so
            // a failure leaves no dangling pointer.
            let keystroke: ITfKeystrokeMgr = manager.cast()?;
            migrate_settings_key();
            let receiver = unsafe { receiver::create_receiver(self as *const _ as *mut c_void) };
            if receiver.is_null() {
                return E_FAIL.ok();
            }
            let sink: ITfKeyEventSink = self.to_interface();
            if let Err(error) = unsafe { keystroke.AdviseKeyEventSink(client_id, &sink, true) } {
                unsafe {
                    DestroyWindow(receiver);
                }
                return Err(error);
            }
            self.receiver.set(receiver);
            SESSION_RECEIVER.with(|cell| cell.set(receiver));
            self.client_id.set(client_id);
            if let Ok(source) = manager.cast::<ITfSource>() {
                let event_sink: ITfThreadMgrEventSink = self.to_interface();
                if let Ok(cookie) =
                    unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &event_sink) }
                {
                    self.manager_sink_cookie.set(Some(cookie));
                }
            }
            log("Activate");
            TextService::ensure_keyboard_mode(&manager, client_id);
            self.add_mode_button(&manager, client_id);
            let description: Vec<u16> = "中英切换 Ctrl+Space".encode_utf16().collect();
            if let Err(error) = unsafe {
                keystroke.PreserveKey(client_id, &TOGGLE_KEY, &TOGGLE_CHORD, &description)
            } {
                unsafe {
                    OutputDebugStringW(
                        wide(&format!("YiIME: Ctrl+Space 注册失败：{error}")).as_ptr(),
                    );
                }
            }
            for (guid, chord, text) in [
                (&PUNCT_KEY, &PUNCT_CHORD, "中/英文标点 Ctrl+."),
                (&TRAD_KEY, &TRAD_CHORD, "简/繁输出 Ctrl+Shift+F"),
            ] {
                let description: Vec<u16> = text.encode_utf16().collect();
                let _ = unsafe { keystroke.PreserveKey(client_id, guid, chord, &description) };
            }
            let history_text = history_key_text();
            if let Some(chord) = parse_hotkey(&history_text) {
                let description: Vec<u16> = "输入记忆".encode_utf16().collect();
                if unsafe { keystroke.PreserveKey(client_id, &HISTORY_KEY, &chord, &description) }
                    .is_ok()
                {
                    self.history_chord.set(Some(chord));
                }
            }
            if let Some(chord) = parse_hotkey(&plain_toggle_text()) {
                let description: Vec<u16> = "普通五笔模式 / 翻译模式".encode_utf16().collect();
                if unsafe { keystroke.PreserveKey(client_id, &PLAIN_KEY, &chord, &description) }
                    .is_ok()
                {
                    self.plain_chord.set(Some(chord));
                }
            }
            let initial_context = unsafe { manager.GetFocus() }
                .ok()
                .and_then(|document| unsafe { document.GetTop() }.ok());
            *self.thread_manager.borrow_mut() = Some(manager);
            if let Some(context) = initial_context {
                self.probe_scope(context);
            }
            Ok(())
        })
    }
    fn Deactivate(&self) -> Result<()> {
        com_guard("Deactivate", || {
            SESSION_RECEIVER.with(|cell| cell.set(null_mut()));
            // End this application's session in the shared service; the service stays for the
            // other applications (and exits by itself when idle).
            let editor_window = self.editor_window.get();
            if !editor_window.is_null() {
                unsafe {
                    PostMessageW(editor_window, WM_APP + 3, self.receiver.get() as usize, 0);
                }
            }
            if let Some((context, cookie)) = self.edit_sink.borrow_mut().take()
                && let Ok(source) = context.cast::<ITfSource>()
            {
                let _ = unsafe { source.UnadviseSink(cookie) };
            }
            if let Some(manager) = self.thread_manager.borrow_mut().take() {
                self.remove_mode_button(&manager);
                if let Some(cookie) = self.manager_sink_cookie.take()
                    && let Ok(source) = manager.cast::<ITfSource>()
                {
                    let _ = unsafe { source.UnadviseSink(cookie) };
                }
                if let Ok(keystroke) = manager.cast::<ITfKeystrokeMgr>() {
                    let _ = unsafe { keystroke.UnpreserveKey(&TOGGLE_KEY, &TOGGLE_CHORD) };
                    let _ = unsafe { keystroke.UnpreserveKey(&PUNCT_KEY, &PUNCT_CHORD) };
                    let _ = unsafe { keystroke.UnpreserveKey(&TRAD_KEY, &TRAD_CHORD) };
                    if let Some(chord) = self.history_chord.take() {
                        let _ = unsafe { keystroke.UnpreserveKey(&HISTORY_KEY, &chord) };
                    }
                    if let Some(chord) = self.plain_chord.take() {
                        let _ = unsafe { keystroke.UnpreserveKey(&PLAIN_KEY, &chord) };
                    }
                    let _ = unsafe { keystroke.UnadviseKeyEventSink(self.client_id.get()) };
                }
            }
            self.scope_cache.borrow_mut().take();
            self.tested_key.borrow_mut().take();
            let receiver = self.receiver.replace(null_mut());
            if !receiver.is_null() {
                unsafe {
                    SetWindowLongPtrW(receiver, GWLP_USERDATA, 0);
                    DestroyWindow(receiver);
                }
            }
            self.reset_editor();
            Ok(())
        })
    }
}
impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        com_guard("OnInitDocumentMgr", || Ok(()))
    }
    fn OnUninitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        com_guard("OnUninitDocumentMgr", || Ok(()))
    }
    fn OnSetFocus(
        &self,
        document: Ref<ITfDocumentMgr>,
        previous: Ref<ITfDocumentMgr>,
    ) -> Result<()> {
        com_guard("OnSetFocus", || {
            // Focus moved to another text field of this application (not to the input box, which
            // leaves no focused document here): the old session ends now.
            if document.is_some() && document.as_ref() != previous.as_ref() {
                self.drop_stale_session("焦点移到另一个输入框");
            }
            self.notify_focus_moved();
            self.scope_cache.borrow_mut().take();
            self.tested_key.borrow_mut().take();
            self.sync_plain_mode();
            if let Some(document) = document.as_ref()
                && let Ok(context) = unsafe { document.GetTop() }
            {
                self.probe_scope(context);
            }
            Ok(())
        })
    }
    fn OnPushContext(&self, context: Ref<ITfContext>) -> Result<()> {
        com_guard("OnPushContext", || {
            if let Some(context) = context.as_ref() {
                self.probe_scope(context.clone());
            }
            Ok(())
        })
    }
    fn OnPopContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        com_guard("OnPopContext", || {
            self.scope_cache.borrow_mut().take();
            self.tested_key.borrow_mut().take();
            Ok(())
        })
    }
}
impl ITfTextEditSink_Impl for TextService_Impl {
    fn OnEndEdit(
        &self,
        context: Ref<ITfContext>,
        _cookie: u32,
        _record: Ref<ITfEditRecord>,
    ) -> Result<()> {
        com_guard("OnEndEdit", || {
            if let Some(context) = context.as_ref() {
                self.probe_scope(context.clone());
            }
            Ok(())
        })
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    match std::panic::catch_unwind(|| dll_get_class_object(clsid, iid, out)) {
        Ok(result) => result,
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log("panic in DllGetClassObject"));
            E_FAIL
        }
    }
}

fn dll_get_class_object(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_FAIL;
    }
    unsafe { *out = null_mut() };
    if unsafe { *clsid } != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = ClassFactory.into();
    unsafe { factory.query(iid, out) }
}
#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    match std::panic::catch_unwind(register) {
        Ok(result) => result.map_or_else(|error| error.code(), |_| S_OK),
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log("panic in DllRegisterServer"));
            E_FAIL
        }
    }
}
#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    match std::panic::catch_unwind(unregister) {
        Ok(result) => result.map_or_else(|error| error.code(), |_| S_OK),
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log("panic in DllUnregisterServer"));
            E_FAIL
        }
    }
}
#[unsafe(no_mangle)]
extern "system" fn DllMain(instance: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(instance.0, Ordering::SeqCst);
    }
    true.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_in_com_callback_becomes_e_fail() {
        let result: Result<()> = com_guard("test", || panic!("test callback panic"));
        assert_eq!(result.unwrap_err().code(), E_FAIL);
    }
}
