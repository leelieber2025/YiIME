use super::*;
use std::path::PathBuf;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DeleteObject, EndPaint, FillRect, GetDC, GetStockObject,
    GetTextExtentPoint32W, HFONT, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT, TextOutW, WHITE_BRUSH,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, GetClientRect, GetCursorPos, GetWindowRect,
    MA_NOACTIVATE, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT,
};

const CANDIDATE_GUID: GUID = GUID::from_u128(0xd39fb88b_09bb_4ca2_a050_174b5ab3b7c0);
const CANDIDATE_CLASS: &str = "YiIMECandidate";
// Same palette and metrics as the draft box (windows_gui/app.rs and candidate.rs).
const BLUE: u32 = 0x00C0_6000;
const GRAY: u32 = 0x0080_8080;
const INK: u32 = 0x0020_2020;
const HIGHLIGHT: u32 = 0x00FB_E9D6;
const SEPARATOR: u32 = 0x00E0_E0E0;
const FONT_PX: i32 = 18;
const SMALL_FONT_PX: i32 = 13;
const PAD: i32 = 6;
const LINE_H: i32 = 24;
const GAP: i32 = 14;
static DICTIONARY: DictionaryLock<yiime_core::MixedInput> = DictionaryLock::new();
static USER_WORDS: Mutex<Option<yiime_core::UserWordsReloader>> = Mutex::new(None);

struct DisplayCandidate {
    text: String,
    hint: String,
    complete: bool,
}

struct CandidateView {
    code: String,
    rows: Vec<DisplayCandidate>,
}

fn user_words_path() -> Option<PathBuf> {
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    let dir = appdata.join("YiIME");
    let legacy = appdata.join("InputTranslation");
    let folder = if !dir.exists() && legacy.is_dir() {
        legacy
    } else {
        dir
    };
    Some(folder.join(yiime_core::USER_WORDS_FILE))
}

fn dictionary() -> &'static yiime_core::MixedInput {
    let input = DICTIONARY.get_or_init(yiime_core::MixedInput::bundled);
    refresh_user_words(input);
    input
}

fn refresh_user_words(input: &yiime_core::MixedInput) {
    let mut guard = USER_WORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        let (reloader, contents) = yiime_core::UserWordsReloader::load(user_words_path());
        if let Some(contents) = contents {
            input.set_user_words(&contents);
        }
        *guard = Some(reloader);
    } else if let Some(reloader) = guard.as_mut()
        && let Some(contents) = reloader.check()
    {
        input.set_user_words(&contents);
    }
}

fn hint_for(input: &yiime_core::MixedInput, candidate: &yiime_core::Candidate) -> String {
    // Same annotations as the draft box (engine.rs Composer::set_code): Wubi matches show
    // no hint; others get the remaining/full Wubi code, wildcard a short form, corrections
    // a 纠错 label.
    if candidate.source == yiime_core::Source::Wubi {
        return String::new();
    }
    let Some(hint) = input.wubi_hint(&candidate.text) else {
        return if candidate.source == yiime_core::Source::Corrected {
            "(纠错)".to_owned()
        } else {
            String::new()
        };
    };
    let hint = if candidate.source == yiime_core::Source::Wildcard {
        hint.rsplit('/').next().unwrap_or(&hint).to_owned()
    } else {
        hint
    };
    if candidate.source == yiime_core::Source::Corrected {
        format!("(纠错 {hint})")
    } else {
        format!("({hint})")
    }
}

