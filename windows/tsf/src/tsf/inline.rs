use super::*;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, COLOR_WINDOWTEXT, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK, DrawTextW,
    EndPaint, FillRect, GetSysColor, GetSysColorBrush, InvalidateRect, PAINTSTRUCT, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, MA_NOACTIVATE,
    WM_MOUSEACTIVATE, WM_PAINT,
};

const CANDIDATE_GUID: GUID = GUID::from_u128(0xd39fb88b_09bb_4ca2_a050_174b5ab3b7c0);
const CANDIDATE_CLASS: &str = "YiIMECandidate";
static DICTIONARY: DictionaryLock<yiime_core::MixedInput> = DictionaryLock::new();

fn candidates(code: &str) -> Vec<yiime_core::Candidate> {
    DICTIONARY
        .get_or_init(yiime_core::MixedInput::bundled)
        .candidates(code, 9)
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
}

#[implement(ITfEditSession)]
struct ComposeEdit {
    context: ITfContext,
    action: Action,
    ui: CandidateUi,
    client_id: u32,
}
impl ITfEditSession_Impl for ComposeEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("ComposeEdit", || {
            if let Err(error) = self.apply(cookie) {
                log_hresult("组字更新失败", &error);
                return Err(error);
            }
            Ok(())
        })
    }
}
impl ComposeEdit_Impl {
    fn apply(&self, cookie: u32) -> Result<()> {
        let range = match &self.action {
            Action::Start(code) => {
                log("组字开始");
                let insert: ITfInsertAtSelection = self.context.cast()?;
                let range = unsafe {
                    insert.InsertTextAtSelection(
                        cookie,
                        INSERT_TEXT_AT_SELECTION_FLAGS(0),
                        &code.encode_utf16().collect::<Vec<_>>(),
                    )?
                };
                let context: ITfContextComposition = self.context.cast()?;
                let composition = unsafe {
                    context.StartComposition(cookie, &range, None::<&ITfCompositionSink>)?
                };
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
                let mut state = self.ui.state.borrow_mut();
                let active = state
                    .as_mut()
                    .filter(|active| active.context == self.context)
                    .ok_or(E_FAIL)?;
                let range = unsafe { active.composition.GetRange()? };
                unsafe { range.SetText(cookie, 0, &code.encode_utf16().collect::<Vec<_>>())? };
                if matches!(self.action, Action::Finish(_)) {
                    unsafe { active.composition.EndComposition(cookie)? };
                    state.take();
                } else {
                    active.code = code.clone();
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
        if let Ok(view) = unsafe { self.context.GetActiveView() }
            && let Some(position) = position_of(cookie, &view, &caret)
        {
            place(window, position);
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
        let monitor = MonitorFromPoint(POINT { x, y: bottom }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut y = bottom + 4;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            x = x.clamp(
                info.rcWork.left,
                (info.rcWork.right - 420).max(info.rcWork.left),
            );
            if y + 62 > info.rcWork.bottom {
                y = top - 66;
            }
            y = y.clamp(
                info.rcWork.top,
                (info.rcWork.bottom - 62).max(info.rcWork.top),
            );
        }
        SetWindowPos(
            window,
            HWND_TOPMOST,
            x,
            y,
            420,
            62,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

fn register_candidate_class() -> bool {
    let instance = MODULE.load(Ordering::SeqCst) as _;
    let class = wide(CANDIDATE_CLASS);
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(candidate_proc),
        hInstance: instance,
        hbrBackground: (COLOR_WINDOW + 1) as _,
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
    if message == WM_PAINT {
        let mut paint = unsafe { std::mem::zeroed::<PAINTSTRUCT>() };
        let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
        if !hdc.is_null() {
            let mut rect = windows_sys::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            unsafe { GetClientRect(hwnd, &mut rect) };
            unsafe { FillRect(hdc, &rect, GetSysColorBrush(COLOR_WINDOW)) };
            unsafe { SetBkMode(hdc, TRANSPARENT as i32) };
            unsafe { SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT)) };
            let length = unsafe { GetWindowTextLengthW(hwnd) };
            if length > 0 {
                let mut text = vec![0u16; length as usize + 1];
                let written = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) };
                if written > 0 {
                    let mut text_rect = windows_sys::Win32::Foundation::RECT {
                        left: rect.left + 6,
                        top: rect.top + 4,
                        right: rect.right - 6,
                        bottom: rect.bottom - 4,
                    };
                    unsafe {
                        DrawTextW(
                            hdc,
                            text.as_ptr(),
                            written,
                            &mut text_rect,
                            DT_LEFT | DT_WORDBREAK | DT_NOPREFIX,
                        )
                    };
                }
            }
            unsafe { EndPaint(hwnd, &paint) };
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
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
        let mut text = format!("{code}\r\n");
        for (index, candidate) in candidates(code).iter().enumerate() {
            text.push_str(&format!("{} {}  ", index + 1, candidate.text));
            if index == 4 {
                text.push_str("\r\n");
            }
        }
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
                    wide(&text).as_ptr(),
                    WS_POPUP | WS_BORDER,
                    0,
                    0,
                    420,
                    62,
                    owner,
                    null_mut(),
                    MODULE.load(Ordering::SeqCst) as _,
                    null_mut(),
                )
            };
            self.window.set(window);
        }
        if window.is_null() {
            log("候选窗未显示：CreateWindowEx 失败");
            return;
        }
        unsafe { SetWindowTextW(window, wide(&text).as_ptr()) };
        unsafe { InvalidateRect(window, std::ptr::null(), 1) };
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
        let session: ITfEditSession = ComposeEdit {
            context: context.clone(),
            action,
            ui: self.ui.clone(),
            client_id,
        }
        .into();
        // qingjian apps/windows/tsf/src/com/edit/update.rs: TF_ES_READWRITE only, no
        // TF_ES_SYNC; Ok means the session was accepted, the write happens in the callback.
        request_session(context, client_id, session, name)
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
