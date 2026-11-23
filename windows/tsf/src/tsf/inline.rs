use super::*;
use std::path::PathBuf;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FillRect, GetDC, GetStockObject,
    GetTextExtentPoint32W, GetTextMetricsW, HFONT, InvalidateRect, NULL_BRUSH, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, TEXTMETRICW, TRANSPARENT, TextOutW, WHITE_BRUSH,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CS_HREDRAW, CS_VREDRAW, GetClientRect, GetCursorPos, GetWindowRect,
    MA_NOACTIVATE, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT,
};

const CANDIDATE_GUID: GUID = GUID::from_u128(0xd39fb88b_09bb_4ca2_a050_174b5ab3b7c0);
const CANDIDATE_CLASS: &str = "YiIMECandidate";
// Compact Microsoft Wubi (Win11) palette: light bar, gray pill, dark text, small gray hints.
const INK: u32 = 0x0020_2020;
const MUTED: u32 = 0x0070_7070;
const GRAY: u32 = 0x0080_8080;
const DISABLED: u32 = 0x00B0_B0B0;
const HIGHLIGHT: u32 = 0x00EB_EBEB;
const ACCENT: u32 = 0x005C_5C5C;
const BACKGROUND: u32 = 0x00F7_F7F7;
const BORDER: u32 = 0x00D8_D8D8;
const DIVIDER: u32 = 0x00E0_E0E0;
const FONT_PX: i32 = 14;
const SMALL_FONT_PX: i32 = 11;
const PAD_X: i32 = 8;
const PAD_Y: i32 = 4;
const ITEM_GAP: i32 = 16;
const NUM_GAP: i32 = 4;
const HINT_GAP: i32 = 2;
const PILL_PAD_X: i32 = 5;
const TRAIL_SLOT: i32 = 18;
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
    page: usize,
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