fn candidates(code: &str) -> Vec<DisplayCandidate> {
    let input = dictionary();
    let size = page_size();
    let fuzzy = option(Flag::FuzzyPinyin);
    let traditional = option(Flag::Traditional);
    let common_only = option(Flag::CommonOnly);
    let scheme = shuangpin_scheme();
    let pinyin_first = option(Flag::PinyinFirst) || scheme.is_some();
    let pending = match scheme {
        Some(_) if code.replace('\'', "").len() % 2 == 1 => code.chars().last(),
        _ => None,
    };
    let lookup = match scheme {
        _ if code.is_empty() => None,
        Some(scheme) => {
            let keys = &code[..code.len() - pending.map_or(0, char::len_utf8)];
            if keys.replace('\'', "").is_empty() {
                None
            } else {
                scheme.expand(keys)
            }
        }
        None => Some(code.to_owned()),
    };
    let mut items = match &lookup {
        None => Vec::new(),
        Some(lookup) => input.candidates_filtered(
            lookup,
            size,
            size,
            fuzzy,
            pinyin_first,
            common_only,
            common_only && !traditional,
        ),
    };
    if let Some(scheme) = scheme {
        items.retain_mut(|candidate| {
            if candidate.rest.is_empty() {
                return true;
            }
            match scheme.rest_keys(code, &candidate.rest) {
                Some(keys) => {
                    candidate.rest = keys;
                    true
                }
                None => false,
            }
        });
        if let Some(key) = pending {
            for candidate in &mut items {
                candidate.rest.push(key);
                candidate.source = yiime_core::Source::Partial;
            }
        }
    }
    items
        .into_iter()
        .map(|mut candidate| {
            let hint = hint_for(input, &candidate);
            if traditional {
                candidate.text = yiime_core::to_traditional(&candidate.text);
            }
            DisplayCandidate {
                text: candidate.text,
                hint,
                complete: candidate.complete,
            }
        })
        .collect()
}

fn window_dpi(hwnd: HWND) -> u32 {
    let dpi = if !hwnd.is_null() {
        unsafe { GetDpiForWindow(hwnd) }
    } else {
        0
    };
    let dpi = if dpi == 0 {
        unsafe { GetDpiForSystem() }
    } else {
        dpi
    };
    dpi.max(96)
}

fn scale_px(px: i32, dpi: u32) -> i32 {
    (i64::from(px) * i64::from(dpi) / 96).max(1) as i32
}

unsafe fn create_ui_font(height: i32) -> HFONT {
    // windows_gui/candidate.rs create_font: Microsoft YaHei UI, ClearType, default charset.
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

unsafe fn text_width(hdc: windows_sys::Win32::Graphics::Gdi::HDC, text: &str) -> i32 {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe {
        GetTextExtentPoint32W(hdc, units.as_ptr(), units.len() as i32, &mut size);
    }
    size.cx
}

unsafe fn draw_text(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    x: i32,
    y: i32,
    color: u32,
    text: &str,
) {
    let units: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        SetTextColor(hdc, color);
        TextOutW(hdc, x, y, units.as_ptr(), units.len() as i32);
    }
}

/// Client size for the current candidates, matching the draft box row layout.
fn measure_view(hwnd: HWND, view: &CandidateView) -> (i32, i32) {
    let dpi = window_dpi(hwnd);
    let pad = scale_px(PAD, dpi);
    let line_h = scale_px(LINE_H, dpi);
    let gap = scale_px(GAP, dpi);
    let hdc = unsafe { GetDC(hwnd) };
    if hdc.is_null() {
        let rows = 1 + i32::from(!view.rows.is_empty());
        return (scale_px(200, dpi), pad + rows * line_h + pad);
    }
    let font = unsafe { create_ui_font(scale_px(FONT_PX, dpi)) };
    let small = unsafe { create_ui_font(scale_px(SMALL_FONT_PX, dpi)) };
    let old = unsafe { SelectObject(hdc, font as _) };
    let mut width = unsafe { text_width(hdc, &view.code) } + 2 * pad;
    let mut x = pad;
    for (index, row) in view.rows.iter().enumerate() {
        let label = format!("{}.{}", index + 1, row.text);
        let mut w = unsafe { text_width(hdc, &label) };
        if !row.hint.is_empty() {
            unsafe { SelectObject(hdc, small as _) };
            w += unsafe { text_width(hdc, &row.hint) } + 1;
            unsafe { SelectObject(hdc, font as _) };
        }
        x += w + gap;
    }
    if !view.rows.is_empty() {
        width = width.max(x + pad);
    }
    unsafe {
        SelectObject(hdc, old);
        if !font.is_null() {
            DeleteObject(font as _);
        }
        if !small.is_null() {
            DeleteObject(small as _);
        }
        ReleaseDC(hwnd, hdc);
    }
    let rows = 1 + i32::from(!view.rows.is_empty());
    // +2 for the 1px separator under the code and the window border inset.
    (
        width.max(scale_px(120, dpi)) + 2,
        pad + rows * line_h + pad + 2,
    )
}

fn log_hresult(action: &str, error: &windows::core::Error) {
    log(&format!("{action}：{:#010x}", error.code().0 as u32));
}

