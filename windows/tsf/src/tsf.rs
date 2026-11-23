//! Windows TSF text service. Normal typing composes in the target document.
#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::OnceLock as DictionaryLock;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};
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
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED, INSERT_TEXT_AT_SELECTION_FLAGS, ITfCandidateListUIElement,
    ITfCandidateListUIElement_Impl, ITfCategoryMgr, ITfCompartmentEventSink,
    ITfCompartmentEventSink_Impl, ITfCompartmentMgr, ITfComposition, ITfCompositionSink,
    ITfCompositionSink_Impl, ITfContext, ITfContextComposition, ITfContextView, ITfDocumentMgr,
    ITfEditRecord, ITfEditSession, ITfEditSession_Impl, ITfInputProcessorProfiles, ITfInputScope,
    ITfInsertAtSelection, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr,
    ITfLangBarItem_Impl, ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemMgr,
    ITfLangBarItemSink, ITfMenu, ITfSource, ITfSource_Impl, ITfTextEditSink, ITfTextEditSink_Impl,
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfTextLayoutSink, ITfTextLayoutSink_Impl, ITfThreadFocusSink,
    ITfThreadFocusSink_Impl, ITfThreadMgr, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
    ITfUIElement_Impl, ITfUIElementMgr, TF_AE_END, TF_ANCHOR_END, TF_ANCHOR_START, TF_CLUIE_COUNT,
    TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION,
    TF_CLUIE_STRING, TF_CONVERSIONMODE_NATIVE, TF_DEFAULT_SELECTION, TF_E_NOOBJECT, TF_ES_ASYNC,
    TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_IAS_QUERYONLY, TF_LANGBARITEMINFO,
    TF_LBI_CLK_RIGHT, TF_LBI_ICON, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_TEXT, TF_LBI_TOOLTIP,
    TF_MOD_ALT, TF_MOD_CONTROL, TF_MOD_SHIFT, TF_PRESERVEDKEY, TF_SD_READONLY, TF_SELECTION,
    TF_SELECTIONSTYLE, TfLBIClick,
};
use windows::core::{
    BOOL, BSTR, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result, implement,
};
use windows_registry::{CLASSES_ROOT, CURRENT_USER, LOCAL_MACHINE};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITORINFO,
    MonitorFromPoint,
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
    ES_PASSWORD, GA_ROOT, GUITHREADINFO, GWL_STYLE, GWLP_USERDATA, GetAncestor, GetClassNameW,
    GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowLongW, GetWindowThreadProcessId, HWND_MESSAGE, HWND_TOPMOST, IMAGE_ICON, IsChild,
    IsWindow, KillTimer, LoadImageW, PostMessageW, RegisterClassW, SM_CXSMICON, SMTO_ABORTIFHUNG,
    SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SendMessageTimeoutW, SetTimer, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WM_APP, WM_COPYDATA, WM_CREATE, WM_TIMER, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowExW, FindWindowW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx,
};
use yiime_core::{Flag, OptionStore, Options, Quotes, full_width, us_key_char};

mod inline;
mod key_sink;
mod lang_bar;
mod launch;
mod receiver;
mod registration;
use inline::InlineInput;
use lang_bar::{
    InputModeButton, LangBarSink, chinese_punctuation, history_key_text, option, page_size,
    parse_hotkey, plain_mode, plain_toggle_text, refresh_sink, set_plain_mode, shuangpin_scheme,
    toggle_option,
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
const EDITOR_KEY: GUID = GUID::from_u128(0x198710a7_bafd_46e2_9573_43f8c0f1d44a);
const EDITOR_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x54,
    uModifiers: TF_MOD_CONTROL | TF_MOD_ALT,
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
/// Timers: 1 = retry forwarding buffered keys to the editor; 2 = check a starting editor.
const FLUSH_TIMER: usize = 1;
const READY_TIMER: usize = 2;
const READY_TIMEOUT_MS: u64 = 5000;
/// While waiting for the editor, check this often whether it exited or hung.
const READY_POLL_MS: u32 = 250;
/// The InputScope privacy of a context (see yiime_core::private_scope), None until read.
type ScopeCache = Option<(ITfContext, Rc<Cell<Option<bool>>>)>;
/// Receiver messages: an asynchronous InputScope read finished (12), a caret rectangle arrived (13),
/// the language-bar or hotkey switched 译/普通五笔 (14, wParam 0 = 译, 1 = 普通五笔).
const WM_SCOPE_READ: u32 = WM_APP + 12;
const WM_CARET_READ: u32 = WM_APP + 13;
const WM_TRANSLATE_MODE: u32 = WM_APP + 14;
static MODULE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// Last good caret line height (yiime_core::caret_anchor).
static LINE_HEIGHT: AtomicI32 = AtomicI32::new(0);

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

/// Categories for the taskbar mode button, TSF candidate UI and immersive text hosts.
const CATEGORIES: [GUID; 6] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_COMLESS,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
];
/// Small diagnostic log: %TEMP%\YiIME-tsf.log (rotated to .old over 512 KB); ignored where it
/// cannot be written (AppContainer etc.).
fn log_hresult(action: &str, error: &windows::core::Error) {
    log(&format!("{action}：{:#010x}", error.code().0 as u32));
}

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
    /// None until DoEditSession finishes, then Some(success).
    outcome: Rc<Cell<Option<bool>>>,
}
impl ITfEditSession_Impl for CommitEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("DoEditSession", || {
            let result = (|| {
                let insert: ITfInsertAtSelection = self
                    .context
                    .cast()
                    .inspect_err(|error| log_hresult("上屏：ITfInsertAtSelection 接口", error))?;
                let utf16: Vec<u16> = self.text.encode_utf16().collect();
                // SampleIME Composition.cpp `_AddCharAndFinalize`: insert the committed
                // text at the selection (flags 0), then collapse the caret to the end.
                let range = unsafe {
                    insert.InsertTextAtSelection(cookie, INSERT_TEXT_AT_SELECTION_FLAGS(0), &utf16)
                }
                .inspect_err(|error| log_hresult("上屏：InsertTextAtSelection", error))?;
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
                }
                .inspect_err(|error| log_hresult("上屏：SetSelection", error));
                drop(ManuallyDrop::into_inner(selection.range));
                result
            })();
            self.outcome.set(Some(result.is_ok()));
            result
        })
    }
}

#[implement(ITfEditSession)]
struct ScopeCheck {
    context: ITfContext,
    result: Rc<Cell<Option<bool>>>,
    receiver: usize,
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
                drop(ManuallyDrop::into_inner(selection.range));
                self.result.set(Some(false));
                return Ok(());
            }
            let mut private = false;
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
                            let values: Vec<i32> = scopes.iter().map(|scope| scope.0).collect();
                            private = yiime_core::private_scope(&values);
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
            self.result.set(Some(private));
            unsafe {
                PostMessageW(self.receiver as HWND, WM_SCOPE_READ, 0, 0);
            }
            Ok(())
        })
    }
}