fn hint_for(
    input: &yiime_core::MixedInput,
    typed: &str,
    candidate: &yiime_core::Candidate,
) -> String {
    // Same annotations as Composer::set_code / MixedInput::candidate_hint.
    input.candidate_hint(typed, candidate)
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
            size * yiime_core::CANDIDATE_PAGES,
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
            let hint = hint_for(input, code, &candidate);
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

fn page_len() -> usize {
    page_size().max(1)
}

fn page_count(total: usize) -> usize {
    total.div_ceil(page_len()).max(1)
}

fn page_items(view: &CandidateView) -> &[DisplayCandidate] {
    let size = page_len();
    let start = view.page.saturating_mul(size);
    let end = (start + size).min(view.rows.len());
    if start >= view.rows.len() {
        &[]
    } else {
        &view.rows[start..end]
    }
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

struct BarLayout {
    width: i32,
    height: i32,
    text_y: i32,
    hint_y: i32,
    pill_top: i32,
    pill_bottom: i32,
    items: Vec<(i32, i32)>,
    divider_x: i32,
    prev_x: i32,
    next_x: i32,
    icon_x: i32,
    radius: i32,
}

fn measure_fonts(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    small: HFONT,
    dpi: u32,
) -> (i32, i32, i32) {
    let old = unsafe { SelectObject(hdc, font as _) };
    let mut metrics: TEXTMETRICW = unsafe { std::mem::zeroed() };
    unsafe { GetTextMetricsW(hdc, &mut metrics) };
    let text_h = metrics.tmHeight.max(scale_px(FONT_PX, dpi));
    unsafe { SelectObject(hdc, small as _) };
    unsafe { GetTextMetricsW(hdc, &mut metrics) };
    let small_h = metrics.tmHeight.max(scale_px(SMALL_FONT_PX, dpi));
    unsafe { SelectObject(hdc, old) };
    let pad_y = scale_px(PAD_Y, dpi);
    (text_h, small_h, (text_h + 2 * pad_y).max(scale_px(24, dpi)))
}

fn item_width(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    small: HFONT,
    index: usize,
    row: &DisplayCandidate,
    dpi: u32,
) -> i32 {
    let num = format!("{}", index + 1);
    let num_gap = scale_px(NUM_GAP, dpi);
    let hint_gap = scale_px(HINT_GAP, dpi);
    unsafe { SelectObject(hdc, font as _) };
    let mut w = unsafe { text_width(hdc, &num) } + num_gap + unsafe { text_width(hdc, &row.text) };
    if !row.hint.is_empty() {
        unsafe { SelectObject(hdc, small as _) };
        w += hint_gap + unsafe { text_width(hdc, &row.hint) };
        unsafe { SelectObject(hdc, font as _) };
    }
    w
}

fn layout_bar(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    small: HFONT,
    view: &CandidateView,
    dpi: u32,
) -> BarLayout {
    let (text_h, small_h, height) = measure_fonts(hdc, font, small, dpi);
    let pad_x = scale_px(PAD_X, dpi);
    let pad_y = scale_px(PAD_Y, dpi);
    let gap = scale_px(ITEM_GAP, dpi);
    let slot = scale_px(TRAIL_SLOT, dpi);
    let radius = scale_px(8, dpi);
    let text_y = (height - text_h) / 2;
    let hint_y = (height - small_h) / 2 + scale_px(1, dpi);
    let pill_top = pad_y.max(1);
    let pill_bottom = height - pad_y.max(1);
    let items_on_page = page_items(view);
    let mut x = pad_x;
    let mut items = Vec::new();
    for (index, row) in items_on_page.iter().enumerate() {
        let w = item_width(hdc, font, small, index, row, dpi);
        items.push((x, w));
        x += w + gap;
    }
    if items.is_empty() {
        x += scale_px(24, dpi);
    }
    // divider + ◀ + ▶ + menu icon, as in the Win11 Wubi bar.
    let divider_x = x - gap / 2 + scale_px(2, dpi);
    x = divider_x + scale_px(8, dpi);
    let prev_x = x;
    x += slot;
    let next_x = x;
    x += slot;
    let icon_x = x;
    x += slot + pad_x;
    BarLayout {
        width: x.max(scale_px(80, dpi)),
        height,
        text_y,
        hint_y,
        pill_top,
        pill_bottom,
        items,
        divider_x,
        prev_x,
        next_x,
        icon_x,
        radius,
    }
}

fn measure_view(hwnd: HWND, view: &CandidateView) -> (i32, i32) {
    let dpi = window_dpi(hwnd);
    let hdc = unsafe { GetDC(hwnd) };
    if hdc.is_null() {
        return (scale_px(200, dpi), scale_px(26, dpi));
    }
    let font = unsafe { create_ui_font(scale_px(FONT_PX, dpi)) };
    let small = unsafe { create_ui_font(scale_px(SMALL_FONT_PX, dpi)) };
    let layout = layout_bar(hdc, font, small, view, dpi);
    unsafe {
        if !font.is_null() {
            DeleteObject(font as _);
        }
        if !small.is_null() {
            DeleteObject(small as _);
        }
        ReleaseDC(hwnd, hdc);
    }
    (layout.width, layout.height)
}

fn apply_window_shape(hwnd: HWND, width: i32, height: i32) {
    let preference = DWMWCP_ROUNDSMALL;
    let rounded = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const _ as _,
            std::mem::size_of_val(&preference) as u32,
        )
    } >= 0;
    if !rounded && width > 0 && height > 0 {
        let radius = scale_px(8, window_dpi(hwnd));
        let region = unsafe { CreateRoundRectRgn(0, 0, width, height, radius, radius) };
        if !region.is_null() {
            unsafe { SetWindowRgn(hwnd, region, 1) };
        }
    }
}