fn request_session(
    context: &ITfContext,
    client_id: u32,
    session: ITfEditSession,
    action: &str,
) -> bool {
    match unsafe { context.RequestEditSession(client_id, &session, TF_ES_READWRITE) } {
        Ok(status) => match status.ok() {
            Ok(()) => {
                log(&format!("{action}：会话已受理"));
                true
            }
            Err(error) => {
                log_hresult(&format!("{action}：会话返回"), &error);
                false
            }
        },
        Err(error) => {
            log_hresult(&format!("{action}：会话未被受理"), &error);
            false
        }
    }
}

/// Microsoft SampleIME `CSampleIME` implements `ITfCompositionSink` and passes `this` to
/// `StartComposition` (`StartComposition.cpp` `CStartCompositionEditSession::DoEditSession`;
/// `Composition.cpp` `OnCompositionTerminated`). Weasel and qingjian do the same.
#[implement(ITfCompositionSink)]
struct CompositionSink {
    ui: CandidateUi,
}
impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        composition: Ref<'_, ITfComposition>,
    ) -> Result<()> {
        // weasel WeaselTSF::OnCompositionTerminated: ignore our own EndComposition.
        let ours = self.ui.state.borrow().as_ref().is_some_and(|active| {
            composition
                .as_ref()
                .is_some_and(|ended| ended == &active.composition)
        });
        if !ours {
            return Ok(());
        }
        log("组句被应用终止，清本地状态");
        self.ui.state.borrow_mut().take();
        self.ui.end_ui();
        self.ui.destroy_window();
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionOutcome {
    Pending,
    Succeeded,
    Failed,
}

struct Composition {
    context: ITfContext,
    composition: ITfComposition,
    code: String,
}

#[implement(ITfCandidateListUIElement)]
struct CandidateElement {
    state: Rc<RefCell<Option<Composition>>>,
    shown: Rc<Cell<bool>>,
}
impl ITfUIElement_Impl for CandidateElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("YiIME 候选"))
    }
    fn GetGUID(&self) -> Result<GUID> {
        Ok(CANDIDATE_GUID)
    }
    fn Show(&self, show: BOOL) -> Result<()> {
        self.shown.set(show.as_bool());
        Ok(())
    }
    fn IsShown(&self) -> Result<BOOL> {
        Ok(self.shown.get().into())
    }
}
impl ITfCandidateListUIElement_Impl for CandidateElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        Ok(TF_CLUIE_DOCUMENTMGR
            | TF_CLUIE_COUNT
            | TF_CLUIE_CURRENTPAGE
            | TF_CLUIE_SELECTION
            | TF_CLUIE_STRING
            | TF_CLUIE_PAGEINDEX)
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        let state = self.state.borrow();
        unsafe { state.as_ref().ok_or(E_FAIL)?.context.GetDocumentMgr() }
    }
    fn GetCount(&self) -> Result<u32> {
        Ok(self
            .state
            .borrow()
            .as_ref()
            .map_or(0, |state| candidates(&state.code).len() as u32))
    }
    fn GetSelection(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetString(&self, index: u32) -> Result<BSTR> {
        let state = self.state.borrow();
        candidates(&state.as_ref().ok_or(E_FAIL)?.code)
            .get(index as usize)
            .map(|candidate| BSTR::from(candidate.text.as_str()))
            .ok_or(E_FAIL.into())
    }
    fn GetPageIndex(&self, index: *mut u32, size: u32, count: *mut u32) -> Result<()> {
        if count.is_null() || (size > 0 && index.is_null()) {
            return Err(E_FAIL.into());
        }
        unsafe {
            *count = u32::from(self.GetCount()? > 0);
            if size > 0 {
                *index = 0;
            }
        }
        Ok(())
    }
    fn SetPageIndex(&self, _index: *const u32, _count: u32) -> Result<()> {
        Ok(())
    }
    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(0)
    }
}

enum Action {
    Start(String),
    Update(String),
    Finish(String),
}

/// Shared pieces the async edit session needs after OnKeyDown returns (qingjian
/// apps/windows/tsf/src/com/edit/update.rs: the callback may run later).
#[derive(Clone)]
struct CandidateUi {
    state: Rc<RefCell<Option<Composition>>>,
    window: Rc<Cell<HWND>>,
    shown: Rc<Cell<bool>>,
    manager: Rc<RefCell<Option<ITfUIElementMgr>>>,
    element: Rc<Cell<Option<u32>>>,
    layout: Rc<RefCell<Option<(ITfContext, u32)>>>,
    view: Rc<RefCell<CandidateView>>,
    last_caret: Rc<Cell<Option<(i32, i32, i32)>>>,
}

