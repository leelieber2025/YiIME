//! Minimal Windows TSF text service. The user edits in the companion window;
//! the translated result is inserted through a TSF edit session.
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
use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
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
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowW, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use yiime_core::{Flag, OptionStore, Options, Quotes, full_width, us_key_char};

const CLSID: GUID = GUID::from_u128(0xa72091c2_7459_4c47_85f4_4626ae42a6c8);
const CLSID_TEXT: &str = "{A72091C2-7459-4C47-85F4-4626AE42A6C8}";
const PROFILE: GUID = GUID::from_u128(0x54a39d73_0625_4560_b878_94660908c69d);
const TOGGLE_KEY: GUID = GUID::from_u128(0x39c04f7e_c659_4d9e_ad8e_55929249bb77);
const TOGGLE_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x20,
    uModifiers: TF_MOD_CONTROL,
};
/// Reserved key for switching "normal Wubi mode (no translation)"; the key combination is taken
/// from HKCU\\Software\\InputTranslation\\PlainToggleKey.
const PLAIN_KEY: GUID = GUID::from_u128(0x6b1f2d0e_93c4_4e1a_9f6d_2a7c51e0b8d3);
/// Ctrl+.: Chinese/English punctuation; Ctrl+Shift+F: Simplified/Traditional output (same as
/// Microsoft Wubi).
const PUNCT_KEY: GUID = GUID::from_u128(0x0d6c5e21_7a43_4f8e_b1d2_6e9a3c4f5b71);
const PUNCT_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0xBE,
    uModifiers: TF_MOD_CONTROL,
};
/// Input memory: Call up the floating box and load the most recently translated Chinese draft
/// (default Ctrl+Shift+Z, HKCU …\\HistoryKey).
const HISTORY_KEY: GUID = GUID::from_u128(0x2f4b8c6a_51d3_4e07_9a2c_7b1e0d3f6a58);
/// When the "first key" sent to the editor is 1, it means entering memory retrieval, not letters.
const RECALL: u8 = 1;
const TRAD_KEY: GUID = GUID::from_u128(0x5a8e7f13_2c64_4b9d_8e1f_3d7b6a2c9e04);
const TRAD_CHORD: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: b'F' as u32,
    uModifiers: TF_MOD_CONTROL | TF_MOD_SHIFT,
};
const STATE_KEY: &str = "Software\\InputTranslation";
const LANG_ZH_CN: u16 = 0x0804;
const DESCRIPTION: &str = "YiIME 译（五笔）";
const WINDOW_CLASS: &str = "InputTranslationTsfReceiver";
/// Timer: 1 = Retry forwarding cached keys to the editor; 2 = Timeout waiting for the editor to be
/// ready.
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
                .is_some_and(|name| name.eq_ignore_ascii_case("InputTranslation-Portable.exe"))
    })
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

/// Registered TIP capabilities. SYSTRAYSUPPORT is the prerequisite for the Win8+ taskbar input
/// indicator to display the "mode icon" (GUID_LBI_INPUTMODE) of this input method and respond to
/// its click; without it, the system only displays the brand icon and does not call the mode button
/// (this was the case before v9). Not registering IMMERSIVESUPPORT: The UWP/AppContainer host
/// cannot launch the floating box EXE, write HKCU or send messages across integrity levels.
/// Declaring support will cause problems in these applications (Microsoft documentation:
/// SYSTRAYSUPPORT is used for input methods that "do not support immersive but are compatible with
/// the tray").
const CATEGORIES: [GUID; 4] = [
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_COMLESS,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
];
/// Lightweight diagnostic log: %TEMP%\YiIME-tsf.log (over 512 KB rotated to .old). If it cannot be
/// written (AppContainer, etc.), it will be ignored.
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
fn register() -> Result<()> {
    let key = CLASSES_ROOT.create(format!("CLSID\\{CLSID_TEXT}"))?;
    key.set_string("", DESCRIPTION)?;
    let inproc = key.create("InprocServer32")?;
    inproc.set_string("", module_path()?)?;
    inproc.set_string("ThreadingModel", "Apartment")?;
    com_scope(|| {
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        let description = wide(DESCRIPTION);
        let icon = wide(&module_path()?);
        unsafe {
            profiles.Register(&CLSID)?;
            profiles.AddLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE, &description, &icon, 0)?;
            let _ = profiles.EnableLanguageProfileByDefault(&CLSID, LANG_ZH_CN, &PROFILE, true);
        }
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        for category in CATEGORIES {
            unsafe {
                categories.RegisterCategory(&CLSID, &category, &CLSID)?;
            }
        }
        Ok(())
    })
}

fn unregister() -> Result<()> {
    com_scope(|| {
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        let _ = unsafe {
            categories.UnregisterCategory(&CLSID, &GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, &CLSID)
        };
        for category in CATEGORIES {
            absent_is_ok(unsafe { categories.UnregisterCategory(&CLSID, &category, &CLSID) })?;
        }
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        absent_is_ok(unsafe { profiles.RemoveLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE) })?;
        absent_is_ok(unsafe { profiles.Unregister(&CLSID) })?;
        Ok(())
    })?;
    absent_is_ok(CLASSES_ROOT.remove_tree(format!("CLSID\\{CLSID_TEXT}")))?;
    Ok(())
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
        if !outer.is_null() {
            return CLASS_E_NOAGGREGATION.ok();
        }
        if iid.is_null() || out.is_null() {
            return E_FAIL.ok();
        }
        let object: IUnknown = TextService::new().into();
        unsafe { object.query(iid, out).ok() }
    }
    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[implement(ITfEditSession)]