/// Reads the screen rectangle of the insertion point so the editor can place its box at the
/// target's cursor.
fn rect_of([left, top, right, bottom]: [i32; 4]) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}
/// System caret of the focused thread, else the mouse, as a last resort when GetTextExt is empty.
fn screen_caret() -> Option<RECT> {
    let hwnd = unsafe { GetFocus() };
    let hwnd = if hwnd.is_null() {
        unsafe { GetForegroundWindow() }
    } else {
        hwnd
    };
    let thread = if hwnd.is_null() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(hwnd, null_mut()) }
    };
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
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
        if bottom_right.y > top_left.y {
            return Some(RECT {
                left: top_left.x,
                top: top_left.y,
                right: bottom_right.x,
                bottom: bottom_right.y,
            });
        }
    }
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } != 0 {
        let height = LINE_HEIGHT.load(Ordering::Relaxed).max(20);
        return Some(RECT {
            left: point.x,
            top: point.y,
            right: point.x + 1,
            bottom: point.y + height,
        });
    }
    None
}

#[implement(ITfEditSession)]
struct CaretQuery {
    context: ITfContext,
    result: Rc<Cell<yiime_core::SegmentAnchor>>,
    /// Survives SegmentAnchor::begin, so a later session can fall back to this rectangle.
    last: Rc<Cell<Option<[i32; 4]>>>,
    ticket: u32,
    receiver: usize,
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
            // The start of the selection: where the typed text goes.
            unsafe {
                range.Collapse(cookie, TF_ANCHOR_START)?;
                view.GetTextExt(cookie, &range, &mut rect, &mut clipped)?;
            }
            let Some((anchor, height)) = yiime_core::caret_anchor(
                [rect.left, rect.top, rect.right, rect.bottom],
                LINE_HEIGHT.load(Ordering::Relaxed),
            ) else {
                return Ok(());
            };
            // Hosts without a real text store (some full-screen games) answer a rectangle off
            // every monitor, e.g. (-1000,-1000,…): not a place for the box.
            let center = POINT {
                x: anchor[0] + (anchor[2] - anchor[0]) / 2,
                y: anchor[1] + (anchor[3] - anchor[1]) / 2,
            };
            if unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) }.is_null() {
                log("光标矩形不在任何显示器上，忽略");
                return Ok(());
            }
            LINE_HEIGHT.store(height, Ordering::Relaxed);
            let mut current = self.result.get();
            if !current.arrive(self.ticket, anchor) {
                return Ok(());
            }
            self.result.set(current);
            self.last.set(Some(anchor));
            unsafe {
                PostMessageW(self.receiver as HWND, WM_CARET_READ, 0, 0);
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
/// Window that shows `context` (null if unknown).
/// Both windows are inside the same top-level window (GetAncestor GA_ROOT).
fn same_root(a: HWND, b: HWND) -> bool {
    !a.is_null() && !b.is_null() && unsafe { GetAncestor(a, GA_ROOT) == GetAncestor(b, GA_ROOT) }
}
/// `window` is, or contains, the window with the keyboard focus.
fn has_keyboard(window: HWND, keyboard: HWND) -> bool {
    !window.is_null()
        && !keyboard.is_null()
        && (window == keyboard || unsafe { IsChild(window, keyboard) } != 0)
}
/// Window class for the log (no content).
fn window_class(window: HWND) -> String {
    if window.is_null() {
        return "-".into();
    }
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) } as usize;
    String::from_utf16_lossy(&class[..len])
}
fn context_window(context: &ITfContext) -> HWND {
    unsafe { context.GetActiveView() }
        .and_then(|view| unsafe { view.GetWnd() })
        .map_or(null_mut(), |window| window.0 as HWND)
}
/// Quote arguments for `ShellExecuteW` (empty values become `""`).
fn shell_parameters(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(' ') || arg.contains('\\') || arg.contains('"') {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    /// text field, and tells the box to close (WM_APP + 8, lParam = the id from [`Self::hold_context`]).
    /// The context is parked, not dropped: the box commits a translation's Chinese there, and a
    /// newer session's context is left alone.
    fn drop_stale_session(&self, reason: &str) {
        if !self.buffering() {
            return;
        }
        log(&format!("结束旧的输入会话：{reason}"));
        self.kill_timers();
        self.editor_open.set(false);
        self.pending_keys.borrow_mut().clear();
        self.flush_attempts.set(0);
        let id = self.hold_context();
        let editor_window = self.editor_window.get();
        let receiver = self.receiver.get();
        if editor_window.is_null() || receiver.is_null() || unsafe { IsWindow(editor_window) } == 0
        {
            // No box to send the field back: release it now, or the next session's first commit
            // would go there.
            self.clear_held();
            return;
        }
        // Synchronous, so a translation result already queued on the box is applied only after
        // this session's Chinese has been committed and the draft cleared. If the box does not
        // answer in time, post it (a second delivery only releases an id that was already used);
        // if even that fails nobody will release the field, so release it here.
        let answered = unsafe {
            let mut result = 0usize;
            SendMessageTimeoutW(
                editor_window,
                WM_APP + 8,
                receiver as usize,
                id as isize,
                SMTO_ABORTIFHUNG,
                yiime_core::REQUEST_TIMEOUT_MS,
                &mut result,
            ) != 0
        };
        if !answered
            && unsafe { PostMessageW(editor_window, WM_APP + 8, receiver as usize, id as isize) }
                == 0
        {
            self.clear_held();
        }
    }
    /// Moves the current text field aside so a new session can replace it. Returns the id the box
    /// sends back with the Chinese source (never 0).
    fn hold_context(&self) -> u64 {
        let id = yiime_core::next_held_id(self.next_held.get());
        self.next_held.set(id);
        *self.held_context.borrow_mut() = self.context.borrow_mut().take();
        self.session_document.borrow_mut().take();
        self.session_window.set(null_mut());
        self.held_id.set(id);
        id
    }
    fn clear_held(&self) {
        self.held_context.borrow_mut().take();
        self.held_id.set(0);
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
    let class = wide(yiime_core::APP_WINDOW_CLASS);
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
            wide(yiime_core::APP_WINDOW_CLASS).as_ptr(),
            wide(yiime_core::TRAY_WINDOW_TITLE).as_ptr(),
        )
    };
    if !tray.is_null() {
        unsafe {
            PostMessageW(tray, WM_APP + if history { 6 } else { 5 }, 0, 0);
        }
        return;
    }
    // qingjian launch.rs: ShellExecute + mutex/cooldown; refused launches are only logged.
    match launch::launch_companion(if history { "--history" } else { "--settings" }) {
        launch::LaunchOutcome::Requested | launch::LaunchOutcome::InFlight => {}
        launch::LaunchOutcome::Refused => log("启动托盘失败：无法拉起 YiIME.exe"),
    }
}