#[implement(ITfEditSession)]
struct ComposeEdit {
    context: ITfContext,
    action: Action,
    ui: CandidateUi,
    client_id: u32,
    outcome: Rc<Cell<SessionOutcome>>,
}
impl ITfEditSession_Impl for ComposeEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("ComposeEdit", || {
            if let Err(error) = self.apply(cookie) {
                self.outcome.set(SessionOutcome::Failed);
                log_hresult("组字更新失败", &error);
                return Err(error);
            }
            self.outcome.set(SessionOutcome::Succeeded);
            Ok(())
        })
    }
}
impl ComposeEdit_Impl {
    fn apply(&self, cookie: u32) -> Result<()> {
        let range = match &self.action {
            Action::Start(code) => {
                // SampleIME StartComposition.cpp CStartCompositionEditSession::DoEditSession:
                // InsertTextAtSelection(TF_IAS_QUERYONLY, empty) then StartComposition with a
                // real ITfCompositionSink. Preedit is written on the composition range (qingjian
                // update_preedit / SampleIME _AddComposingAndChar).
                log("组字开始");
                let insert: ITfInsertAtSelection = self.context.cast().inspect_err(|error| {
                    log_hresult("ITfInsertAtSelection 接口", error);
                })?;
                let _range = unsafe { insert.InsertTextAtSelection(cookie, TF_IAS_QUERYONLY, &[]) }
                    .inspect_err(|error| {
                        log_hresult("InsertTextAtSelection(QUERYONLY)", error);
                    })?;
                let context: ITfContextComposition = self.context.cast().inspect_err(|error| {
                    log_hresult("ITfContextComposition 接口", error);
                })?;
                let sink: ITfCompositionSink = CompositionSink {
                    ui: self.ui.clone(),
                }
                .into();
                let composition = unsafe { context.StartComposition(cookie, &_range, &sink) }
                    .inspect_err(|error| {
                        log_hresult("StartComposition", error);
                    })?;
                let range = unsafe { composition.GetRange() }.inspect_err(|error| {
                    log_hresult("GetRange", error);
                })?;
                unsafe { range.SetText(cookie, 0, &code.encode_utf16().collect::<Vec<_>>()) }
                    .inspect_err(|error| {
                        log_hresult("SetText", error);
                    })?;
                *self.ui.state.borrow_mut() = Some(Composition {
                    context: self.context.clone(),
                    composition,
                    code: code.clone(),
                });
                range
            }
            Action::Update(code) | Action::Finish(code) => {
                log(if matches!(self.action, Action::Finish(_)) {
                    "组字上屏"
                } else {
                    "组字更新"
                });
                // Drop the state borrow before EndComposition: OnCompositionTerminated may run
                // synchronously (weasel notes this) and also borrows state.
                let finishing = matches!(self.action, Action::Finish(_));
                let (range, ended) = {
                    let mut state = self.ui.state.borrow_mut();
                    let active = state
                        .as_mut()
                        .filter(|active| active.context == self.context)
                        .ok_or(E_FAIL)?;
                    let range = unsafe { active.composition.GetRange()? };
                    unsafe { range.SetText(cookie, 0, &code.encode_utf16().collect::<Vec<_>>())? };
                    if finishing {
                        let composition = state.take().map(|active| active.composition);
                        (range, composition)
                    } else {
                        active.code = code.clone();
                        (range, None)
                    }
                };
                if let Some(composition) = ended {
                    unsafe { composition.EndComposition(cookie)? };
                }
                range
            }
        };
        let caret = unsafe { range.Clone()? };
        unsafe { caret.Collapse(cookie, TF_ANCHOR_END)? };
        let selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(caret.clone())),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_END,
                fInterimChar: false.into(),
            },
        };
        let _ = unsafe {
            self.context
                .SetSelection(cookie, std::slice::from_ref(&selection))
        };
        drop(ManuallyDrop::into_inner(selection.range));
        let position = if let Ok(view) = unsafe { self.context.GetActiveView() } {
            position_of(cookie, &view, &caret)
        } else {
            None
        };
        let code = self
            .ui
            .state
            .borrow()
            .as_ref()
            .map(|state| state.code.clone());
        if let Some(code) = code {
            self.ui.advise_layout(&self.context, self.client_id);
            self.ui.refresh_element();
            if self.ui.shown.get() {
                self.ui.show(&self.context, position, &code);
            } else {
                log("候选窗未显示：宿主要求无界面");
                self.ui.hide();
            }
        } else {
            self.ui.end_ui();
        }
        Ok(())
    }
}