struct CommitEdit {
    context: ITfContext,
    text: String,
}
impl ITfEditSession_Impl for CommitEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
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
    }
}

#[implement(ITfEditSession)]
struct ScopeCheck {
    context: ITfContext,
    result: Rc<Cell<Option<bool>>>,
}
impl ITfEditSession_Impl for ScopeCheck_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
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
            // When the selection cannot be read, it will be treated as a normal input box,
            // otherwise the input method will never be activated in this context.
            drop(ManuallyDrop::into_inner(selection.range));
            unsafe {
                OutputDebugStringW(
                    wide("InputTranslation: 读取选区失败，按非密码输入框处理").as_ptr(),
                );
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
    }
}

/// Read the screen rectangle of the insertion point and display the editor's floating box against
/// the cursor of the target application.
#[implement(ITfEditSession)]
struct CaretQuery {
    context: ITfContext,
    result: Rc<Cell<Option<RECT>>>,
}
impl ITfEditSession_Impl for CaretQuery_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
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
    }
}

/// Chinese/English mode for read and write threads (GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, 1 =
/// Chinese).
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
    // At the same time, the NATIVE bit of the "conversion mode" compartment is maintained (other
    // bits are reserved): press it on the taskbar and touch keyboard to determine Chinese/English.
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
/// Read "Conversion Mode"; if it is not set, it will be calculated as Chinese (NATIVE).
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
/// A floating box/tray EXE in the same directory as this DLL; if it cannot be found, use the
/// current version directory recorded by the installation script.
fn companion_exe() -> Option<std::path::PathBuf> {
    let path = module_path().ok()?;
    let editor = std::path::Path::new(&path)
        .parent()?
        .join("InputTranslation-Portable.exe");
    if editor.is_file() {
        return Some(editor);
    }
    LOCAL_MACHINE
        .open("Software\\InputTranslation")
        .and_then(|key| key.get_string("CurrentDir"))
        .ok()
        .map(|dir| std::path::PathBuf::from(dir).join("InputTranslation-Portable.exe"))
        .filter(|path| path.is_file())
}
/// Let the tray instance open settings (WM_APP+5) or input memory (WM_APP+6); start the EXE with
/// parameters when the tray is not running.
fn open_tray_window(history: bool) {
    unsafe {
        AllowSetForegroundWindow(u32::MAX); // ASFW_ANY: The front desk power is on the order menu side and is given to the tray process.
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
/// The "Chinese/English" mode button on the left side of the taskbar input method icon (the same
/// GUID_LBI_INPUTMODE as Microsoft Pinyin/Wubi). Click to switch modes; monitor OPENCLOSE
/// compartment, Ctrl+Space or other ways to refresh the display when changing modes.
#[implement(ITfLangBarItemButton, ITfSource, ITfCompartmentEventSink)]
struct InputModeButton {
    manager: ITfThreadMgr,
    client_id: u32,
    /// Shared with the TextService to facilitate a refresh button when the mode (normal/translated)
    /// changes elsewhere.
    sink: LangBarSink,
}
type LangBarSink = Rc<RefCell<Option<ITfLangBarItemSink>>>;
fn refresh_sink(sink: &LangBarSink) {
    let sink = sink.borrow().clone();
    if let Some(sink) = sink {
        let _ = unsafe { sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP) };
    }
}
const MENU_CHINESE: u32 = 1;
const MENU_ENGLISH: u32 = 2;
const MENU_TRANSLATE: u32 = 3;
const MENU_PLAIN: u32 = 4;
const MENU_PUNCT: u32 = 5;
const MENU_TRAD: u32 = 6;
const MENU_HISTORY: u32 = 7;
const MENU_SETTINGS: u32 = 8;
impl InputModeButton_Impl {
    fn chinese(&self) -> bool {
        keyboard_mode(&self.manager).unwrap_or(true)
    }
    /// The menu is hung on the window of this thread: the window where the focus input box is
    /// located, and then the foreground/focus window of this thread (the same as Xiaolanghao).
    fn menu_owner(&self) -> HWND {
        let from_context = unsafe {
            self.manager
                .GetFocus()
                .and_then(|document| document.GetTop())
                .and_then(|context| context.GetActiveView())
                .and_then(|view| view.GetWnd())
        }
        .ok()
        .map(|hwnd| hwnd.0 as HWND)
        .filter(|hwnd| !hwnd.is_null());
        from_context
            .or_else(|| {
                let foreground = unsafe { GetForegroundWindow() };
                let thread = unsafe { GetWindowThreadProcessId(foreground, null_mut()) };
                (thread == unsafe { GetCurrentThreadId() }).then_some(foreground)
            })
            .or_else(|| Some(unsafe { GetFocus() }))
            .filter(|hwnd| !hwnd.is_null())
            .unwrap_or(null_mut())
    }
    /// The right-click menu of the taskbar mode icon: Chinese/English, Translation/Common Wubi,
    /// Punctuation, Simplified and Traditional, Input Memory, and Settings.
    fn show_menu(&self, x: i32, y: i32) {
        let owner = self.menu_owner();
        if owner.is_null() {
            log("右键菜单：找不到本线程的窗口");
            return;
        }
        let chinese = self.chinese();
        let plain = plain_mode();
        let checked = |on: bool| if on { MF_CHECKED } else { 0 };
        let history = if option(Flag::HistoryEnabled) {
            let key = OPTIONS.history_key();
            format!("输入记忆…\t{key}")
        } else {
            "输入记忆…（已关闭）".to_owned()
        };
        let items: [Option<(u32, String, u32)>; 11] = [
            Some((MENU_CHINESE, "中文".into(), checked(chinese))),
            Some((MENU_ENGLISH, "英文".into(), checked(!chinese))),
            None,
            Some((
                MENU_TRANSLATE,
                format!("翻译模式（译）\t{}", plain_toggle_text()),
                checked(!plain),
            )),
            Some((
                MENU_PLAIN,
                format!("普通五笔模式（中）\t{}", plain_toggle_text()),
                checked(plain),
            )),
            None,
            Some((
                MENU_PUNCT,
                "中文标点\tCtrl+.".into(),
                checked(!option(Flag::PunctEnglish)),
            )),
            Some((
                MENU_TRAD,
                "繁体输出\tCtrl+Shift+F".into(),
                checked(option(Flag::Traditional)),
            )),
            None,
            Some((MENU_HISTORY, history, 0)),
            Some((MENU_SETTINGS, "设置…".into(), 0)),
        ];
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        for item in items {
            match item {
                Some((id, text, flags)) => unsafe {
                    AppendMenuW(menu, MF_STRING | flags, id as usize, wide(&text).as_ptr());
                },
                None => unsafe {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                },
            }
        }
        // The icon is on the taskbar and the menu pops up; TPM_RETURNCMD retrieves the selected
        // item synchronously.
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let id = unsafe { TrackPopupMenuEx(menu, flags, x, y, owner, std::ptr::null()) } as u32;
        unsafe {
            DestroyMenu(menu);
        }
        log(&format!("右键菜单选择 {id}"));
        match id {
            MENU_CHINESE | MENU_ENGLISH => {
                set_keyboard_mode(&self.manager, self.client_id, id == MENU_CHINESE);
            }
            MENU_TRANSLATE | MENU_PLAIN => {
                set_plain_mode(id == MENU_PLAIN);
                if !chinese {
                    set_keyboard_mode(&self.manager, self.client_id, true);
                }
            }
            MENU_PUNCT => {
                toggle_option(Flag::PunctEnglish);
            }
            MENU_TRAD => {
                toggle_option(Flag::Traditional);
            }
            MENU_HISTORY => open_tray_window(true),
            MENU_SETTINGS => open_tray_window(false),
            _ => {}
        }
    }
    fn refresh(&self) {
        refresh_sink(&self.sink);
    }
}
/// Normal Wubi mode (no translation): shares HKCU\\Software\\InputTranslation\\PlainMode with the
/// editor EXE. Storage of setting switches: HKCU\\Software\\InputTranslation (shared with floating
/// box EXE, name and default value are defined by yiime-core).
struct Registry;
impl OptionStore for Registry {
    fn get_u32(&self, name: &str) -> Option<u32> {
        CURRENT_USER
            .open(STATE_KEY)
            .and_then(|key| key.get_u32(name))
            .ok()
    }
    fn set_u32(&self, name: &str, value: u32) -> bool {
        CURRENT_USER
            .create(STATE_KEY)
            .and_then(|key| key.set_u32(name, value))
            .is_ok()
    }
    fn get_string(&self, name: &str) -> Option<String> {
        CURRENT_USER
            .open(STATE_KEY)
            .and_then(|key| key.get_string(name))
            .ok()
    }
    fn set_string(&self, name: &str, value: &str) -> bool {
        CURRENT_USER
            .create(STATE_KEY)
            .and_then(|key| key.set_string(name, value))
            .is_ok()
    }
}
const OPTIONS: Options<Registry> = Options(Registry);
fn option(flag: Flag) -> bool {
    OPTIONS.flag(flag)
}
fn toggle_option(flag: Flag) -> bool {
    OPTIONS.toggle(flag).is_some()
}
fn plain_mode() -> bool {
    option(Flag::PlainMode)
}
fn set_plain_mode(on: bool) -> bool {
    OPTIONS.set_flag(Flag::PlainMode, on)
}
/// Chinese punctuation when the input box is not open (virtual key + Shift of the American keyboard
/// layout, mapped in yiime-core). . and : after numbers remain half-width. Quotes are handled by
/// the caller on an open/closed basis, and None is returned here.
fn chinese_punctuation(key: usize, shift: bool, after_digit: bool) -> Option<&'static str> {
    let key = u16::try_from(key).ok()?;
    us_key_char(key, shift).and_then(|ch| full_width(ch, after_digit))
}
/// Parse shortcut keys such as `Ctrl+Shift+Space` (the rules are in yiime-core) and convert them
/// into TSF reserved keys.
fn parse_hotkey(text: &str) -> Option<TF_PRESERVEDKEY> {
    let hotkey = yiime_core::parse_hotkey(text)?;
    let mut modifiers = 0u32;
    if hotkey.ctrl {
        modifiers |= TF_MOD_CONTROL;
    }
    if hotkey.shift {
        modifiers |= TF_MOD_SHIFT;
    }
    if hotkey.alt {
        modifiers |= TF_MOD_ALT;
    }
    Some(TF_PRESERVEDKEY {
        uVKey: hotkey.vk as u32,
        uModifiers: modifiers,
    })
}
fn plain_toggle_text() -> String {
    OPTIONS.plain_toggle_key()
}
fn light_taskbar() -> bool {
    windows_registry::CURRENT_USER
        .open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|value| value != 0)
}
impl ITfLangBarItem_Impl for InputModeButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        let mut description = [0u16; 32];
        for (slot, unit) in description.iter_mut().zip("中英模式".encode_utf16()) {
            *slot = unit;
        }
        unsafe {
            *info = TF_LANGBARITEMINFO {
                clsidService: CLSID,
                guidItem: GUID_LBI_INPUTMODE,
                // The same as the Microsoft input method: only BTN_BUTTON is used; the right-click
                // menu is opened by OnClick(RIGHT) itself (the input indicator of Win10/11 no
                // longer calls InitMenu).
                dwStyle: TF_LBI_STYLE_BTN_BUTTON,
                ulSort: 0,
                szDescription: description,
            };
        }
        Ok(())
    }
    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }
    fn Show(&self, _show: BOOL) -> Result<()> {
        Ok(())
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        let toggle = plain_toggle_text();
        let extras = format!(
            " · {} · {}",
            if option(Flag::PunctEnglish) {
                "英文标点"
            } else {
                "中文标点"
            },
            if option(Flag::Traditional) {
                "繁体"
            } else {
                "简体"
            }
        );
        Ok(BSTR::from(
            match (self.chinese(), plain_mode()) {
                (false, _) => "英文模式（单击、Shift 或 Ctrl+Space 切到中文）".to_owned(),
                (true, false) => {
                    format!(
                        "译：翻译模式（单击/Shift：中英切换；{toggle}：普通五笔模式；右键：菜单）"
                    )
                }
                (true, true) => format!(
                    "中：普通五笔模式，不翻译（单击/Shift：中英切换；{toggle}：翻译模式；右键：菜单）"
                ),
            } + &extras,
        ))
    }
}
impl ITfLangBarItemButton_Impl for InputModeButton_Impl {
    fn OnClick(
        &self,
        click: TfLBIClick,
        point: &windows::Win32::Foundation::POINT,
        _area: *const RECT,
    ) -> Result<()> {
        log(&format!("模式按钮 OnClick {}", click.0));
        if click == TF_LBI_CLK_RIGHT {
            self.show_menu(point.x, point.y);
        } else {
            set_keyboard_mode(&self.manager, self.client_id, !self.chinese());
        }
        self.refresh();
        Ok(())
    }
    fn InitMenu(&self, _menu: Ref<ITfMenu>) -> Result<()> {
        Ok(())
    }
    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        Ok(())
    }
    fn GetIcon(&self) -> Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
        // Resources 101/102/105: Light taskbar (dark text) Chinese/English/Translation;
        // 103/104/106: Dark taskbar (white text). The translation mode displays "Yi", the normal
        // Wubi mode displays "Chinese", and the English display "Eng". The caller is responsible
        // for the DestroyIcon.
        let light = light_taskbar();
        let id = match (self.chinese(), plain_mode()) {
            (false, _) => {
                if light {
                    102
                } else {
                    104
                }
            }
            (true, true) => {
                if light {
                    101
                } else {
                    103
                }
            }
            (true, false) => {
                if light {
                    105
                } else {
                    106
                }
            }
        };
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
        let icon = unsafe {
            LoadImageW(
                MODULE.load(Ordering::SeqCst) as _,
                std::ptr::without_provenance::<u16>(id),
                IMAGE_ICON,
                size,
                size,
                0,
            )
        };
        if icon.is_null() {
            log(&format!("模式图标资源 {id} 加载失败"));
            Err(E_FAIL.into())
        } else {
            Ok(windows::Win32::UI::WindowsAndMessaging::HICON(icon))
        }
    }
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(match (self.chinese(), plain_mode()) {
            (false, _) => "英",
            (true, true) => "中",
            (true, false) => "译",
        }))
    }
}
impl ITfSource_Impl for InputModeButton_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Ref<IUnknown>) -> Result<u32> {
        if riid.is_null() || unsafe { *riid } != ITfLangBarItemSink::IID {
            return Err(HRESULT(0x8004_0202_u32 as i32).into()); // CONNECT_E_CANNOTCONNECT
        }
        let sink: ITfLangBarItemSink = punk.ok()?.cast()?;
        *self.sink.borrow_mut() = Some(sink);
        Ok(LANGBAR_SINK_COOKIE)
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie != LANGBAR_SINK_COOKIE {
            return Err(HRESULT(0x8004_0200_u32 as i32).into()); // CONNECT_E_NOCONNECTION
        }
        self.sink.borrow_mut().take();
        Ok(())
    }
}
impl ITfCompartmentEventSink_Impl for InputModeButton_Impl {
    fn OnChange(&self, guid: *const GUID) -> Result<()> {
        if guid.is_null() {
            return Ok(());
        }
        let guid = unsafe { *guid };
        if guid == GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION {
            // The system (taskbar/touch keyboard) changed the NATIVE bit of the conversion mode:
            // let the on/off follow up (the same value is not written to avoid triggering back and
            // forth).
            if let Some(mode) = conversion_mode(&self.manager) {
                let native = mode & TF_CONVERSIONMODE_NATIVE as i32 != 0;
                if native != self.chinese() {
                    set_keyboard_mode(&self.manager, self.client_id, native);
                }
            }
            self.refresh();
        } else if guid == GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
            self.refresh();
        }
        Ok(())
    }
}
/// The maximum hold time for clicking Shift to switch between Chinese/English (exceeding it is
/// regarded as Shift+mouse selection and other uses).
const SHIFT_TAP_MAX: std::time::Duration = std::time::Duration::from_millis(600);
fn is_shift(key: usize) -> bool {
    matches!(key, 0x10 | 0xA0 | 0xA1)
}
fn caps_lock() -> bool {
    (unsafe { GetKeyState(VK_CAPITAL as i32) } & 1) != 0
}
/// Letters cached during float startup preserve case (Shift XORed with Caps Lock): lowercase is
/// encoded, uppercase is put into draft by the float as is.
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
/// A modal button has been added to the language bar, and its event cookie on the OPENCLOSE
/// compartment.
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
    editor: RefCell<Option<std::process::Child>>,
    editor_window: Cell<HWND>,
    pending_keys: RefCell<String>,
    first_key: Cell<Option<char>>,
    flush_attempts: Cell<u8>,
    scope_cache: RefCell<ScopeCache>,
    manager_sink_cookie: Cell<Option<u32>>,
    edit_sink: RefCell<Option<(ITfContext, u32)>>,
    tested_key: RefCell<Option<(ITfContext, usize, bool)>>,
    mode_button: RefCell<ModeButton>,
    /// The moment when Shift is pressed alone; cleared if other keys are pressed during the period.
    /// If it is still there and short enough when released, switch to Chinese/English.
    shift_down: Cell<Option<std::time::Instant>>,
    /// The registered "normal mode" reserved keys are activated this time (the same combination
    /// must be used when logging out).
    plain_chord: Cell<Option<TF_PRESERVEDKEY>>,
    history_chord: Cell<Option<TF_PRESERVEDKEY>>,
    /// Whether the last key passed directly to the application was a number (determines whether . :
    /// remains half-width).
    after_digit: Cell<bool>,
    /// Chinese quotation marks open/close alternately: (single quotation mark, double quotation
    /// mark).
    quotes: Cell<Quotes>,
    /// The normal mode state of the mode button when it was last displayed; refreshes the button
    /// accordingly when the tray or other application changes it.
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
            editor_window: Cell::new(null_mut()),
            pending_keys: RefCell::new(String::new()),
            first_key: Cell::new(None),
            flush_attempts: Cell::new(0),
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
        if exited {
            self.reset_editor();
        }
    }
    fn reset_editor(&self) {
        self.kill_timers();
        self.editor_open.set(false);
        self.editor.borrow_mut().take();
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
    /// An input session is in progress (the editor is starting, or the floating box is displayed):
    /// the letters pressed in the host are cached and transferred to the floating box, and appended
    /// to the current encoding. In the past, when the floating box was not brought to the front,
    /// each letter would be reactivated and the code would be cleared, which would appear as
    /// "letters coming out of the box one by one." The session ends (up screen, cancel, lose focus)
    /// and is closed only after being notified by the editor.
    fn buffering(&self) -> bool {
        self.editor_open.get()
    }
    /// Allow the editor process to bring the floating box to the foreground (only the current
    /// foreground process, the host, can grant this).
    fn allow_editor_foreground(&self) {
        if let Some(child) = self.editor.borrow().as_ref() {
            unsafe {
                let _ = AllowSetForegroundWindow(child.id());
            }
        }
    }
    /// Synchronously reads the insertion point screen rectangle; can only be called when the
    /// document lock can be obtained, such as key processing.
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
        if self.buffering() {
            return self
                .context
                .borrow()
                .as_ref()
                .is_some_and(|active| active == context);
        }
        // No active session: Start a new typing of letters without modifier keys. Shift+letters,
        // letters when Caps Lock is turned on are handed over to the application as usual (input
        // capitals/letters directly, no pop-up box, consistent with Microsoft Wubi).
        !caps_lock()
            && !modifier(VK_CONTROL)
            && !modifier(VK_MENU)
            && !modifier(VK_LWIN)
            && !modifier(VK_RWIN)
            && !modifier(VK_SHIFT)
    }
    /// When the input box is not opened, in Chinese mode, and Chinese punctuation is used, this key
    /// should be converted into Chinese punctuation.
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
    /// Insert text directly into the current document (without going through the floating box).
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
    /// When switching to this input method, it always enters Chinese mode. The OPENCLOSE
    /// compartment is shared by all input methods in the same thread. If the "English" status left
    /// by other input methods is used, the keys will go up the screen directly and the floating box
    /// will not appear.
    fn ensure_keyboard_mode(manager: &ITfThreadMgr, client_id: u32) {
        set_keyboard_mode(manager, client_id, true);
    }
    fn toggle_keyboard_mode(&self) -> bool {
        let manager = self.thread_manager.borrow();
        let Some(manager) = manager.as_ref() else {
            return false;
        };
        // The mode button monitors compartment changes and automatically refreshes the
        // "Chinese/English" display.
        set_keyboard_mode(manager, self.client_id.get(), !self.keyboard_open())
    }
    /// Tracking "Shift alone": Time is recorded when Shift is pressed, canceled by any other key
    /// press.
    fn track_key_down(&self, key: usize) {
        if is_shift(key) {
            let modifier = |vk| unsafe { GetKeyState(vk as i32) } < 0;
            if modifier(VK_CONTROL) || modifier(VK_MENU) || modifier(VK_LWIN) || modifier(VK_RWIN) {
                self.shift_down.set(None);
            } else if self.shift_down.get().is_none() {
                // Automatic repeated presses do not refresh the time.
                self.shift_down.set(Some(std::time::Instant::now()));
            }
        } else {
            self.shift_down.set(None);
        }
    }
    /// Release Shift: If no other keys are pressed during this period and the time is short enough,
    /// switch between Chinese and English (synchronized with Ctrl+Space and language bar buttons,
    /// both written as OPENCLOSE compartment). Drafts and coding in the floating box are not
    /// affected.
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
    fn refresh_mode_button(&self) {
        if let Some((_, _, sink)) = self.mode_button.borrow().as_ref() {
            refresh_sink(sink);
        }
    }
    /// Normal/translation mode may have been changed elsewhere (tray menu, shortcut keys in other
    /// applications): refresh button if changed.
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
        // The normal mode also supports Chinese input: when switching from English, it returns to
        // Chinese as well.
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
        // Monitor the two compartments of "on/off" (Ctrl+Space, Shift, click) and "conversion mode"
        // (taskbar, touch keyboard).
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
    fn open_editor(&self, context: ITfContext, first_key: u8) -> bool {
        let target = unsafe { GetForegroundWindow() } as usize;
        let window = self.editor_window.get();
        // The remaining cached keys from the previous session cannot be transferred to this time.
        self.pending_keys.borrow_mut().clear();
        let caret = self.caret_rect(&context);
        if !window.is_null() && unsafe { IsWindow(window) } != 0 {
            if let Some(child) = self.editor.borrow().as_ref() {
                unsafe {
                    AllowSetForegroundWindow(child.id());
                }
            }
            *self.context.borrow_mut() = Some(context.clone());
            self.first_key.set(
                first_key
                    .is_ascii_alphabetic()
                    .then(|| (first_key as char).to_ascii_lowercase()),
            );
            self.editor_open.set(true);
            self.editor_window.set(null_mut());
            // First send the cursor rectangle synchronously (if it cannot be retrieved, send an
            // empty packet to let the editor position itself), and then activate it asynchronously.
            let mut rect = caret.map(|rect| [rect.left, rect.top, rect.right, rect.bottom]);
            let packet = COPYDATASTRUCT {
                dwData: 3,
                cbData: if rect.is_some() { 16 } else { 0 },
                lpData: rect
                    .as_mut()
                    .map_or(null_mut(), |rect| rect.as_mut_ptr() as _),
            };
            let mut result = 0usize;
            unsafe {
                SendMessageTimeoutW(
                    window,
                    WM_COPYDATA,
                    self.receiver.get() as usize,
                    &packet as *const _ as isize,
                    SMTO_ABORTIFHUNG,
                    100,
                    &mut result,
                );
            }
            if unsafe { PostMessageW(window, WM_APP + 2, target, first_key as isize) } != 0 {
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
        // Priority is given to using an editor in the same directory (same version) as this DLL;
        // when the EXE in the old version directory is deleted, it will return to the current
        // version directory recorded by the installation script
        // (HKLM\\Software\\InputTranslation\\CurrentDir).
        let mut editor = directory.join("InputTranslation-Portable.exe");
        if !editor.is_file() {
            let Some(current) = LOCAL_MACHINE
                .open("Software\\InputTranslation")
                .and_then(|key| key.get_string("CurrentDir"))
                .ok()
                .map(|dir| std::path::PathBuf::from(dir).join("InputTranslation-Portable.exe"))
                .filter(|path| path.is_file())
            else {
                return false;
            };
            editor = current;
        }
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
            // The host's working directory and standard handles are not inherited.
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if first_key == RECALL {
            command.arg("--recall");
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
        let sent = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                self.receiver.get() as usize,
                &packet as *const _ as isize,
                SMTO_ABORTIFHUNG,
                100,
                &mut result,
            ) != 0
                && result != 0
        };
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
        // Clone first and then initiate an editing session: the session may be executed
        // synchronously and re-enter finish_editor(borrow_mut).
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
                OutputDebugStringW(wide(&format!(
                    "InputTranslation: InputScope 查询失败；此输入框中输入法暂不可用。宿主：{host}"
                )).as_ptr());
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
        let manager = manager.ok()?.clone();
        // Complete the cast that may fail first, and then create a receiving window holding the
        // self pointer to avoid leaving a dangling pointer when it fails.
        let keystroke: ITfKeystrokeMgr = manager.cast()?;
        let receiver = unsafe { create_receiver(self as *const _ as *mut c_void) };
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
        if let Err(error) =
            unsafe { keystroke.PreserveKey(client_id, &TOGGLE_KEY, &TOGGLE_CHORD, &description) }
        {
            unsafe {
                OutputDebugStringW(
                    wide(&format!("InputTranslation: Ctrl+Space 注册失败：{error}")).as_ptr(),
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
        let history_text = OPTIONS.history_key();
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
            if unsafe { keystroke.PreserveKey(client_id, &PLAIN_KEY, &chord, &description) }.is_ok()
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
    }
    fn Deactivate(&self) -> Result<()> {
        let editor_window = self.editor_window.get();
        if !editor_window.is_null() {
            unsafe {
                PostMessageW(editor_window, WM_APP + 3, 0, 0);
            }
        } else if let Some(child) = self.editor.borrow_mut().as_mut() {
            let _ = child.kill();
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
    }
}
impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        self.tested_key.borrow_mut().take();
        self.shift_down.set(None);
        if foreground.as_bool() {
            self.sync_plain_mode();
        }
        if foreground.as_bool()
            && let Some(manager) = self.thread_manager.borrow().as_ref()
            && let Ok(document) = unsafe { manager.GetFocus() }
            && let Ok(context) = unsafe { document.GetTop() }
        {
            self.probe_scope(context);
        }
        Ok(())
    }
    fn OnTestKeyDown(
        &self,
        context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        self.track_key_down(key.0);
        let consume = context.as_ref().is_some_and(|context| {
            self.should_consume(context, key.0) || self.punctuation(context, key.0).is_some()
        });
        *self.tested_key.borrow_mut() = context
            .as_ref()
            .map(|context| (context.clone(), key.0, consume));
        Ok(consume.into())
    }
    fn OnKeyDown(
        &self,
        context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        self.track_key_down(key.0);
        let Some(context) = context.as_ref() else {
            return Ok(false.into());
        };
        let tested = self.tested_key.borrow_mut().take();
        let consume = tested
            .as_ref()
            .filter(|(tested_context, tested_key, _)| {
                tested_context == context && *tested_key == key.0
            })
            .map(|(_, _, consume)| *consume)
            .unwrap_or_else(|| self.should_consume(context, key.0));
        if !self.buffering()
            && !self.should_consume(context, key.0)
            && let Some(text) = self.punctuation(context, key.0)
        {
            if key.0 == 0xDE {
                let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                let mut quotes = self.quotes.get();
                quotes.next(if shift { '"' } else { '\'' });
                self.quotes.set(quotes);
            }
            self.after_digit.set(false);
            return Ok(self.insert_direct(context, text).into());
        }
        // Record the numeric keys so that the . and : that follow them are half-width (3.5, 10:30).
        let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
        self.after_digit
            .set((!shift && (0x30..=0x39).contains(&key.0)) || (0x60..=0x69).contains(&key.0));
        // Only letters will be given to the floating box; during the test phase, the keys that are
        // eaten by punctuation and whose conditions have changed are released.
        if !consume || !(0x41..=0x5a).contains(&key.0) {
            return Ok(false.into());
        }
        if self.buffering() {
            self.pending_keys
                .borrow_mut()
                .push(typed_letter(key.0 as u8));
            if !self.editor_window.get().is_null() {
                self.allow_editor_foreground();
                unsafe {
                    PostMessageW(self.receiver.get(), WM_APP + 1, 0, 0);
                }
            }
            return Ok(true.into());
        }
        Ok(self.open_editor(context.clone(), key.0 as u8).into())
    }
    fn OnTestKeyUp(
        &self,
        _context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        // If you release Shift without eating it, the host will receive it as usual; the switch
        // will only be done once (track_key_up will clear the record).
        self.track_key_up(key.0);
        Ok(false.into())
    }
    fn OnKeyUp(
        &self,
        _context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        self.track_key_up(key.0);
        Ok(false.into())
    }
    fn OnPreservedKey(&self, context: Ref<ITfContext>, guid: *const GUID) -> Result<BOOL> {
        // It is not processed in the editor process (floating box): the floating box responds to
        // these two shortcut keys by itself and retains the group words and drafts.
        if running_in_editor() || guid.is_null() {
            return Ok(false.into());
        }
        let guid = unsafe { *guid };
        if guid == HISTORY_KEY {
            // Allowed when input memory is off, floating box is on, English mode, or password box
            // (the app receives this key combination as usual).
            self.refresh_editor();
            let Some(context) = context.as_ref() else {
                return Ok(false.into());
            };
            if !option(Flag::HistoryEnabled)
                || self.buffering()
                || !self.keyboard_open()
                || TextService::disabled(context)
                || self.password_scope(context)
            {
                return Ok(false.into());
            }
            return Ok(self.open_editor(context.clone(), RECALL).into());
        }
        let toggled = (guid == TOGGLE_KEY && self.toggle_keyboard_mode())
            || (guid == PLAIN_KEY && self.toggle_plain_mode())
            || (guid == PUNCT_KEY && toggle_option(Flag::PunctEnglish))
            || (guid == TRAD_KEY && toggle_option(Flag::Traditional));
        if toggled {
            self.refresh_mode_button();
        }
        Ok(toggled.into())
    }
}
impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnUninitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }
    fn OnSetFocus(
        &self,
        document: Ref<ITfDocumentMgr>,
        _previous: Ref<ITfDocumentMgr>,
    ) -> Result<()> {
        self.scope_cache.borrow_mut().take();
        self.tested_key.borrow_mut().take();
        self.sync_plain_mode();
        if let Some(document) = document.as_ref()
            && let Ok(context) = unsafe { document.GetTop() }
        {
            self.probe_scope(context);
        }
        Ok(())
    }
    fn OnPushContext(&self, context: Ref<ITfContext>) -> Result<()> {
        if let Some(context) = context.as_ref() {
            self.probe_scope(context.clone());
        }
        Ok(())
    }
    fn OnPopContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        self.scope_cache.borrow_mut().take();
        self.tested_key.borrow_mut().take();
        Ok(())
    }
}
impl ITfTextEditSink_Impl for TextService_Impl {
    fn OnEndEdit(
        &self,
        context: Ref<ITfContext>,
        _cookie: u32,
        _record: Ref<ITfEditRecord>,
    ) -> Result<()> {
        if let Some(context) = context.as_ref() {
            self.probe_scope(context.clone());
        }
        Ok(())
    }
}

