use super::*;

const CANDIDATE_GUID: GUID = GUID::from_u128(0xd39fb88b_09bb_4ca2_a050_174b5ab3b7c0);
static DICTIONARY: DictionaryLock<yiime_core::MixedInput> = DictionaryLock::new();

fn candidates(code: &str) -> Vec<yiime_core::Candidate> {
    DICTIONARY
        .get_or_init(yiime_core::MixedInput::bundled)
        .candidates(code, 9)
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
#[implement(ITfEditSession)]
struct ComposeEdit {
    context: ITfContext,
    state: Rc<RefCell<Option<Composition>>>,
    action: Action,
    position: Rc<Cell<Option<(i32, i32, i32)>>>,
    success: Rc<Cell<bool>>,
}
impl ITfEditSession_Impl for ComposeEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        com_guard("ComposeEdit", || {
            let range = match &self.action {
                Action::Start(code) => {
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
                    *self.state.borrow_mut() = Some(Composition {
                        context: self.context.clone(),
                        composition,
                        code: code.clone(),
                    });
                    range
                }
                Action::Update(code) | Action::Finish(code) => {
                    let mut state = self.state.borrow_mut();
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
            if let Ok(view) = unsafe { self.context.GetActiveView() } {
                self.position.set(position_of(cookie, &view, &caret));
            }
            self.success.set(true);
            Ok(())
        })
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
    state: Rc<RefCell<Option<Composition>>>,
    window: Rc<Cell<HWND>>,
    shown: Rc<Cell<bool>>,
}
impl ITfEditSession_Impl for PositionEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let state = self.state.borrow();
        let Some(active) = state
            .as_ref()
            .filter(|active| active.context == self.context)
        else {
            return Ok(());
        };
        let window = self.window.get();
        if window.is_null() || !self.shown.get() {
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
    state: Rc<RefCell<Option<Composition>>>,
    window: Rc<Cell<HWND>>,
    shown: Rc<Cell<bool>>,
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
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.context == *context)
        {
            return Ok(());
        }
        let session: ITfEditSession = PositionEdit {
            context: context.clone(),
            state: self.state.clone(),
            window: self.window.clone(),
            shown: self.shown.clone(),
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

#[implement(ITfEditSession)]
struct CancelEdit {
    composition: ITfComposition,
}
impl ITfEditSession_Impl for CancelEdit_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let range = unsafe { self.composition.GetRange()? };
        unsafe {
            range.SetText(cookie, 0, &[])?;
            self.composition.EndComposition(cookie)
        }
    }
}

pub(super) struct InlineInput {
    state: Rc<RefCell<Option<Composition>>>,
    window: Rc<Cell<HWND>>,
    shown: Rc<Cell<bool>>,
    manager: RefCell<Option<ITfUIElementMgr>>,
    element: Cell<Option<u32>>,
    layout: RefCell<Option<(ITfContext, u32)>>,
}
impl InlineInput {
    pub(super) fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(None)),
            window: Rc::new(Cell::new(null_mut())),
            shown: Rc::new(Cell::new(true)),
            manager: RefCell::new(None),
            element: Cell::new(None),
            layout: RefCell::new(None),
        }
    }
    pub(super) fn activate(&self, manager: &ITfThreadMgr) {
        *self.manager.borrow_mut() = manager.cast().ok();
    }
    pub(super) fn active(&self) -> bool {
        self.state.borrow().is_some()
    }
    pub(super) fn wants(&self, context: &ITfContext, key: usize) -> bool {
        if let Some(state) = self.state.borrow().as_ref() {
            state.context == *context
                && matches!(key, 0x41..=0x5a | 0x08 | 0x1b | 0x0d | 0x20 | 0x31..=0x39)
        } else {
            (0x41..=0x5a).contains(&key)
        }
    }
    pub(super) fn key(&self, context: &ITfContext, client_id: u32, key: usize) -> bool {
        let code = self.state.borrow().as_ref().map(|state| state.code.clone());
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
        let position = Rc::new(Cell::new(None));
        let success = Rc::new(Cell::new(false));
        let session: ITfEditSession = ComposeEdit {
            context: context.clone(),
            state: self.state.clone(),
            action,
            position: position.clone(),
            success: success.clone(),
        }
        .into();
        let accepted = unsafe {
            context.RequestEditSession(client_id, &session, TF_ES_SYNC | TF_ES_READWRITE)
        }
        .and_then(|status| status.ok())
        .is_ok();
        if !accepted || !success.get() {
            return false;
        }
        let code = self.state.borrow().as_ref().map(|state| state.code.clone());
        if let Some(code) = code {
            self.advise_layout(context, client_id);
            if let Some(manager) = self.manager.borrow().as_ref() {
                if let Some(id) = self.element.get() {
                    let _ = unsafe { manager.UpdateUIElement(id) };
                } else {
                    let element: ITfCandidateListUIElement = CandidateElement {
                        state: self.state.clone(),
                        shown: self.shown.clone(),
                    }
                    .into();
                    let mut show = BOOL(1);
                    let mut id = 0;
                    if unsafe { manager.BeginUIElement(&element, &mut show, &mut id) }.is_ok() {
                        self.element.set(Some(id));
                        self.shown.set(show.as_bool());
                    }
                }
            }
            if self.shown.get() {
                self.show(context, position.get(), &code);
            } else {
                self.hide();
            }
        } else {
            self.end_ui();
        }
        true
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
            let owner = unsafe { context.GetActiveView() }
                .ok()
                .and_then(|view| unsafe { view.GetWnd() }.ok())
                .filter(|hwnd| !hwnd.0.is_null())
                .map_or_else(|| unsafe { GetFocus() }, |hwnd| hwnd.0);
            window = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    wide("STATIC").as_ptr(),
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
            return;
        }
        unsafe { SetWindowTextW(window, wide(&text).as_ptr()) };
        if let Some(position) = position {
            place(window, position);
        } else {
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
                state: self.state.clone(),
                window: self.window.clone(),
                shown: self.shown.clone(),
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
    pub(super) fn cancel(&self, client_id: u32) {
        let context = self
            .state
            .borrow()
            .as_ref()
            .map(|state| state.context.clone());
        if let Some(context) = context
            && !self.edit(&context, client_id, Action::Finish(String::new()))
            && let Some(state) = self.state.borrow_mut().take()
        {
            let session: ITfEditSession = CancelEdit {
                composition: state.composition,
            }
            .into();
            let _ = unsafe {
                state
                    .context
                    .RequestEditSession(client_id, &session, TF_ES_ASYNC | TF_ES_READWRITE)
            };
        }
        self.end_ui();
        let window = self.window.replace(null_mut());
        if !window.is_null() {
            unsafe { DestroyWindow(window) };
        }
    }
    pub(super) fn deactivate(&self, client_id: u32) {
        self.cancel(client_id);
        self.manager.borrow_mut().take();
    }
}