fn position_of(
    cookie: u32,
    view: &ITfContextView,
    range: &windows::Win32::UI::TextServices::ITfRange,
) -> Option<(i32, i32, i32)> {
    let mut rect = RECT::default();
    let mut clipped = BOOL(0);
    unsafe { view.GetTextExt(cookie, range, &mut rect, &mut clipped) }.ok()?;
    (rect.left != 0 || rect.top != 0 || rect.right != 0 || rect.bottom != 0).then_some((
        rect.left,
        rect.top,
        rect.bottom,
    ))
}

#[implement(ITfEditSession)]
struct PositionEdit {
    context: ITfContext,
    ui: CandidateUi,
}
impl ITfEditSession_Impl for PositionEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let state = self.ui.state.borrow();
        let Some(active) = state
            .as_ref()
            .filter(|active| active.context == self.context)
        else {
            return Ok(());
        };
        let window = self.ui.window.get();
        if window.is_null() || !self.ui.shown.get() {
            return Ok(());
        }
        let range = unsafe { active.composition.GetRange()? };
        let caret = unsafe { range.Clone()? };
        unsafe { caret.Collapse(cookie, TF_ANCHOR_END)? };
        if let Ok(view) = unsafe { self.context.GetActiveView() } {
            if let Some(position) = position_of(cookie, &view, &caret) {
                self.ui.last_caret.set(Some(position));
                place(window, position);
            } else if let Some(position) = self.ui.last_caret.get() {
                place(window, position);
            }
        }
        Ok(())
    }
}

#[implement(ITfTextLayoutSink)]
struct LayoutSink {
    ui: CandidateUi,
    client_id: u32,
}
impl ITfTextLayoutSink_Impl for LayoutSink_Impl {
    fn OnLayoutChange(
        &self,
        context: Ref<ITfContext>,
        _code: windows::Win32::UI::TextServices::TfLayoutCode,
        _view: Ref<ITfContextView>,
    ) -> Result<()> {
        let Some(context) = context.as_ref() else {
            return Ok(());
        };
        if !self
            .ui
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.context == *context)
        {
            return Ok(());
        }
        let session: ITfEditSession = PositionEdit {
            context: context.clone(),
            ui: self.ui.clone(),
        }
        .into();
        let _ = unsafe {
            context.RequestEditSession(self.client_id, &session, TF_ES_ASYNC | TF_ES_READ)
        };
        Ok(())
    }
}