unsafe fn create_receiver(service: *mut c_void) -> HWND {
    let instance = MODULE.load(Ordering::SeqCst) as _;
    let class = wide(WINDOW_CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(receiver_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&wc) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return null_mut();
    }
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide("").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            service,
        )
    }
}
unsafe extern "system" fn receiver_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_CREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return 0;
    }
    if message == WM_COPYDATA {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if pointer.is_null() || l == 0 {
            return 0;
        }
        let mut sender_pid = 0;
        unsafe {
            GetWindowThreadProcessId(w as HWND, &mut sender_pid);
        }
        if sender_pid == 0
            || unsafe {
                (&(*pointer))
                    .editor
                    .borrow()
                    .as_ref()
                    .map(std::process::Child::id)
            } != Some(sender_pid)
        {
            return 0;
        }
        let packet = unsafe { &*(l as *const COPYDATASTRUCT) };
        if packet.dwData == 1 {
            let ready = unsafe { (*pointer).editor_ready(w as HWND) };
            if ready {
                unsafe {
                    PostMessageW(hwnd, WM_APP + 1, 0, 0);
                }
            }
            return ready as LRESULT;
        }
        if packet.dwData != 0 {
            return 0;
        }
        if packet.cbData == 0 {
            return unsafe { (*pointer).commit(String::new()) as LRESULT };
        }
        if packet.cbData as usize > 1_000_000
            || !packet.cbData.is_multiple_of(2)
            || packet.lpData.is_null()
        {
            return 0;
        }
        let units = unsafe {
            std::slice::from_raw_parts(packet.lpData as *const u16, packet.cbData as usize / 2)
        };
        let Ok(text) = String::from_utf16(units) else {
            return 0;
        };
        return unsafe { (*pointer).commit(text) as LRESULT };
    }
    if message == WM_APP + 1 {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if !pointer.is_null() && unsafe { !(*pointer).flush_keys() } {
            unsafe {
                SetTimer(hwnd, FLUSH_TIMER, 100, None);
            }
        }
        return 0;
    }
    if message == WM_TIMER && w == READY_TIMER {
        unsafe {
            KillTimer(hwnd, READY_TIMER);
        }
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        // The editor is not ready yet: end it, return the cached letters to the host, and the input
        // method returns to normal.
        if !pointer.is_null()
            && unsafe { (&*pointer).editor_open.get() && (&*pointer).editor_window.get().is_null() }
        {
            unsafe {
                OutputDebugStringW(
                    wide("InputTranslation: 编辑器就绪超时，已归还缓存按键").as_ptr(),
                );
                {
                    let mut editor = (&*pointer).editor.borrow_mut();
                    if let Some(child) = editor.as_mut() {
                        let _ = child.kill();
                    }
                }
                (*pointer).restore_pending_keys();
                (*pointer).reset_editor();
            }
        }
        return 0;
    }
    if message == WM_TIMER && w == FLUSH_TIMER {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if pointer.is_null() || unsafe { (*pointer).flush_keys() } {
            unsafe {
                KillTimer(hwnd, FLUSH_TIMER);
            }
        } else if unsafe { (&*pointer).flush_attempts.get() } >= 10 {
            unsafe {
                KillTimer(hwnd, FLUSH_TIMER);
                {
                    let mut editor = (&*pointer).editor.borrow_mut();
                    if let Some(child) = editor.as_mut() {
                        let _ = child.kill();
                    }
                }
                (*pointer).restore_pending_keys();
                (*pointer).reset_editor();
            }
        } else {
            unsafe {
                let service = &*pointer;
                service.flush_attempts.set(service.flush_attempts.get() + 1);
            }
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || iid.is_null() || out.is_null() {
        return E_FAIL;
    }
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
    register().map_or_else(|error| error.code(), |_| S_OK)
}
#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    unregister().map_or_else(|error| error.code(), |_| S_OK)
}
#[unsafe(no_mangle)]
extern "system" fn DllMain(instance: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(instance.0, Ordering::SeqCst);
    }
    true.into()
}