/// Language-bar menu lives on the button (no TextService); the session receiver applies the route.
fn notify_plain_mode(plain: bool) {
    let receiver = SESSION_RECEIVER.with(Cell::get);
    if !receiver.is_null() {
        unsafe {
            let _ = PostMessageW(receiver, WM_TRANSLATE_MODE, usize::from(plain), 0);
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
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfThreadFocusSink,
    ITfTextEditSink
)]
struct TextService {
    thread_manager: RefCell<Option<ITfThreadMgr>>,
    context: RefCell<Option<ITfContext>>,
    /// Text field a dropped session was typing into, and the id the box echoes to commit there.
    held_context: RefCell<Option<ITfContext>>,
    /// Document (text field) of the open session; focus events compare against it.
    session_document: RefCell<Option<ITfDocumentMgr>>,
    /// Window showing the session's field (0 if unknown).
    session_window: Cell<HWND>,
    /// Last reason logged for letters passed to the application (logged again only on change).
    pass_logged: Cell<Option<yiime_core::PassReason>>,
    /// Digest and time of the last text inserted for the box (see yiime_core::duplicate_commit).
    last_commit: Cell<Option<(u32, std::time::Instant)>>,
    /// Last good caret rectangle (shared with the asynchronous CaretQuery).
    caret: Rc<Cell<yiime_core::SegmentAnchor>>,
    /// Last GetTextExt rectangle kept after a new session clears [`Self::caret`].
    last_caret: Rc<Cell<Option<[i32; 4]>>>,
    /// A rectangle arrived while the box was starting; sent once it is ready.
    caret_fresh: Cell<bool>,
    held_id: Cell<u64>,
    next_held: Cell<u64>,
    client_id: Cell<u32>,
    receiver: Cell<HWND>,
    editor_open: Cell<bool>,
    /// The editor process this instance started (None when it uses a service found running).
    editor: RefCell<Option<std::process::Child>>,
    /// Process id of the editor service in use: commits are accepted only from it.
    editor_pid: Cell<u32>,
    editor_window: Cell<HWND>,
    /// Window of the shared service a starting session waits for (cleared when it is ready).
    starting_window: Cell<HWND>,
    /// When a starting session gives up waiting for its editor to report ready.
    ready_deadline: Cell<Option<std::time::Instant>>,
    pending_keys: RefCell<String>,
    flush_attempts: Cell<u8>,
    /// Timeouts of the shared service and restart backoff (see yiime_core::ServiceHealth).
    health: RefCell<yiime_core::ServiceHealth>,
    /// This application is elevated / high integrity (or its token is unreadable): it uses a
    /// private input box, never the shared service. Computed on first use.
    private: Cell<Option<bool>>,
    scope_cache: RefCell<ScopeCache>,
    manager_sink_cookie: Cell<Option<u32>>,
    /// ITfThreadFocusSink advise cookie.
    focus_sink_cookie: Cell<Option<u32>>,
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
    inline: InlineInput,
}
impl TextService {
    fn new() -> Self {
        Self {
            thread_manager: RefCell::new(None),
            context: RefCell::new(None),
            held_context: RefCell::new(None),
            session_document: RefCell::new(None),
            session_window: Cell::new(null_mut()),
            pass_logged: Cell::new(None),
            last_commit: Cell::new(None),
            caret: Rc::new(Cell::new(yiime_core::SegmentAnchor::default())),
            last_caret: Rc::new(Cell::new(None)),
            caret_fresh: Cell::new(false),
            held_id: Cell::new(0),
            next_held: Cell::new(0),
            client_id: Cell::new(0),
            receiver: Cell::new(null_mut()),
            editor_open: Cell::new(false),
            editor: RefCell::new(None),
            editor_pid: Cell::new(0),
            editor_window: Cell::new(null_mut()),
            starting_window: Cell::new(null_mut()),
            ready_deadline: Cell::new(None),
            pending_keys: RefCell::new(String::new()),
            flush_attempts: Cell::new(0),
            health: RefCell::new(yiime_core::ServiceHealth::default()),
            private: Cell::new(None),
            scope_cache: RefCell::new(None),
            manager_sink_cookie: Cell::new(None),
            focus_sink_cookie: Cell::new(None),
            edit_sink: RefCell::new(None),
            tested_key: RefCell::new(None),
            mode_button: RefCell::new(None),
            shift_down: Cell::new(None),
            plain_chord: Cell::new(None),
            history_chord: Cell::new(None),
            after_digit: Cell::new(false),
            quotes: Cell::new(Quotes::default()),
            shown_plain: Cell::new(false),
            inline: InlineInput::new(),
        }
    }
    fn refresh_editor(&self) {
        // A session still starting whose editor exited or hung: give its letters back first.
        if self.check_startup() {
            return;
        }
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
            if self.editor_open.get() {
                // The box died during a session: letters it never received go back to the
                // application instead of vanishing.
                log("输入框进程已退出，未发送的按键已归还");
                self.restore_pending_keys();
            }
            self.reset_editor();
        }
    }
    /// The session is waiting for its editor to report ready.
    fn waiting_for_editor(&self) -> bool {
        self.editor_open.get() && self.editor_window.get().is_null()
    }
    /// Checks a starting session (on every key and on the ready timer). Returns true when its
    /// editor exited or did not become ready in time and the session was given up: the first
    /// letter and the buffered ones are typed into the application.
    fn check_startup(&self) -> bool {
        // ShellExecuteW does not return a pid: bind a running companion as soon as its
        // window appears so the ready/commit COPYDATA is not rejected (editor_pid == 0).
        if self.editor_open.get()
            && self.editor_pid.get() == 0
            && let Some(editor) = companion_exe()
            && let Some(host) = crate::process_security::process_security(0)
            && let Some((found, pid)) = find_editor_service(&editor, &host)
        {
            self.editor_pid.set(pid);
            self.starting_window.set(found);
            self.allow_editor_foreground();
            log(&format!("输入框服务已找到，等待就绪握手：pid={pid}"));
        }
        let overdue = self
            .ready_deadline
            .get()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline);
        let state = yiime_core::startup_state(
            self.editor_open.get(),
            !self.editor_window.get().is_null(),
            self.waiting_for_editor() && self.startup_process_gone(),
            overdue,
        );
        match state {
            yiime_core::Startup::Idle | yiime_core::Startup::Waiting => false,
            yiime_core::Startup::Exited => {
                self.abandon_start(false);
                true
            }
            yiime_core::Startup::TimedOut => {
                self.abandon_start(true);
                true
            }
        }
    }
    fn startup_process_gone(&self) -> bool {
        if let Some(child) = self.editor.borrow_mut().as_mut() {
            return child.try_wait().ok().flatten().is_some();
        }
        let starting = self.starting_window.get();
        !starting.is_null() && unsafe { IsWindow(starting) } == 0
    }
    fn abandon_start(&self, timed_out: bool) {
        let owned = self.editor.borrow().is_some();
        if owned {
            if let Some(child) = self.editor.borrow_mut().as_mut() {
                let _ = child.kill();
            }
            // Back off before starting another explicit editor session.
            self.health
                .borrow_mut()
                .start_failed(std::time::Instant::now());
        } else if timed_out {
            // A shared service that never answers is restarted on the next activation.
            self.health.borrow_mut().timed_out(self.editor_pid.get());
        }
        log(if timed_out {
            "输入框就绪超时，已归还缓存按键"
        } else {
            "输入框启动失败（进程已退出），已归还缓存按键"
        });
        self.restore_pending_keys();
        self.reset_editor();
    }
    fn reset_editor(&self) {
        self.kill_timers();
        self.editor_open.set(false);
        self.editor.borrow_mut().take();
        self.editor_pid.set(0);
        self.editor_window.set(null_mut());
        self.starting_window.set(null_mut());
        self.ready_deadline.set(None);
        self.pending_keys.borrow_mut().clear();
        self.flush_attempts.set(0);
        self.context.borrow_mut().take();
        self.session_document.borrow_mut().take();
        self.session_window.set(null_mut());
        self.clear_held();
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
    /// The insertion point's screen rectangle for the box. Prefer the same source as the
    /// candidate bar (qingjian `report_caret`: reuse the last-good anchor when GetTextExt on
    /// the composition range is empty). A separate selection GetTextExt (CaretQuery) can
    /// "succeed" with a misleading rectangle in hosts like Grok Bot, so it is only used when
    /// the candidate bar's last GetTextExt also succeeded, or when there is no last-good caret.
    /// The async read still runs (SampleIME KeyHandler.cpp: no TF_ES_SYNC) and may refine the
    /// box later via WM_CARET_READ, subject to the same rule.
    fn caret_rect(&self, context: &ITfContext) -> Option<RECT> {
        self.caret_fresh.set(false);
        let mut anchor = self.caret.get();
        let ticket = anchor.begin();
        self.caret.set(anchor);
        let session: ITfEditSession = CaretQuery {
            context: context.clone(),
            result: self.caret.clone(),
            last: self.last_caret.clone(),
            ticket,
            receiver: self.receiver.get() as usize,
        }
        .into();
        let _ = unsafe {
            context.RequestEditSession(
                self.client_id.get(),
                &session,
                TF_ES_ASYNCDONTCARE | TF_ES_READ,
            )
        };
        let measured = self.caret.get().rect().map(rect_of);
        let inline = self.inline.last_caret().map(|(x, top, bottom)| RECT {
            left: x,
            top,
            right: x,
            bottom,
        });
        if let Some(rect) = inline {
            if self.inline.last_ext_ok() {
                if let Some(measured) = measured {
                    log(&format!(
                        "草稿框光标：GetTextExt=({},{},{},{}) 候选窗=({},{},{},{})，与候选窗同一来源",
                        measured.left,
                        measured.top,
                        measured.right,
                        measured.bottom,
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom
                    ));
                    Some(measured)
                } else {
                    log(&format!(
                        "草稿框光标：GetTextExt 为空，使用候选窗位置 ({},{},{},{})",
                        rect.left, rect.top, rect.right, rect.bottom
                    ));
                    Some(rect)
                }
            } else {
                if let Some(measured) = measured {
                    log(&format!(
                        "草稿框光标：组字 GetTextExt 不可靠，忽略选区矩形 ({},{},{},{})，使用候选窗位置 ({},{},{},{})",
                        measured.left,
                        measured.top,
                        measured.right,
                        measured.bottom,
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom
                    ));
                } else {
                    log(&format!(
                        "草稿框光标：GetTextExt 为空，使用候选窗位置 ({},{},{},{})",
                        rect.left, rect.top, rect.right, rect.bottom
                    ));
                }
                Some(rect)
            }
        } else if let Some(rect) = measured {
            log(&format!(
                "草稿框光标：GetTextExt=({},{},{},{})",
                rect.left, rect.top, rect.right, rect.bottom
            ));
            Some(rect)
        } else if let Some(rect) = self.last_caret.get() {
            let rect = rect_of(rect);
            log(&format!(
                "草稿框光标：尚未读到，使用上次位置 ({},{},{},{})",
                rect.left, rect.top, rect.right, rect.bottom
            ));
            Some(rect)
        } else if let Some(rect) = screen_caret() {
            log(&format!(
                "草稿框光标：尚未读到，使用系统光标或鼠标 ({},{},{},{})",
                rect.left, rect.top, rect.right, rect.bottom
            ));
            Some(rect)
        } else {
            log("草稿框光标：没有可用矩形");
            None
        }
    }
    /// A caret rectangle read asynchronously arrived: move the open box there.
    fn caret_arrived(&self) {
        let Some(rect) = self.caret.get().rect().map(rect_of) else {
            return;
        };
        // Same rule as caret_rect: do not let a misleading selection GetTextExt jump the box
        // away from the candidate bar's last-good caret (Grok Bot, Firefox, …).
        if self.inline.last_caret().is_some() && !self.inline.last_ext_ok() {
            log(&format!(
                "草稿框光标：忽略迟到的选区矩形 ({},{},{},{})，保持候选窗位置",
                rect.left, rect.top, rect.right, rect.bottom
            ));
            return;
        }
        if !self.buffering() {
            return;
        }
        let window = self.editor_window.get();
        if window.is_null() {
            // Still starting: sent once it is ready.
            self.caret_fresh.set(true);
            return;
        }
        self.caret_fresh.set(false);
        let mut values = [rect.left, rect.top, rect.right, rect.bottom];
        let packet = COPYDATASTRUCT {
            dwData: 3,
            cbData: 16,
            lpData: values.as_mut_ptr() as _,
        };
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(
                window,
                WM_COPYDATA,
                self.receiver.get() as usize,
                &packet as *const _ as isize,
                SMTO_ABORTIFHUNG,
                yiime_core::REQUEST_TIMEOUT_MS,
                &mut result,
            );
        }
    }
    /// Unreadable compartments do not disable (yiime_core::FieldSwitches).
    fn compartment_on(manager: &ITfCompartmentMgr, id: &GUID) -> Option<bool> {
        let compartment = unsafe { manager.GetCompartment(id) }.ok()?;
        let value = unsafe { compartment.GetValue() }.ok()?;
        let inner = unsafe { &*value.Anonymous.Anonymous };
        Some(inner.vt.0 == 3 && unsafe { inner.Anonymous.lVal != 0 })
    }
    /// The field refuses IME input: TF_SD_READONLY, KEYBOARD_DISABLED or EMPTYCONTEXT.
    fn disabled(context: &ITfContext) -> bool {
        let manager = context.cast::<ITfCompartmentMgr>().ok();
        yiime_core::FieldSwitches {
            read_only: unsafe { context.GetStatus() }
                .is_ok_and(|status| status.dwDynamicFlags & TF_SD_READONLY != 0),
            keyboard_disabled: manager.as_ref().and_then(|manager| {
                Self::compartment_on(manager, &GUID_COMPARTMENT_KEYBOARD_DISABLED)
            }),
            empty_context: manager
                .as_ref()
                .and_then(|manager| Self::compartment_on(manager, &GUID_COMPARTMENT_EMPTYCONTEXT)),
            es_password: false,
        }
        .blocked()
    }
    /// A Win32 edit control with ES_PASSWORD has the keyboard (the bit means something else in
    /// other window classes).
    fn es_password(&self) -> bool {
        let focus = unsafe { GetFocus() };
        if focus.is_null() {
            return false;
        }
        let mut class = [0u16; 32];
        let len = unsafe { GetClassNameW(focus, class.as_mut_ptr(), class.len() as i32) } as usize;
        let class = String::from_utf16_lossy(&class[..len]).to_ascii_lowercase();
        (class == "edit" || class.starts_with("richedit"))
            && unsafe { GetWindowLongW(focus, GWL_STYLE) } & ES_PASSWORD != 0
    }
    /// The field declared a private InputScope (as far as read so far). Only privacy follows
    /// from it, never whether letters are taken.
    fn private_field(&self, context: &ITfContext) -> bool {
        self.scope_cache
            .borrow()
            .as_ref()
            .filter(|(cached, _)| cached == context)
            .is_some_and(|(_, result)| result.get() == Some(true))
    }
    /// Tells the box whether the session's field is private (WM_APP + 11, wParam = receiver,
    /// lParam 1 = private): no learning, history or translation service there.
    fn notify_privacy(&self) {
        let window = self.editor_window.get();
        if !self.buffering() || window.is_null() {
            return;
        }
        let private = self
            .context
            .borrow()
            .as_ref()
            .is_some_and(|context| self.private_field(context));
        unsafe {
            PostMessageW(
                window,
                WM_APP + 11,
                self.receiver.get() as usize,
                private as isize,
            );
        }
    }
    /// Logs why a plain letter went to the application, once per change of reason.
    fn note_pass(&self, reason: Option<yiime_core::PassReason>) {
        if self.pass_logged.replace(reason) != reason
            && let Some(reason) = reason
        {
            log(&format!("按键交给应用：{}", reason.describe()));
        }
    }
    fn wants_key(&self, context: &ITfContext, key: usize) -> bool {
        if running_in_editor() {
            return false;
        }
        self.refresh_editor();
        let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
        if !self.buffering() {
            if self.inline.active() && !self.inline.same_context(context) {
                log("按键来自另一个输入框，取消组字");
                self.inline.cancel(self.client_id.get());
            }
            let active = self.inline.active();
            return !Self::disabled(context)
                && self.keyboard_open()
                && !self.es_password()
                && (active || !caps_lock())
                && ![VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                    .into_iter()
                    .any(modifier)
                && (active || !modifier(VK_SHIFT))
                && self.inline.wants(context, key);
        }
        if !(0x41..=0x5a).contains(&key) || Self::disabled(context) {
            return false;
        }
        if !self.keyboard_open() {
            return false;
        }
        // Same field: the session's context, or another context of its document (hosts may
        // re-create the context of a field).
        let same_context = self
            .context
            .borrow()
            .as_ref()
            .is_some_and(|active| active == context)
            || self
                .session_document
                .borrow()
                .as_ref()
                .is_some_and(|document| {
                    unsafe { context.GetDocumentMgr() }.is_ok_and(|other| &other == document)
                });
        let key_window = context_window(context);
        let session_window = self.session_window.get();
        let other_child_window =
            key_window != session_window && same_root(key_window, session_window);
        match yiime_core::key_route(self.buffering(), same_context, other_child_window) {
            yiime_core::KeyRoute::Buffer => return true,
            yiime_core::KeyRoute::Adopt => {
                log(&format!(
                    "按键来自同一窗口的另一个子窗口（{} → {}，键盘焦点 {}）：会话改用该输入框",
                    window_class(session_window),
                    window_class(key_window),
                    window_class(unsafe { GetFocus() }),
                ));
                *self.context.borrow_mut() = Some(context.clone());
                *self.session_document.borrow_mut() = unsafe { context.GetDocumentMgr() }.ok();
                self.session_window.set(key_window);
                return true;
            }
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
            || self.inline.active()
            || modifier(VK_CONTROL)
            || modifier(VK_MENU)
            || modifier(VK_LWIN)
            || modifier(VK_RWIN)
            || !self.keyboard_open()
            || option(Flag::PunctEnglish)
            || Self::disabled(context)
            || self.es_password()
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
            outcome: Rc::new(Cell::new(None)),
        }
        .into();
        unsafe { context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE) }
            .is_ok()
    }
    fn should_consume(&self, context: &ITfContext, key: usize) -> bool {
        let consume = self.wants_key(context, key);
        if consume {
            self.note_pass(None);
        } else if (0x41..=0x5a).contains(&key) && !running_in_editor() {
            let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
            let read_only = Self::disabled(context);
            let english = !self.keyboard_open();
            let modifier_or_caps = caps_lock()
                || [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN, VK_SHIFT]
                    .into_iter()
                    .any(modifier);
            let password = !read_only && !english && self.es_password();
            self.note_pass(yiime_core::pass_reason(
                read_only,
                english,
                modifier_or_caps,
                password,
            ));
        }
        consume
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
            && ![VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                .into_iter()
                .any(|vk| unsafe { GetKeyState(vk as i32) } < 0)
        {
            self.inline.cancel(self.client_id.get());
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
        notify_plain_mode(plain);
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
            log("草稿框：找不到 YiIME.exe");
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
            *self.session_document.borrow_mut() = unsafe { context.GetDocumentMgr() }.ok();
            self.session_window.set(context_window(&context));
            self.editor_open.set(true);
            self.editor_window.set(null_mut());
            self.starting_window.set(window);
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
                log("草稿框已打开（已连接到运行中的服务）");
                return true;
            }
            log(if answered && !accepted {
                "草稿框：COPYDATA 被拒绝，改启动新进程"
            } else {
                "草稿框：未能激活已有服务，改启动新进程"
            });
            if let Some(child) = self.editor.borrow_mut().as_mut() {
                let _ = child.kill();
            }
            self.reset_editor();
        }
        let Ok(path) = module_path() else {
            log("草稿框：无法确定 DLL 路径");
            return false;
        };
        let Some(directory) = std::path::Path::new(&path).parent() else {
            return false;
        };
        let directory = editor.parent().unwrap_or(directory).to_path_buf();
        let initial = if first_key == 0 {
            String::new()
        } else if first_key == RECALL {
            "-".to_owned()
        } else {
            format!("{}", first_key as char)
        };
        let mut args = vec![
            "--tsf-hwnd".into(),
            format!("{}", self.receiver.get() as usize),
            "--target-hwnd".into(),
            format!("{target}"),
            "--initial".into(),
            initial,
        ];
        if first_key == RECALL {
            args.push("--recall".into());
        }
        if let Some(rect) = caret {
            args.push("--caret".into());
            args.push(format!(
                "{},{},{},{}",
                rect.left, rect.top, rect.right, rect.bottom
            ));
        }
        // Elevated / high-integrity hosts keep a private instance (CreateProcess is fine;
        // qingjian launch.rs only starts the shared Medium-integrity server).
        if self.private_editor() {
            args.push("--private".into());
            let mut command = std::process::Command::new(editor);
            command
                .args(&args)
                .current_dir(directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            return if let Ok(child) = command.spawn() {
                *self.session_document.borrow_mut() = unsafe { context.GetDocumentMgr() }.ok();
                self.session_window.set(context_window(&context));
                *self.context.borrow_mut() = Some(context);
                self.editor_pid.set(child.id());
                *self.editor.borrow_mut() = Some(child);
                self.allow_editor_foreground();
                self.editor_open.set(true);
                self.start_ready_timer();
                true
            } else {
                log("输入框程序无法启动");
                self.health
                    .borrow_mut()
                    .start_failed(std::time::Instant::now());
                false
            };
        }
        match launch::launch_companion(&shell_parameters(&args)) {
            launch::LaunchOutcome::Requested => {
                *self.session_document.borrow_mut() = unsafe { context.GetDocumentMgr() }.ok();
                self.session_window.set(context_window(&context));
                *self.context.borrow_mut() = Some(context);
                self.allow_editor_foreground();
                self.editor_open.set(true);
                self.start_ready_timer();
                true
            }
            launch::LaunchOutcome::InFlight => {
                // Another launch is already on the way (no --initial for this key).
                if first_key != 0 && first_key != RECALL {
                    self.pending_keys.borrow_mut().push(typed_letter(first_key));
                }
                *self.session_document.borrow_mut() = unsafe { context.GetDocumentMgr() }.ok();
                self.session_window.set(context_window(&context));
                *self.context.borrow_mut() = Some(context);
                self.allow_editor_foreground();
                self.editor_open.set(true);
                self.start_ready_timer();
                true
            }
            launch::LaunchOutcome::Refused => {
                log("输入框程序无法启动");
                self.health
                    .borrow_mut()
                    .start_failed(std::time::Instant::now());
                false
            }
        }
    }
    fn start_ready_timer(&self) {
        self.ready_deadline.set(Some(
            std::time::Instant::now() + std::time::Duration::from_millis(READY_TIMEOUT_MS),
        ));
        unsafe {
            SetTimer(self.receiver.get(), READY_TIMER, READY_POLL_MS, None);
        }
    }
    fn commit(&self, text: String) -> bool {
        if text.is_empty() {
            // Empty: the box closed its session. With no session open, release a parked field
            // (the box was not told about it yet).
            if self.editor_open.get() {
                self.finish_editor();
            } else if self.held_id.get() != 0 {
                self.commit_held(self.held_id.get(), String::new());
            }
            return true;
        }
        let session = self
            .editor_open
            .get()
            .then(|| self.context.borrow().clone())
            .flatten();
        let held = self.held_context.borrow().clone();
        let digest = yiime_core::commit_digest(&text);
        let last = self
            .last_commit
            .get()
            .map(|(previous, at)| (previous, at.elapsed().as_millis() as u64));
        if yiime_core::duplicate_commit(session.is_some(), held.is_some(), last, digest) {
            log("重复的上屏请求（上一次已上屏），忽略");
            self.confirm_commit(digest);
            return true;
        }
        let focused = self.focused_context();
        let session_window = self.session_window.get();
        let moved_in_window = session.is_some()
            && focused.as_ref().is_some_and(|focused| {
                let document = unsafe { focused.GetDocumentMgr() }.ok();
                same_root(context_window(focused), session_window)
                    && document.as_ref() != self.session_document.borrow().as_ref()
            });
        let mut redirect = false;
        if moved_in_window && let (Some(session), Some(focused)) = (&session, &focused) {
            let alive = self.session_alive(session);
            let focused_window = context_window(focused);
            let same_control = focused_window == session_window;
            let keyboard = unsafe { GetFocus() };
            let session_keyboard = has_keyboard(session_window, keyboard);
            let focused_keyboard = has_keyboard(focused_window, keyboard);
            redirect = yiime_core::focused_first(
                session_keyboard,
                focused_keyboard,
                yiime_core::redirect_to_focused(true, alive, same_control),
            );
            log(&format!(
                "上屏：焦点在同一顶层窗口的另一个文档（会话上下文仍有效={alive}，同一控件窗口={same_control}，会话窗口 {} 键盘={session_keyboard}，焦点文档窗口 {} 键盘={focused_keyboard}，键盘焦点 {}）→ 先上屏到{}",
                window_class(session_window),
                window_class(focused_window),
                window_class(keyboard),
                if redirect {
                    "焦点输入框"
                } else {
                    "会话的输入框"
                }
            ));
        }
        let targets = yiime_core::commit_targets(
            session.is_some(),
            held.is_some(),
            focused.is_some(),
            redirect,
        );
        for target in targets {
            let (context, which) = match target {
                yiime_core::CommitTarget::Session => (session.clone(), "session"),
                yiime_core::CommitTarget::Held => (held.clone(), "held"),
                yiime_core::CommitTarget::Focused => (focused.clone(), "focused"),
            };
            let Some(context) = context else { continue };
            if self.insert_text(&context, text.clone()) {
                log(&format!("上屏成功：{which}"));
                self.last_commit
                    .set(Some((digest, std::time::Instant::now())));
                self.confirm_commit(digest);
                match target {
                    yiime_core::CommitTarget::Session => self.finish_editor(),
                    yiime_core::CommitTarget::Held => self.clear_held(),
                    yiime_core::CommitTarget::Focused if moved_in_window => self.finish_editor(),
                    yiime_core::CommitTarget::Focused => {
                        log("上屏到当前焦点输入框（原会话已结束）");
                        self.clear_held();
                        self.finish_editor();
                    }
                }
                return true;
            }
            log(&format!("上屏失败：{which}"));
        }
        log("没有可上屏的输入框");
        false
    }
    /// Tells the box the text landed (WM_APP + 10, wParam = receiver, lParam = digest), posted
    /// besides the COPYDATA reply: that reply can be lost to a timeout while the focus moves, and
    /// the box would keep a draft that was already inserted.
    fn confirm_commit(&self, digest: u32) {
        let editor_window = self.editor_window.get();
        let receiver = self.receiver.get();
        if !editor_window.is_null() && !receiver.is_null() {
            unsafe {
                let _ = PostMessageW(
                    editor_window,
                    WM_APP + 10,
                    receiver as usize,
                    digest as isize,
                );
            }
        }
    }
    /// The session's context is still in place: the top context of its document, and that
    /// document still belongs to this thread.
    fn session_alive(&self, session: &ITfContext) -> bool {
        let Some(document) = self.session_document.borrow().clone() else {
            return false;
        };
        if unsafe { document.GetTop() }.ok().as_ref() != Some(session) {
            return false;
        }
        let Some(manager) = self.thread_manager.borrow().clone() else {
            return false;
        };
        let Ok(list) = (unsafe { manager.EnumDocumentMgrs() }) else {
            return true;
        };
        loop {
            let mut item = [None];
            let mut fetched = 0;
            if unsafe { list.Next(&mut item, &mut fetched) }.is_err() || fetched == 0 {
                return false;
            }
            if item[0].as_ref() == Some(&document) {
                return true;
            }
        }
    }
    /// Top context of the document that has the focus in this thread, if it takes input.
    fn focused_context(&self) -> Option<ITfContext> {
        let manager = self.thread_manager.borrow().clone()?;
        let document = unsafe { manager.GetFocus() }.ok()?;
        let context = unsafe { document.GetTop() }.ok()?;
        (!Self::disabled(&context)).then_some(context)
    }
    /// Inserts `text` into the parked text field `id` and releases it. Does not touch a session
    /// that started afterwards. Empty text only releases the field.
    fn commit_held(&self, id: u64, text: String) -> bool {
        if id == 0 || id != self.held_id.get() {
            return false;
        }
        let context = self.held_context.borrow_mut().take();
        self.held_id.set(0);
        if text.is_empty() {
            return true;
        }
        let Some(context) = context else {
            return false;
        };
        self.insert_text(&context, text)
    }
    fn insert_text(&self, context: &ITfContext, text: String) -> bool {
        // qingjian edit/update.rs request_update: TF_ES_READWRITE only (no TF_ES_SYNC);
        // Win11 Notepad and other immersive hosts can crash on a synchronous lock.
        // SampleIME KeyHandler.cpp uses TF_ES_ASYNCDONTCARE | TF_ES_READWRITE for the same reason.
        let outcome = Rc::new(Cell::new(None));
        let session: ITfEditSession = CommitEdit {
            context: context.clone(),
            text,
            outcome: outcome.clone(),
        }
        .into();
        let accepted = match unsafe {
            context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE)
        } {
            Ok(status) => match status.ok() {
                Ok(()) => {
                    log("上屏编辑会话已受理");
                    true
                }
                Err(error) => {
                    log_hresult("上屏编辑会话返回", &error);
                    false
                }
            },
            Err(error) => {
                log_hresult("上屏编辑会话未被受理", &error);
                false
            }
        };
        if !accepted {
            return false;
        }
        match outcome.get() {
            Some(true) => true,
            Some(false) => {
                log("上屏编辑会话已失败");
                false
            }
            None => {
                log("上屏编辑会话已受理，等待回调写入");
                true
            }
        }
    }
    /// Accept COPYDATA only from the bound YiIME.exe, or bind it on the first packet of a
    /// session started with ShellExecuteW (which does not return a process id).
    fn bind_editor_sender(&self, sender: HWND) -> bool {
        let pid = window_pid(sender);
        if pid == 0 {
            log("COPYDATA 被拒绝：无法取得发送进程");
            return false;
        }
        let expected = self.editor_pid.get();
        if expected != 0 {
            if expected != pid {
                log(&format!(
                    "COPYDATA 被拒绝：发送进程 {pid} 不是已绑定的输入框 {expected}"
                ));
                return false;
            }
            return true;
        }
        if !self.editor_open.get() {
            log("COPYDATA 被拒绝：当前没有输入会话");
            return false;
        }
        let Some(exe) = companion_exe() else {
            log("COPYDATA 被拒绝：找不到 YiIME.exe 路径");
            return false;
        };
        let expected_path = exe.to_string_lossy().to_lowercase();
        if process_image(pid)
            .is_none_or(|path| path.to_string_lossy().to_lowercase() != expected_path)
        {
            log("COPYDATA 被拒绝：发送方不是本机 YiIME.exe");
            return false;
        }
        self.editor_pid.set(pid);
        self.allow_editor_foreground();
        log(&format!("输入框进程已绑定：pid={pid}"));
        true
    }
    fn finish_editor(&self) {
        self.editor_open.set(false);
        self.session_document.borrow_mut().take();
        self.session_window.set(null_mut());
        self.starting_window.set(null_mut());
        self.ready_deadline.set(None);
        self.context.borrow_mut().take();
        self.pending_keys.borrow_mut().clear();
        self.flush_attempts.set(0);
        self.kill_timers();
    }
    fn editor_ready(&self, hwnd: HWND) -> bool {
        self.editor_window.set(hwnd);
        self.starting_window.set(null_mut());
        self.ready_deadline.set(None);
        self.health.borrow_mut().started();
        if !self.receiver.get().is_null() {
            unsafe {
                KillTimer(self.receiver.get(), READY_TIMER);
            }
        }
        // After the box activated the session (it resets privacy then).
        unsafe {
            PostMessageW(self.receiver.get(), WM_SCOPE_READ, 0, 0);
            if self.caret_fresh.get() {
                PostMessageW(self.receiver.get(), WM_CARET_READ, 0, 0);
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
    /// Types letters buffered while the explicit editor was starting into the application.
    fn restore_pending_keys(&self) {
        let pending = std::mem::take(&mut *self.pending_keys.borrow_mut());
        if pending.is_empty() {
            return;
        }
        // Clone before starting the edit session: it may run synchronously and re-enter
        // finish_editor (borrow_mut).
        let context = self.context.borrow().clone();
        if let Some(context) = context {
            let session: ITfEditSession = CommitEdit {
                context: context.clone(),
                text: pending,
                outcome: Rc::new(Cell::new(None)),
            }
            .into();
            let _ = unsafe {
                context.RequestEditSession(self.client_id.get(), &session, TF_ES_READWRITE)
            };
        }
    }
}

impl TextService_Impl {
    /// After the 译/普通五笔 flag changes: 译 opens (or re-uses) the draft box, 普通五笔
    /// ends a draft session so the next letters compose in the document.
    fn apply_plain_mode(&self, plain: bool) {
        log(&format!(
            "输入模式已切换：{}",
            if plain {
                "普通五笔"
            } else {
                "翻译模式"
            }
        ));
        if plain {
            if self.inline.active() {
                // Stay in the document; just make sure a leftover session is consistent.
            }
            if self.buffering() {
                self.drop_stale_session("已切换到普通五笔");
            }
            return;
        }
        if self.inline.active() {
            self.inline.cancel(self.client_id.get());
        }
        if self.buffering() {
            log("翻译模式：草稿框已在使用");
            self.allow_editor_foreground();
            return;
        }
        let Some(context) = self.focused_context() else {
            log("翻译模式：没有焦点输入框，下一键打开草稿框");
            return;
        };
        self.probe_new_field(&context);
        if !self.open_editor(context, 0) {
            log("翻译模式：草稿框未能打开，下一键再试");
        }
    }
    /// Starts reading the InputScope of a field a session opens in, unless already read.
    fn probe_new_field(&self, context: &ITfContext) {
        if !self
            .scope_cache
            .borrow()
            .as_ref()
            .is_some_and(|(cached, _)| cached == context)
        {
            self.probe_scope(context.clone());
        }
    }
    /// Reads the field's InputScope for privacy, asynchronously when TSF cannot do it now.
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
            result,
            receiver: self.receiver.get() as usize,
        }
        .into();
        let _ = unsafe {
            context.RequestEditSession(
                self.client_id.get(),
                &session,
                TF_ES_ASYNCDONTCARE | TF_ES_READ,
            )
        };
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
                let focus_sink: ITfThreadFocusSink = self.to_interface();
                if let Ok(cookie) =
                    unsafe { source.AdviseSink(&ITfThreadFocusSink::IID, &focus_sink) }
                {
                    self.focus_sink_cookie.set(Some(cookie));
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
            let description: Vec<u16> = "打开翻译草稿 Ctrl+Alt+T".encode_utf16().collect();
            let _ = unsafe {
                keystroke.PreserveKey(client_id, &EDITOR_KEY, &EDITOR_CHORD, &description)
            };
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
            if let Some(manager) = self.thread_manager.borrow().as_ref() {
                self.inline.activate(manager);
            }
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
                if let Ok(source) = manager.cast::<ITfSource>() {
                    for cookie in [
                        self.manager_sink_cookie.take(),
                        self.focus_sink_cookie.take(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let _ = unsafe { source.UnadviseSink(cookie) };
                    }
                }
                if let Ok(keystroke) = manager.cast::<ITfKeystrokeMgr>() {
                    let _ = unsafe { keystroke.UnpreserveKey(&TOGGLE_KEY, &TOGGLE_CHORD) };
                    let _ = unsafe { keystroke.UnpreserveKey(&EDITOR_KEY, &EDITOR_CHORD) };
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
            self.inline.deactivate(self.client_id.get());
            self.clear_held();
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
impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, manager: Ref<ITfThreadMgr>, client_id: u32, _flags: u32) -> Result<()> {
        ITfTextInputProcessor_Impl::Activate(self, manager, client_id)
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
        _previous: Ref<ITfDocumentMgr>,
    ) -> Result<()> {
        com_guard("OnSetFocus", || {
            // Same-window document changes must not cancel in-document composition (Chrome,
            // Electron, Word, Explorer). A letter in another field cancels it in wants_key.
            if self.inline.active() {
                let window = document
                    .as_ref()
                    .and_then(|document| unsafe { document.GetTop() }.ok())
                    .map_or(null_mut(), |context| context_window(&context));
                let same_root = same_root(window, unsafe { GetFocus() });
                log(&format!(
                    "焦点文档变化：同一顶层窗口={same_root} → 保持组字"
                ));
            }
            // A change of the focused document never ends the session (as in qingjian, which
            // has no document-manager switch at all): Chromium, Electron, Word and Explorer's
            // search box move it while the box has the keyboard. A letter typed into another
            // field ends it (yiime_core::key_route), and the commit follows the focus
            // (yiime_core::commit_targets).
            if self.buffering()
                && document.is_some()
                && document.as_ref() != self.session_document.borrow().as_ref()
            {
                let window = document
                    .as_ref()
                    .and_then(|document| unsafe { document.GetTop() }.ok())
                    .map_or(null_mut(), |context| context_window(&context));
                let same_root = same_root(window, self.session_window.get());
                log(&format!(
                    "焦点文档变化：同一顶层窗口={same_root} → 保持会话"
                ));
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
    fn OnPushContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        com_guard("OnPushContext", || Ok(()))
    }
    fn OnPopContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        com_guard("OnPopContext", || {
            self.tested_key.borrow_mut().take();
            Ok(())
        })
    }
}
/// Window switches reach here, not ITfKeyEventSink::OnSetFocus (qingjian com/focus.rs). Gaining
/// the focus picks up a mode changed in another application (qingjian service/mode.rs
/// set_thread_focus); losing it hides an empty box. Unlike qingjian it does not commit: the
/// input box itself takes the focus from the application for every session.
impl ITfThreadFocusSink_Impl for TextService_Impl {
    fn OnSetThreadFocus(&self) -> Result<()> {
        com_guard("OnSetThreadFocus", || {
            self.tested_key.borrow_mut().take();
            self.shift_down.set(None);
            self.sync_plain_mode();
            self.refresh_mode_button();
            Ok(())
        })
    }
    fn OnKillThreadFocus(&self) -> Result<()> {
        com_guard("OnKillThreadFocus", || {
            self.inline.cancel(self.client_id.get());
            self.shift_down.set(None);
            self.notify_focus_moved();
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
            // Only a field whose InputScope is not read yet.
            if let Some(context) = context.as_ref()
                && !self
                    .scope_cache
                    .borrow()
                    .as_ref()
                    .is_some_and(|(cached, result)| cached == context && result.get().is_some())
            {
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