fn place(window: HWND, (mut x, top, bottom): (i32, i32, i32)) {
    unsafe {
        let mut rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetWindowRect(window, &mut rect);
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        let monitor = MonitorFromPoint(POINT { x, y: bottom }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        // Draft box candidate.rs place: below the caret, or above if it does not fit.
        let mut y = bottom + 4;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            x = x.clamp(
                info.rcWork.left,
                (info.rcWork.right - width).max(info.rcWork.left),
            );
            if y + height > info.rcWork.bottom {
                y = top - height - 4;
            }
            y = y.clamp(
                info.rcWork.top,
                (info.rcWork.bottom - height).max(info.rcWork.top),
            );
        }
        SetWindowPos(
            window,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn mouse_caret() -> Option<(i32, i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } != 0 {
        Some((point.x, point.y, point.y + 20))
    } else {
        None
    }
}

fn register_candidate_class() -> bool {
    let instance = MODULE.load(Ordering::SeqCst) as _;
    let class = wide(CANDIDATE_CLASS);
    let wc = WNDCLASSW {
        // CS_DROPSHADOW matches the draft box (windows_gui/app.rs).
        style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
        lpfnWndProc: Some(candidate_proc),
        hInstance: instance,
        hbrBackground: unsafe { GetStockObject(WHITE_BRUSH) } as _,
        lpszClassName: class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    let registered = unsafe { RegisterClassW(&wc) };
    registered != 0 || unsafe { GetLastError() } == ERROR_CLASS_ALREADY_EXISTS
}

unsafe extern "system" fn candidate_proc(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(|| unsafe { candidate_proc_impl(hwnd, message, w, l) }) {
        Ok(result) => result,
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log("panic in candidate_proc"));
            0
        }
    }
}

unsafe fn candidate_proc_impl(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // qingjian apps/windows/server/src/ui/candidates/click.rs: mouse_activate returns
    // MA_NOACTIVATE so a click does not take the host's focus.
    if message == WM_MOUSEACTIVATE {
        return MA_NOACTIVATE as LRESULT;
    }
    if message == WM_NCDESTROY {
        let data =
            unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Rc<RefCell<CandidateView>>;
        if !data.is_null() {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            drop(unsafe { Box::from_raw(data) });
        }
        return 0;
    }
    if message == WM_PAINT {
        paint_candidates(hwnd);
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

fn paint_candidates(hwnd: HWND) {
    let mut paint = unsafe { std::mem::zeroed::<PAINTSTRUCT>() };
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    if hdc.is_null() {
        return;
    }
    let data =
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Rc<RefCell<CandidateView>>;
    let view = if data.is_null() {
        None
    } else {
        Some(unsafe { (*data).borrow() })
    };
    let mut client = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetClientRect(hwnd, &mut client) };
    unsafe { FillRect(hdc, &client, GetStockObject(WHITE_BRUSH) as _) };
    unsafe { SetBkMode(hdc, TRANSPARENT as i32) };
    let Some(view) = view.as_ref() else {
        unsafe { EndPaint(hwnd, &paint) };
        return;
    };
    let dpi = window_dpi(hwnd);
    let pad = scale_px(PAD, dpi);
    let line_h = scale_px(LINE_H, dpi);
    let gap = scale_px(GAP, dpi);
    let font = unsafe { create_ui_font(scale_px(FONT_PX, dpi)) };
    let small = unsafe { create_ui_font(scale_px(SMALL_FONT_PX, dpi)) };
    let old = unsafe { SelectObject(hdc, font as _) };
    let separator = unsafe { CreateSolidBrush(SEPARATOR) };
    let line = windows_sys::Win32::Foundation::RECT {
        left: pad,
        top: pad + line_h - 2,
        right: client.right - pad,
        bottom: pad + line_h - 1,
    };
    unsafe {
        FillRect(hdc, &line, separator);
        DeleteObject(separator as _);
        draw_text(hdc, pad, pad + 2, BLUE, &view.code);
    }
    if !view.rows.is_empty() {
        let row_top = pad + line_h;
        let mut x = pad;
        for (index, row) in view.rows.iter().enumerate() {
            let label = format!("{}.{}", index + 1, row.text);
            let mut w = unsafe { text_width(hdc, &label) };
            if !row.hint.is_empty() {
                unsafe { SelectObject(hdc, small as _) };
                w += unsafe { text_width(hdc, &row.hint) } + 1;
                unsafe { SelectObject(hdc, font as _) };
            }
            if index == 0 {
                let highlight = unsafe { CreateSolidBrush(HIGHLIGHT) };
                let rect = windows_sys::Win32::Foundation::RECT {
                    left: x - 3,
                    top: row_top,
                    right: x + w + 3,
                    bottom: row_top + line_h - 2,
                };
                unsafe {
                    FillRect(hdc, &rect, highlight);
                    DeleteObject(highlight as _);
                }
            }
            let color = if row.complete { INK } else { GRAY };
            unsafe { draw_text(hdc, x, row_top + 2, color, &label) };
            if !row.hint.is_empty() {
                let hx = x + unsafe { text_width(hdc, &label) } + 1;
                unsafe {
                    let previous = SelectObject(hdc, small as _);
                    draw_text(hdc, hx, row_top + 6, GRAY, &row.hint);
                    SelectObject(hdc, previous);
                }
            }
            x += w + gap;
        }
    }
    unsafe {
        SelectObject(hdc, old);
        if !font.is_null() {
            DeleteObject(font as _);
        }
        if !small.is_null() {
            DeleteObject(small as _);
        }
        EndPaint(hwnd, &paint);
    }
}

#[implement(ITfEditSession)]
struct CancelEdit {
    composition: ITfComposition,
}
impl ITfEditSession_Impl for CancelEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("CancelEdit", || {
            let range = unsafe { self.composition.GetRange()? };
            unsafe {
                range.SetText(cookie, 0, &[])?;
                self.composition.EndComposition(cookie)
            }
        })
    }
}