fn fill_round(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    rect: windows_sys::Win32::Foundation::RECT,
    color: u32,
    radius: i32,
) {
    let brush = unsafe { CreateSolidBrush(color) };
    let pen = unsafe { CreatePen(PS_SOLID, 1, color) };
    let old_brush = unsafe { SelectObject(hdc, brush as _) };
    let old_pen = unsafe { SelectObject(hdc, pen as _) };
    unsafe {
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        )
    };
    unsafe { SelectObject(hdc, old_brush) };
    unsafe { SelectObject(hdc, old_pen) };
    unsafe { DeleteObject(brush as _) };
    unsafe { DeleteObject(pen as _) };
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
    view: Rc<RefCell<CandidateView>>,
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
        Ok(page_items(&self.view.borrow()).len() as u32)
    }
    fn GetSelection(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetString(&self, index: u32) -> Result<BSTR> {
        page_items(&self.view.borrow())
            .get(index as usize)
            .map(|candidate| BSTR::from(candidate.text.as_str()))
            .ok_or(E_FAIL.into())
    }
    fn GetPageIndex(&self, index: *mut u32, size: u32, count: *mut u32) -> Result<()> {
        if count.is_null() || (size > 0 && index.is_null()) {
            return Err(E_FAIL.into());
        }
        let pages = page_count(self.view.borrow().rows.len()) as u32;
        let step = page_len() as u32;
        unsafe {
            *count = pages;
            let n = size.min(pages);
            for i in 0..n {
                *index.add(i as usize) = i * step;
            }
        }
        Ok(())
    }
    fn SetPageIndex(&self, _index: *const u32, _count: u32) -> Result<()> {
        Ok(())
    }
    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(self.view.borrow().page as u32)
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
    /// Last GetTextExt on the composition range succeeded. Grok Bot / Firefox often fail, and
    /// a different selection GetTextExt can return a misleading rectangle.
    last_ext_ok: Rc<Cell<bool>>,
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
                self.ui.last_ext_ok.set(true);
                place(window, position);
            } else {
                self.ui.last_ext_ok.set(false);
                if let Some(position) = self.ui.last_caret.get() {
                    place(window, position);
                }
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
    let background = unsafe { CreateSolidBrush(BACKGROUND) };
    unsafe { FillRect(hdc, &client, background) };
    unsafe { DeleteObject(background as _) };
    unsafe { SetBkMode(hdc, TRANSPARENT as i32) };
    let Some(view) = view.as_ref() else {
        unsafe { EndPaint(hwnd, &paint) };
        return;
    };
    let dpi = window_dpi(hwnd);
    let font = unsafe { create_ui_font(scale_px(FONT_PX, dpi)) };
    let small = unsafe { create_ui_font(scale_px(SMALL_FONT_PX, dpi)) };
    let layout = layout_bar(hdc, font, small, view, dpi);
    let old = unsafe { SelectObject(hdc, font as _) };
    let items = page_items(view);
    let num_gap = scale_px(NUM_GAP, dpi);
    let hint_gap = scale_px(HINT_GAP, dpi);
    let pill = scale_px(PILL_PAD_X, dpi);
    let accent_w = scale_px(2, dpi).max(2);
    let accent_inset = scale_px(3, dpi);
    for (index, row) in items.iter().enumerate() {
        let Some(&(x, w)) = layout.items.get(index) else {
            break;
        };
        if index == 0 {
            fill_round(
                hdc,
                windows_sys::Win32::Foundation::RECT {
                    left: x - pill,
                    top: layout.pill_top,
                    right: x + w + pill,
                    bottom: layout.pill_bottom,
                },
                HIGHLIGHT,
                scale_px(4, dpi),
            );
            let accent = unsafe { CreateSolidBrush(ACCENT) };
            let bar = windows_sys::Win32::Foundation::RECT {
                left: x - pill,
                top: layout.pill_top + accent_inset,
                right: x - pill + accent_w,
                bottom: layout.pill_bottom - accent_inset,
            };
            unsafe {
                FillRect(hdc, &bar, accent);
                DeleteObject(accent as _);
            }
        }
        let number = format!("{}", index + 1);
        unsafe { SelectObject(hdc, font as _) };
        let num_w = unsafe { text_width(hdc, &number) };
        unsafe { draw_text(hdc, x, layout.text_y, MUTED, &number) };
        let text_x = x + num_w + num_gap;
        let color = if row.complete { INK } else { GRAY };
        unsafe { draw_text(hdc, text_x, layout.text_y, color, &row.text) };
        if !row.hint.is_empty() {
            let hx = text_x + unsafe { text_width(hdc, &row.text) } + hint_gap;
            unsafe {
                let previous = SelectObject(hdc, small as _);
                draw_text(hdc, hx, layout.hint_y, GRAY, &row.hint);
                SelectObject(hdc, previous);
            }
        }
    }
    let divider = unsafe { CreateSolidBrush(DIVIDER) };
    let mid = windows_sys::Win32::Foundation::RECT {
        left: layout.divider_x,
        top: layout.pill_top + scale_px(2, dpi),
        right: layout.divider_x + scale_px(1, dpi).max(1),
        bottom: layout.pill_bottom - scale_px(2, dpi),
    };
    unsafe {
        FillRect(hdc, &mid, divider);
        DeleteObject(divider as _);
    }
    let pages = page_count(view.rows.len());
    let prev_color = if view.page > 0 { MUTED } else { DISABLED };
    let next_color = if view.page + 1 < pages {
        MUTED
    } else {
        DISABLED
    };
    unsafe { SelectObject(hdc, small as _) };
    unsafe { draw_text(hdc, layout.prev_x, layout.hint_y, prev_color, "◀") };
    unsafe { draw_text(hdc, layout.next_x, layout.hint_y, next_color, "▶") };
    // Menu button: a compact rounded chip with a down chevron, not a Microsoft logo.
    fill_round(
        hdc,
        windows_sys::Win32::Foundation::RECT {
            left: layout.icon_x,
            top: layout.pill_top,
            right: layout.icon_x + scale_px(TRAIL_SLOT, dpi) - scale_px(2, dpi),
            bottom: layout.pill_bottom,
        },
        HIGHLIGHT,
        scale_px(4, dpi),
    );
    unsafe {
        draw_text(
            hdc,
            layout.icon_x + scale_px(3, dpi),
            layout.hint_y,
            MUTED,
            "▾",
        )
    };
    let border = unsafe { CreatePen(PS_SOLID, 1, BORDER) };
    let empty = unsafe { GetStockObject(NULL_BRUSH) };
    let old_pen = unsafe { SelectObject(hdc, border as _) };
    let old_brush = unsafe { SelectObject(hdc, empty) };
    unsafe {
        RoundRect(
            hdc,
            client.left,
            client.top,
            client.right,
            client.bottom,
            layout.radius,
            layout.radius,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old);
        DeleteObject(border as _);
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
            view: self.view.clone(),
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
        {
            let mut view = self.view.borrow_mut();
            let page = if view.code == code { view.page } else { 0 };
            *view = CandidateView {
                code: code.to_owned(),
                rows,
                page,
            };
            let last = page_count(view.rows.len()).saturating_sub(1);
            view.page = view.page.min(last);
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
                    wide("").as_ptr(),
                    WS_POPUP,
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
            apply_window_shape(window, width, height);
            InvalidateRect(window, std::ptr::null(), 1);
        }
        let position = if let Some(position) = position {
            self.last_caret.set(Some(position));
            self.last_ext_ok.set(true);
            Some(position)
        } else if let Some(position) = self.last_caret.get() {
            self.last_ext_ok.set(false);
            log("候选窗：光标矩形为空，使用上次位置");
            Some(position)
        } else if let Some(position) = mouse_caret() {
            self.last_ext_ok.set(false);
            log("候选窗：光标矩形为空，使用鼠标位置");
            Some(position)
        } else {
            self.last_ext_ok.set(false);
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
    fn turn_page(&self, forward: bool) -> bool {
        let changed = {
            let mut view = self.view.borrow_mut();
            let last = page_count(view.rows.len()).saturating_sub(1);
            let next = if forward {
                view.page.saturating_add(1).min(last)
            } else {
                view.page.saturating_sub(1)
            };
            if next == view.page {
                false
            } else {
                view.page = next;
                true
            }
        };
        if changed {
            self.relayout();
        }
        true
    }
    fn relayout(&self) {
        let window = self.window.get();
        if window.is_null() || unsafe { IsWindow(window) } == 0 {
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
            apply_window_shape(window, width, height);
            InvalidateRect(window, std::ptr::null(), 1);
        }
        if let Some(position) = self.last_caret.get() {
            place(window, position);
        }
        self.refresh_element();
    }
    fn page_choice(&self, index: usize) -> Option<String> {
        page_items(&self.view.borrow())
            .get(index)
            .map(|item| item.text.clone())
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
                    page: 0,
                })),
                last_caret: Rc::new(Cell::new(None)),
                last_ext_ok: Rc::new(Cell::new(false)),
            },
        }
    }
    pub(super) fn activate(&self, manager: &ITfThreadMgr) {
        *self.ui.manager.borrow_mut() = manager.cast().ok();
    }
    pub(super) fn active(&self) -> bool {
        self.ui.state.borrow().is_some()
    }
    pub(super) fn last_caret(&self) -> Option<(i32, i32, i32)> {
        self.ui.last_caret.get()
    }
    /// Whether the last composition-range GetTextExt succeeded (same source as the candidate bar).
    pub(super) fn last_ext_ok(&self) -> bool {
        self.ui.last_ext_ok.get()
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
            // 0xBD/0xBB are `-`/`=` paging keys (yiime_core::paging_direction).
            state.context == *context
                && matches!(
                    key,
                    0x41..=0x5a | 0x08 | 0x1b | 0x0d | 0x20 | 0x31..=0x39 | 0xBD | 0xBB
                )
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
                let word = self.ui.page_choice(0).unwrap_or(code);
                self.edit(context, client_id, Action::Finish(word))
            }
            0x31..=0x39 => self
                .ui
                .page_choice(key - 0x31)
                .is_some_and(|word| self.edit(context, client_id, Action::Finish(word))),
            0xBD => self.ui.turn_page(false),
            0xBB => self.ui.turn_page(true),
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