impl CandidateUi {
    fn refresh_element(&self) {
        let manager = self.manager.borrow();
        let Some(manager) = manager.as_ref() else {
            return;
        };
        if let Some(id) = self.element.get() {
            let _ = unsafe { manager.UpdateUIElement(id) };
            return;
        }
        let element: ITfCandidateListUIElement = CandidateElement {
            state: self.state.clone(),
            shown: self.shown.clone(),
        }
        .into();
        let mut show = BOOL(1);
        let mut id = 0;
        match unsafe { manager.BeginUIElement(&element, &mut show, &mut id) } {
            Ok(()) => {
                self.element.set(Some(id));
                self.shown.set(show.as_bool());
                if !show.as_bool() {
                    log("候选窗未显示：BeginUIElement 要求隐藏");
                }
            }
            Err(error) => log_hresult("候选窗 BeginUIElement 失败", &error),
        }
    }
    fn show(&self, context: &ITfContext, position: Option<(i32, i32, i32)>, code: &str) {
        let rows = candidates(code);
        *self.view.borrow_mut() = CandidateView {
            code: code.to_owned(),
            rows,
        };
        let mut window = self.window.get();
        if window.is_null() || unsafe { IsWindow(window) } == 0 {
            if !register_candidate_class() {
                log("候选窗未显示：窗口类注册失败");
                return;
            }
            let owner = unsafe { context.GetActiveView() }
                .ok()
                .and_then(|view| unsafe { view.GetWnd() }.ok())
                .filter(|hwnd| !hwnd.0.is_null())
                .map_or_else(|| unsafe { GetFocus() }, |hwnd| hwnd.0);
            window = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    wide(CANDIDATE_CLASS).as_ptr(),
                    wide("").as_ptr(),
                    WS_POPUP | WS_BORDER,
                    0,
                    0,
                    0,
                    0,
                    owner,
                    null_mut(),
                    MODULE.load(Ordering::SeqCst) as _,
                    null_mut(),
                )
            };
            self.window.set(window);
            if !window.is_null() {
                let leaked = Box::into_raw(Box::new(self.view.clone()));
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, leaked as _) };
            }
        }
        if window.is_null() {
            log("候选窗未显示：CreateWindowEx 失败");
            return;
        }
        let (width, height) = measure_view(window, &self.view.borrow());
        unsafe {
            SetWindowPos(
                window,
                HWND_TOPMOST,
                0,
                0,
                width,
                height,
                SWP_NOACTIVATE | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE,
            );
            InvalidateRect(window, std::ptr::null(), 1);
        }
        let position = if let Some(position) = position {
            self.last_caret.set(Some(position));
            Some(position)
        } else if let Some(position) = self.last_caret.get() {
            log("候选窗：光标矩形为空，使用上次位置");
            Some(position)
        } else if let Some(position) = mouse_caret() {
            log("候选窗：光标矩形为空，使用鼠标位置");
            Some(position)
        } else {
            None
        };
        if let Some(position) = position {
            place(window, position);
        } else {
            log("候选窗未显示：没有光标矩形");
            self.hide();
        }
    }
    fn hide(&self) {
        let window = self.window.get();
        if !window.is_null() {
            unsafe { ShowWindow(window, SW_HIDE) };
        }
    }
    fn advise_layout(&self, context: &ITfContext, client_id: u32) {
        if self.layout.borrow().is_some() {
            return;
        }
        if let Ok(source) = context.cast::<ITfSource>() {
            let sink: ITfTextLayoutSink = LayoutSink {
                ui: self.clone(),
                client_id,
            }
            .into();
            if let Ok(cookie) = unsafe { source.AdviseSink(&ITfTextLayoutSink::IID, &sink) } {
                *self.layout.borrow_mut() = Some((context.clone(), cookie));
            }
        }
    }
    fn end_ui(&self) {
        if let Some((context, cookie)) = self.layout.borrow_mut().take()
            && let Ok(source) = context.cast::<ITfSource>()
        {
            let _ = unsafe { source.UnadviseSink(cookie) };
        }
        if let Some(id) = self.element.take()
            && let Some(manager) = self.manager.borrow().as_ref()
        {
            let _ = unsafe { manager.EndUIElement(id) };
        }
        self.hide();
    }
    fn destroy_window(&self) {
        let window = self.window.replace(null_mut());
        if !window.is_null() {
            unsafe { DestroyWindow(window) };
        }
    }
}

pub(super) struct InlineInput {
    ui: CandidateUi,
}
impl InlineInput {
    pub(super) fn new() -> Self {
        Self {
            ui: CandidateUi {
                state: Rc::new(RefCell::new(None)),
                window: Rc::new(Cell::new(null_mut())),
                shown: Rc::new(Cell::new(true)),
                manager: Rc::new(RefCell::new(None)),
                element: Rc::new(Cell::new(None)),
                layout: Rc::new(RefCell::new(None)),
                view: Rc::new(RefCell::new(CandidateView {
                    code: String::new(),
                    rows: Vec::new(),
                })),
                last_caret: Rc::new(Cell::new(None)),
            },
        }
    }
    pub(super) fn activate(&self, manager: &ITfThreadMgr) {
        *self.ui.manager.borrow_mut() = manager.cast().ok();
    }
    pub(super) fn active(&self) -> bool {
        self.ui.state.borrow().is_some()
    }
    pub(super) fn same_context(&self, context: &ITfContext) -> bool {
        self.ui
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.context == *context)
    }
    pub(super) fn wants(&self, context: &ITfContext, key: usize) -> bool {
        if let Some(state) = self.ui.state.borrow().as_ref() {
            state.context == *context
                && matches!(key, 0x41..=0x5a | 0x08 | 0x1b | 0x0d | 0x20 | 0x31..=0x39)
        } else {
            (0x41..=0x5a).contains(&key)
        }
    }
    pub(super) fn key(&self, context: &ITfContext, client_id: u32, key: usize) -> bool {
        let code = self
            .ui
            .state
            .borrow()
            .as_ref()
            .map(|state| state.code.clone());
        let Some(mut code) = code else {
            return self.edit(
                context,
                client_id,
                Action::Start((key as u8 as char).to_ascii_lowercase().to_string()),
            );
        };
        match key {
            0x41..=0x5a => {
                code.push((key as u8 as char).to_ascii_lowercase());
                self.edit(context, client_id, Action::Update(code))
            }
            0x08 => {
                code.pop();
                if code.is_empty() {
                    self.edit(context, client_id, Action::Finish(String::new()))
                } else {
                    self.edit(context, client_id, Action::Update(code))
                }
            }
            0x1b => self.edit(context, client_id, Action::Finish(String::new())),
            0x0d | 0x20 => {
                let word = candidates(&code)
                    .first()
                    .map(|item| item.text.clone())
                    .unwrap_or(code);
                self.edit(context, client_id, Action::Finish(word))
            }
            0x31..=0x39 => candidates(&code).get(key - 0x31).is_some_and(|item| {
                self.edit(context, client_id, Action::Finish(item.text.clone()))
            }),
            _ => false,
        }
    }
    fn edit(&self, context: &ITfContext, client_id: u32, action: Action) -> bool {
        let name = match &action {
            Action::Start(_) => "组字开始",
            Action::Update(_) => "组字更新",
            Action::Finish(_) => "组字结束",
        };
        let outcome = Rc::new(Cell::new(SessionOutcome::Pending));
        let session: ITfEditSession = ComposeEdit {
            context: context.clone(),
            action,
            ui: self.ui.clone(),
            client_id,
            outcome: outcome.clone(),
        }
        .into();
        // SampleIME KeyHandler.cpp _InvokeKeyHandler: TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
        // no TF_ES_SYNC (Word). qingjian edit/update.rs: TF_ES_READWRITE only; Ok means accepted,
        // the write happens in the callback. A session that already ran and failed is not handled.
        let accepted = request_session(context, client_id, session, name);
        if accepted && outcome.get() == SessionOutcome::Failed {
            log("组字会话已跑完但失败，交回按键");
            false
        } else {
            accepted
        }
    }
    pub(super) fn cancel(&self, client_id: u32) {
        let Some(state) = self.ui.state.borrow_mut().take() else {
            self.ui.end_ui();
            self.ui.destroy_window();
            return;
        };
        log("组字取消");
        let session: ITfEditSession = CancelEdit {
            composition: state.composition,
        }
        .into();
        let _ = request_session(&state.context, client_id, session, "组字取消");
        self.ui.end_ui();
        self.ui.destroy_window();
    }
    pub(super) fn deactivate(&self, client_id: u32) {
        self.cancel(client_id);
        self.ui.manager.borrow_mut().take();
    }
}
