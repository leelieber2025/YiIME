use super::candidate::{create_font, relayout, scale_px, window_dpi};
use super::*;

const UI_SCALE_LABEL: &str = "造字框大小（五笔和翻译）";
const UI_SCALE_CHOICES: [&str; 5] = ["100%（默认）", "125%", "150%", "175%", "200%"];
const _: () = assert!(UI_SCALE_CHOICES.len() == yiime_core::UI_SCALES.len());

/// Opens the settings window (or brings it forward).
pub(super) unsafe fn open_settings(owner: HWND) {
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
    let (style, ex_style) = settings_styles();
    // Real size is applied in WM_CREATE from font metrics; this is only a placeholder.
    let dpi = window_dpi(owner);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale_px(720, dpi),
        bottom: scale_px(640, dpi),
    };
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, style, 0, ex_style, dpi);
    }
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let window = unsafe {
        CreateWindowExW(
            ex_style,
            wide(SETTINGS_CLASS).as_ptr(),
            wide(concat!("YiIME 译 · 设置 · v", env!("CARGO_PKG_VERSION"))).as_ptr(),
            style,
            0,
            0,
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
        center_settings_window(window);
        ShowWindow(window, SW_SHOWNORMAL);
        SetForegroundWindow(window);
        let url = GetDlgItem(window, SETTINGS_URL);
        SetFocus(url);
        SendMessageW(url, EM_SETSEL, 0, -1);
    }
}

fn settings_styles() -> (u32, u32) {
    (
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX,
        WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_DLGMODALFRAME,
    )
}

fn settings_dialog_font(dpi: u32) -> HFONT {
    unsafe {
        let mut metrics = std::mem::zeroed::<NONCLIENTMETRICSW>();
        metrics.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
        if SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            &mut metrics as *mut NONCLIENTMETRICSW as *mut _,
            0,
        ) != 0
        {
            let system = GetDpiForSystem().max(96);
            metrics.lfMessageFont.lfHeight = (i64::from(metrics.lfMessageFont.lfHeight)
                * i64::from(dpi)
                / i64::from(system)) as i32;
            metrics.lfMessageFont.lfQuality = CLEARTYPE_QUALITY;
            let font = CreateFontIndirectW(&metrics.lfMessageFont);
            if !font.is_null() {
                return font;
            }
        }
        create_font(scale_px(SETTINGS_FONT_PX, dpi))
    }
}

fn text_cx(font: HFONT, text: &str) -> i32 {
    unsafe {
        let dc = GetDC(null_mut());
        if dc.is_null() {
            return scale_px(8, 96) * text.chars().count() as i32;
        }
        let previous = SelectObject(dc, font as _);
        let units = wide(text);
        let mut size = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, units.as_ptr(), units.len() as i32 - 1, &mut size);
        SelectObject(dc, previous);
        ReleaseDC(null_mut(), dc);
        size.cx
    }
}

fn font_height(font: HFONT) -> i32 {
    unsafe {
        let dc = GetDC(null_mut());
        if dc.is_null() {
            return scale_px(SETTINGS_FONT_PX, 96);
        }
        let previous = SelectObject(dc, font as _);
        let mut tm = std::mem::zeroed::<TEXTMETRICW>();
        GetTextMetricsW(dc, &mut tm);
        SelectObject(dc, previous);
        ReleaseDC(null_mut(), dc);
        tm.tmHeight
    }
}

fn metric_for_dpi(index: i32, dpi: u32) -> i32 {
    let value = unsafe { GetSystemMetricsForDpi(index, dpi) };
    if value == 0 {
        unsafe { GetSystemMetrics(index) }
    } else {
        value
    }
}

struct SettingsMetrics {
    margin: i32,
    gutter: i32,
    label_h: i32,
    edit_h: i32,
    btn_h: i32,
    check_extra: i32,
    combo_extra: i32,
    combo_list_h: i32,
    label_gap: i32,
    row_gap: i32,
    tight_gap: i32,
    btn_pad: i32,
}

fn settings_metrics(font: HFONT, dpi: u32) -> SettingsMetrics {
    let font_h = font_height(font).max(1);
    SettingsMetrics {
        margin: scale_px(12, dpi),
        gutter: scale_px(12, dpi),
        label_h: font_h + scale_px(2, dpi),
        edit_h: font_h + scale_px(8, dpi),
        btn_h: font_h + scale_px(12, dpi),
        check_extra: metric_for_dpi(SM_CXMENUCHECK, dpi).max(scale_px(16, dpi)) + scale_px(8, dpi),
        combo_extra: metric_for_dpi(SM_CXVSCROLL, dpi).max(scale_px(16, dpi)) + scale_px(12, dpi),
        combo_list_h: font_h * 10 + scale_px(8, dpi),
        label_gap: scale_px(2, dpi),
        row_gap: scale_px(6, dpi),
        tight_gap: scale_px(2, dpi),
        btn_pad: scale_px(8, dpi),
    }
}

struct SettingsLayout {
    /// Client width used for this layout (may be wider than `min_w` when the user resizes).
    client_w: i32,
    /// Unscrolled content height; also the minimum client height when it fits the work area.
    min_w: i32,
    min_h: i32,
    rects: Vec<(i32, i32, i32, i32, i32)>,
}

impl SettingsLayout {
    fn compute(font: HFONT, dpi: u32, client_w: Option<i32>) -> Self {
        let m = settings_metrics(font, dpi);
        let slack = scale_px(8, dpi);
        let mut col = 0;
        for (_, text, _) in OPTION_BOXES {
            col = col.max(text_cx(font, text) + m.check_extra);
        }
        for engine in Engine::ALL {
            col = col.max(text_cx(font, engine.label()) + m.combo_extra);
        }
        for style in TranslationStyle::ALL {
            col = col.max(text_cx(font, style.label()) + m.combo_extra);
        }
        for text in [
            "翻译引擎",
            "Azure 翻译密钥（加密保存）",
            "模型",
            "翻译风格",
            "Google Cloud 密钥（可选，加密保存）",
            "Azure 区域（如 eastus，全局可留空）",
            "API 密钥（Windows 加密保存）",
        ] {
            col = col.max(text_cx(font, text) + m.btn_pad);
        }
        col = col.max(text_cx(font, "失败时允许改用其他翻译服务") + m.check_extra);

        let mut preferred = m.margin * 2 + col * 2 + m.gutter;
        for (text, extra) in [
            ("DeepL 密钥（:fx 为免费版）", m.btn_pad),
            ("API 地址（OpenAI 兼容）", m.btn_pad),
            (
                "关闭思考（更快；接口不支持时会自动去掉该参数）",
                m.check_extra,
            ),
            ("自定义要求", m.btn_pad),
            ("普通五笔模式（不翻译，选字直接上屏）", m.check_extra),
            ("切换快捷键", m.btn_pad),
            (
                "输入记忆：保存翻译过的中文，快捷键找回重新编辑/翻译",
                m.check_extra,
            ),
        ] {
            preferred = preferred.max(m.margin * 2 + text_cx(font, text) + extra);
        }

        let scheme_label_w = text_cx(font, "输入方案") + m.btn_pad;
        let mut scheme_combo = text_cx(font, "五笔 + 全拼（默认）") + m.combo_extra;
        for item in Shuangpin::ALL {
            scheme_combo = scheme_combo.max(text_cx(font, item.label()) + m.combo_extra);
        }
        let import_w = text_cx(font, "导入搜狗/QQ 词库…") + m.btn_pad * 2;
        preferred = preferred
            .max(m.margin * 2 + scheme_label_w + m.gutter + scheme_combo + m.gutter + import_w);

        let hist_label_w = text_cx(font, "找回快捷键") + m.btn_pad;
        let clear_w = text_cx(font, "清除输入记忆") + m.btn_pad * 2;
        preferred = preferred
            .max(m.margin * 2 + hist_label_w + m.gutter + scale_px(160, dpi) + m.gutter + clear_w);

        let scale_label_w = text_cx(font, UI_SCALE_LABEL) + m.btn_pad;
        let mut scale_combo = m.combo_extra;
        for label in UI_SCALE_CHOICES {
            scale_combo = scale_combo.max(text_cx(font, label) + m.combo_extra);
        }
        preferred = preferred.max(m.margin * 2 + scale_label_w + m.gutter + scale_combo);

        let move_w = (text_cx(font, "上移") + m.btn_pad * 2).max(scale_px(52, dpi));
        let tab_row = (0..LLM_PROFILE_COUNT)
            .map(|index| text_cx(font, llm_profile_title(index)) + m.btn_pad * 2)
            .sum::<i32>()
            + m.gutter / 2 * (LLM_PROFILE_COUNT as i32 - 1)
            + m.gutter
            + move_w * 2
            + m.gutter / 2;
        preferred = preferred.max(m.margin * 2 + tab_row);

        let words_w = text_cx(font, "用户词库…") + m.btn_pad * 2;
        let save_w = (text_cx(font, "保存") + m.btn_pad * 2).max(scale_px(80, dpi));
        let cancel_w = (text_cx(font, "取消") + m.btn_pad * 2).max(scale_px(80, dpi));
        preferred =
            preferred.max(m.margin * 2 + words_w + m.gutter + save_w + m.gutter / 2 + cancel_w);
        preferred += slack;

        let width = client_w.unwrap_or(preferred).max(preferred);
        let col_w = (width - m.margin * 2 - m.gutter) / 2;
        let left = m.margin;
        let right = m.margin + col_w + m.gutter;
        let full_w = width - m.margin * 2;

        let mut y = m.margin;
        let mut rects = Vec::new();
        let mut place = |id: i32, x: i32, y: i32, w: i32, h: i32| {
            rects.push((id, x, y, w, h));
        };

        place(LBL_ENGINE, left, y, col_w, m.label_h);
        place(LBL_GOOGLE_KEY, right, y, col_w, m.label_h);
        y += m.label_h + m.label_gap;
        place(SETTINGS_ENGINE, left, y, col_w, m.combo_list_h);
        place(SETTINGS_GOOGLE_KEY, right, y, col_w, m.edit_h);
        y += m.edit_h + m.row_gap;

        place(LBL_AZURE_KEY, left, y, col_w, m.label_h);
        place(LBL_AZURE_REGION, right, y, col_w, m.label_h);
        y += m.label_h + m.label_gap;
        place(SETTINGS_AZURE_KEY, left, y, col_w, m.edit_h);
        place(SETTINGS_AZURE_REGION, right, y, col_w, m.edit_h);
        y += m.edit_h + m.row_gap;

        let deepl_label_w = text_cx(font, "DeepL 密钥（:fx 为免费版）") + m.btn_pad;
        place(
            LBL_DEEPL,
            left,
            y + (m.edit_h - m.label_h) / 2,
            deepl_label_w.min(col_w),
            m.label_h,
        );
        let deepl_edit_x = left + deepl_label_w.min(col_w) + m.gutter / 2;
        place(
            SETTINGS_DEEPL_KEY,
            deepl_edit_x,
            y,
            (left + col_w - deepl_edit_x).max(scale_px(80, dpi)),
            m.edit_h,
        );
        place(SETTINGS_FALLBACK, right, y, col_w, m.edit_h);
        y += m.edit_h + m.row_gap;

        let move_w = (text_cx(font, "上移") + m.btn_pad * 2).max(scale_px(52, dpi));
        let down_x = left + full_w - move_w;
        let up_x = down_x - m.gutter / 2 - move_w;
        let tab_area = (up_x - m.gutter / 2 - left).max(scale_px(180, dpi));
        let tab_gap = m.gutter / 2;
        let tab_w = ((tab_area - tab_gap * (LLM_PROFILE_COUNT as i32 - 1))
            / LLM_PROFILE_COUNT as i32)
            .max(scale_px(64, dpi));
        for (index, id) in SETTINGS_LLM_TAB.iter().enumerate() {
            place(
                *id,
                left + index as i32 * (tab_w + tab_gap),
                y,
                tab_w,
                m.btn_h,
            );
        }
        place(SETTINGS_LLM_UP, up_x, y, move_w, m.btn_h);
        place(SETTINGS_LLM_DOWN, down_x, y, move_w, m.btn_h);
        y += m.btn_h + m.tight_gap;

        // All three profile field groups share the same coordinates; only the selected tab is shown.
        let url_label_w = text_cx(font, "API 地址") + m.btn_pad;
        let model_label_w = text_cx(font, "模型") + m.btn_pad;
        let key_label_w = text_cx(font, "API 密钥") + m.btn_pad;
        let fields_top = y;
        for index in 0..LLM_PROFILE_COUNT {
            let mut fy = fields_top;
            place(
                LBL_LLM_URL[index],
                left,
                fy + (m.edit_h - m.label_h) / 2,
                url_label_w,
                m.label_h,
            );
            place(
                SETTINGS_LLM_URL[index],
                left + url_label_w + m.gutter / 2,
                fy,
                (full_w - url_label_w - m.gutter / 2).max(scale_px(80, dpi)),
                m.edit_h,
            );
            fy += m.edit_h + m.tight_gap;

            place(
                LBL_LLM_MODEL[index],
                left,
                fy + (m.edit_h - m.label_h) / 2,
                model_label_w,
                m.label_h,
            );
            let model_x = left + model_label_w + m.gutter / 2;
            let model_w = (left + col_w - model_x).max(scale_px(60, dpi));
            place(SETTINGS_LLM_MODEL[index], model_x, fy, model_w, m.edit_h);
            place(
                LBL_LLM_KEY[index],
                right,
                fy + (m.edit_h - m.label_h) / 2,
                key_label_w,
                m.label_h,
            );
            let key_x = right + key_label_w + m.gutter / 2;
            place(
                SETTINGS_LLM_KEY[index],
                key_x,
                fy,
                (left + full_w - key_x).max(scale_px(60, dpi)),
                m.edit_h,
            );
            fy += m.edit_h + m.tight_gap;
            place(SETTINGS_LLM_THINK[index], left, fy, full_w, m.edit_h);
            fy += m.edit_h;
            if index == 0 {
                y = fy + m.row_gap;
            }
        }

        let style_label_w = text_cx(font, "翻译风格") + m.btn_pad;
        place(
            LBL_STYLE,
            left,
            y + (m.edit_h - m.label_h) / 2,
            style_label_w,
            m.label_h,
        );
        let style_x = left + style_label_w + m.gutter / 2;
        place(
            SETTINGS_STYLE,
            style_x,
            y,
            (left + col_w - style_x).max(scale_px(80, dpi)),
            m.combo_list_h,
        );
        place(SETTINGS_PLAIN, right, y, col_w, m.edit_h);
        y += m.edit_h.max(m.btn_h) + m.row_gap;

        let custom_label_w = text_cx(font, "自定义要求") + m.btn_pad;
        place(
            LBL_CUSTOM,
            left,
            y + (m.edit_h - m.label_h) / 2,
            custom_label_w,
            m.label_h,
        );
        place(
            SETTINGS_CUSTOM,
            left + custom_label_w + m.gutter / 2,
            y,
            (full_w - custom_label_w - m.gutter / 2).max(scale_px(80, dpi)),
            m.edit_h,
        );
        y += m.edit_h + m.row_gap;

        let hotkey_label_w = text_cx(font, "切换快捷键") + m.btn_pad;
        place(
            LBL_HOTKEY,
            left,
            y + (m.edit_h - m.label_h) / 2,
            hotkey_label_w,
            m.label_h,
        );
        let hotkey_x = left + hotkey_label_w + m.gutter / 2;
        place(
            SETTINGS_HOTKEY,
            hotkey_x,
            y,
            (left + col_w - hotkey_x).max(scale_px(80, dpi)),
            m.edit_h,
        );

        let page_label_w = text_cx(font, "每页候选数（1–9）") + m.btn_pad;
        let page_edit_w = scale_px(48, dpi);
        place(
            LBL_PAGE_SIZE,
            right,
            y + (m.edit_h - m.label_h) / 2,
            page_label_w,
            m.label_h,
        );
        place(
            SETTINGS_PAGE_SIZE,
            right + page_label_w,
            y,
            page_edit_w.min(col_w.saturating_sub(page_label_w)),
            m.edit_h,
        );
        y += m.edit_h + m.row_gap;

        place(
            LBL_UI_SCALE,
            left,
            y + (m.edit_h - m.label_h) / 2,
            scale_label_w,
            m.label_h,
        );
        place(
            SETTINGS_UI_SCALE,
            left + scale_label_w + m.gutter / 2,
            y,
            scale_combo,
            m.combo_list_h,
        );
        y += m.edit_h + m.row_gap;

        for (index, (id, _, _)) in OPTION_BOXES.iter().enumerate() {
            let x = if index % 2 == 0 { left } else { right };
            place(*id, x, y, col_w, m.edit_h);
            if index % 2 == 1 || index + 1 == OPTION_BOXES.len() {
                y += m.edit_h + m.tight_gap;
            }
        }
        y += m.row_gap - m.tight_gap;

        let scheme_y = y;
        place(
            LBL_SCHEME,
            left,
            scheme_y + (m.edit_h - m.label_h) / 2,
            scheme_label_w,
            m.label_h,
        );
        let import_x = left + full_w - import_w;
        let scheme_x = left + scheme_label_w + m.gutter / 2;
        let scheme_w = (import_x - m.gutter / 2 - scheme_x).max(scheme_combo);
        place(
            SETTINGS_SCHEME,
            scheme_x,
            scheme_y,
            scheme_w,
            m.combo_list_h,
        );
        place(SETTINGS_IMPORT, import_x, scheme_y, import_w, m.btn_h);
        y += m.btn_h.max(m.edit_h) + m.row_gap;

        place(SETTINGS_HISTORY, left, y, full_w, m.edit_h);
        y += m.edit_h + m.row_gap;

        place(
            LBL_HISTORY_KEY,
            left,
            y + (m.edit_h - m.label_h) / 2,
            hist_label_w,
            m.label_h,
        );
        let hist_edit_x = left + hist_label_w + m.gutter / 2;
        let clear_x = left + full_w - clear_w;
        let hist_edit_w = (clear_x - m.gutter / 2 - hist_edit_x).max(scale_px(120, dpi));
        place(SETTINGS_HISTORY_KEY, hist_edit_x, y, hist_edit_w, m.edit_h);
        place(SETTINGS_CLEAR_HISTORY, clear_x, y, clear_w, m.btn_h);
        y += m.btn_h.max(m.edit_h) + m.row_gap;

        let btn_y = y;
        place(SETTINGS_USER_WORDS, left, btn_y, words_w, m.btn_h);
        place(
            ID_CANCEL,
            left + full_w - cancel_w,
            btn_y,
            cancel_w,
            m.btn_h,
        );
        place(
            ID_OK,
            left + full_w - cancel_w - m.gutter / 2 - save_w,
            btn_y,
            save_w,
            m.btn_h,
        );
        y += m.btn_h + m.margin;

        Self {
            client_w: width,
            min_w: preferred,
            min_h: y,
            rects,
        }
    }
}

fn work_area(hwnd: HWND) -> RECT {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = std::mem::zeroed::<MONITORINFO>();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
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

fn window_size_for_client(cw: i32, ch: i32, dpi: u32) -> (i32, i32) {
    let (style, ex_style) = settings_styles();
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, style, 0, ex_style, dpi);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

fn center_settings_window(hwnd: HWND) {
    unsafe {
        let mut rect = std::mem::zeroed::<RECT>();
        GetWindowRect(hwnd, &mut rect);
        let work = work_area(hwnd);
        let width = (rect.right - rect.left).min(work.right - work.left);
        let height = (rect.bottom - rect.top).min(work.bottom - work.top);
        let x = work.left + ((work.right - work.left) - width).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - height).max(0) / 3;
        SetWindowPos(
            hwnd,
            null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn apply_child_font(parent: HWND, font: HFONT) {
    unsafe {
        let mut child = GetWindow(parent, GW_CHILD);
        while !child.is_null() {
            SendMessageW(child, WM_SETFONT, font as usize, 1);
            child = GetWindow(child, GW_HWNDNEXT);
        }
    }
}

fn apply_settings_layout(hwnd: HWND, layout: &SettingsLayout, scroll: i32) {
    unsafe {
        for (id, x, y, w, h) in &layout.rects {
            let child = GetDlgItem(hwnd, *id);
            if !child.is_null() {
                SetWindowPos(
                    child,
                    null_mut(),
                    *x,
                    *y - scroll,
                    *w,
                    *h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
}

fn sync_settings_scroll(hwnd: HWND, state: &State, content_h: i32, client_h: i32) {
    let max_scroll = (content_h - client_h).max(0);
    let scroll = state.settings_scroll.get().clamp(0, max_scroll);
    state.settings_scroll.set(scroll);
    unsafe {
        let mut info = std::mem::zeroed::<SCROLLINFO>();
        info.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
        info.fMask = SIF_RANGE | SIF_PAGE | SIF_POS;
        info.nMin = 0;
        info.nMax = content_h.max(1) - 1;
        info.nPage = client_h.max(1) as u32;
        info.nPos = scroll;
        SetScrollInfo(hwnd, SB_VERT, &info, 1);
        ShowScrollBar(hwnd, SB_VERT, (max_scroll > 0) as i32);
    }
}

fn relayout_settings(hwnd: HWND, state: &State) {
    let font = state.settings_font.get();
    if font.is_null() {
        return;
    }
    let dpi = window_dpi(hwnd);
    let mut client = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetClientRect(hwnd, &mut client);
    }
    let layout = SettingsLayout::compute(font, dpi, Some(client.right).filter(|w| *w > 0));
    state.settings_min.set((layout.min_w, layout.min_h));
    state.settings_dpi.set(dpi);
    sync_settings_scroll(hwnd, state, layout.min_h, client.bottom);
    apply_settings_layout(hwnd, &layout, state.settings_scroll.get());
    apply_llm_tab(hwnd, state);
    unsafe {
        InvalidateRect(hwnd, null(), 1);
    }
}

fn size_settings_to_layout(hwnd: HWND, state: &State, layout: &SettingsLayout) {
    let dpi = window_dpi(hwnd);
    let work = work_area(hwnd);
    let (mut ww, mut wh) = window_size_for_client(layout.client_w, layout.min_h, dpi);
    ww = ww.min(work.right - work.left);
    wh = wh.min(work.bottom - work.top);
    unsafe {
        SetWindowPos(
            hwnd,
            null_mut(),
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    relayout_settings(hwnd, state);
}

fn add_control(parent: HWND, class: &str, text: &str, style: u32, id: i32) -> HWND {
    unsafe { control(parent, class, text, style, 0, 0, 1, 1, id) }
}

fn add_label(parent: HWND, text: &str, id: i32) -> HWND {
    add_control(parent, "STATIC", text, 0x80, id)
}

fn llm_profile_title(index: usize) -> &'static str {
    match index {
        0 => "模型 1",
        1 => "模型 2（可选）",
        _ => "模型 3（可选）",
    }
}

fn llm_profile_field_ids(index: usize) -> [i32; 7] {
    [
        LBL_LLM_URL[index],
        SETTINGS_LLM_URL[index],
        LBL_LLM_MODEL[index],
        SETTINGS_LLM_MODEL[index],
        LBL_LLM_KEY[index],
        SETTINGS_LLM_KEY[index],
        SETTINGS_LLM_THINK[index],
    ]
}

fn apply_llm_tab(hwnd: HWND, state: &State) {
    let selected = state.settings_llm_tab.get().min(LLM_PROFILE_COUNT - 1);
    state.settings_llm_tab.set(selected);
    unsafe {
        for (index, tab) in SETTINGS_LLM_TAB.iter().enumerate() {
            let show = if index == selected { SW_SHOW } else { SW_HIDE };
            for id in llm_profile_field_ids(index) {
                ShowWindow(GetDlgItem(hwnd, id), show);
            }
            SendMessageW(
                GetDlgItem(hwnd, *tab),
                BM_SETCHECK,
                (index == selected) as usize,
                0,
            );
        }
        EnableWindow(GetDlgItem(hwnd, SETTINGS_LLM_UP), (selected > 0) as i32);
        EnableWindow(
            GetDlgItem(hwnd, SETTINGS_LLM_DOWN),
            (selected + 1 < LLM_PROFILE_COUNT) as i32,
        );
    }
}

fn select_llm_tab(hwnd: HWND, state: &State, index: usize) {
    state.settings_llm_tab.set(index.min(LLM_PROFILE_COUNT - 1));
    apply_llm_tab(hwnd, state);
}

fn read_llm_profile(hwnd: HWND, index: usize, field: &impl Fn(i32) -> String) -> LlmProfile {
    LlmProfile {
        url: field(SETTINGS_LLM_URL[index]),
        model: field(SETTINGS_LLM_MODEL[index]),
        key: field(SETTINGS_LLM_KEY[index]),
        disable_thinking: unsafe {
            SendMessageW(
                GetDlgItem(hwnd, SETTINGS_LLM_THINK[index]),
                BM_GETCHECK,
                0,
                0,
            )
        } == 1,
    }
}

fn swap_window_text(hwnd: HWND, a: i32, b: i32) {
    unsafe {
        let left = GetDlgItem(hwnd, a);
        let right = GetDlgItem(hwnd, b);
        let first = get_text(left);
        let second = get_text(right);
        SetWindowTextW(left, wide(&second).as_ptr());
        SetWindowTextW(right, wide(&first).as_ptr());
    }
}

fn swap_llm_profile_fields(hwnd: HWND, left: usize, right: usize) {
    swap_window_text(hwnd, SETTINGS_LLM_URL[left], SETTINGS_LLM_URL[right]);
    swap_window_text(hwnd, SETTINGS_LLM_MODEL[left], SETTINGS_LLM_MODEL[right]);
    swap_window_text(hwnd, SETTINGS_LLM_KEY[left], SETTINGS_LLM_KEY[right]);
    unsafe {
        let a = GetDlgItem(hwnd, SETTINGS_LLM_THINK[left]);
        let b = GetDlgItem(hwnd, SETTINGS_LLM_THINK[right]);
        let first = SendMessageW(a, BM_GETCHECK, 0, 0);
        let second = SendMessageW(b, BM_GETCHECK, 0, 0);
        SendMessageW(a, BM_SETCHECK, second as usize, 0);
        SendMessageW(b, BM_SETCHECK, first as usize, 0);
    }
}

pub(super) unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let owner = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as HWND;
    match msg {
        WM_CREATE => unsafe {
            let create = &*(l as *const CREATESTRUCTW);
            let owner = create.lpCreateParams as HWND;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, owner as isize);
            let Some(state) = state_of(owner) else {
                return -1;
            };
            let settings = current_settings(state);
            state.dialog_keys_unreadable.set(settings.keys_unreadable);
            *state.dialog_base.borrow_mut() = Some(settings.clone());
            let dpi = window_dpi(hwnd);
            let font = settings_dialog_font(dpi);
            let previous = state.settings_font.replace(font);
            if !previous.is_null() {
                DeleteObject(previous as _);
            }
            state.settings_dpi.set(dpi);
            state.settings_scroll.set(0);
            state.settings_llm_tab.set(0);

            add_label(hwnd, "翻译引擎", LBL_ENGINE);
            let _engine = add_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                SETTINGS_ENGINE,
            );
            for item in Engine::ALL {
                SendMessageW(
                    _engine,
                    CB_ADDSTRING,
                    0,
                    wide(item.label()).as_ptr() as isize,
                );
            }
            let selected = Engine::ALL
                .iter()
                .position(|item| *item == settings.engine)
                .unwrap_or(0);
            SendMessageW(_engine, CB_SETCURSEL, selected, 0);
            add_label(hwnd, "Google Cloud 密钥（可选，加密保存）", LBL_GOOGLE_KEY);
            add_control(
                hwnd,
                "EDIT",
                &settings.google_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_GOOGLE_KEY,
            );
            add_label(hwnd, "Azure 翻译密钥（加密保存）", LBL_AZURE_KEY);
            add_control(
                hwnd,
                "EDIT",
                &settings.azure_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_AZURE_KEY,
            );
            add_label(
                hwnd,
                "Azure 区域（如 eastus，全局可留空）",
                LBL_AZURE_REGION,
            );
            add_control(
                hwnd,
                "EDIT",
                &settings.azure_region,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_AZURE_REGION,
            );
            add_label(hwnd, "DeepL 密钥（:fx 为免费版）", LBL_DEEPL);
            add_control(
                hwnd,
                "EDIT",
                &settings.deepl_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_DEEPL_KEY,
            );
            let fallback = add_control(
                hwnd,
                "BUTTON",
                "失败时允许改用其他翻译服务",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                SETTINGS_FALLBACK,
            );
            SendMessageW(fallback, BM_SETCHECK, settings.allow_fallback as usize, 0);

            add_control(
                hwnd,
                "BUTTON",
                "上移",
                BS_PUSHBUTTON as u32 | WS_TABSTOP,
                SETTINGS_LLM_UP,
            );
            add_control(
                hwnd,
                "BUTTON",
                "下移",
                BS_PUSHBUTTON as u32 | WS_TABSTOP,
                SETTINGS_LLM_DOWN,
            );
            let profiles = settings.llm_profiles();
            for (index, profile) in profiles.iter().enumerate() {
                let mut tab_style = BS_AUTORADIOBUTTON as u32 | BS_PUSHLIKE as u32 | WS_TABSTOP;
                if index == 0 {
                    tab_style |= WS_GROUP;
                }
                add_control(
                    hwnd,
                    "BUTTON",
                    llm_profile_title(index),
                    tab_style,
                    SETTINGS_LLM_TAB[index],
                );
                add_label(hwnd, "API 地址", LBL_LLM_URL[index]);
                add_control(
                    hwnd,
                    "EDIT",
                    &profile.url,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    SETTINGS_LLM_URL[index],
                );
                add_label(hwnd, "模型", LBL_LLM_MODEL[index]);
                add_control(
                    hwnd,
                    "EDIT",
                    &profile.model,
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    SETTINGS_LLM_MODEL[index],
                );
                add_label(hwnd, "API 密钥", LBL_LLM_KEY[index]);
                add_control(
                    hwnd,
                    "EDIT",
                    &profile.key,
                    ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                    SETTINGS_LLM_KEY[index],
                );
                let think = add_control(
                    hwnd,
                    "BUTTON",
                    "关闭思考（更快；接口不支持时会自动去掉该参数）",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                    SETTINGS_LLM_THINK[index],
                );
                SendMessageW(think, BM_SETCHECK, profile.disable_thinking as usize, 0);
            }
            add_label(hwnd, "翻译风格", LBL_STYLE);
            let combo = add_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
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
            add_label(hwnd, "自定义要求", LBL_CUSTOM);
            add_control(
                hwnd,
                "EDIT",
                &settings.custom,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_CUSTOM,
            );
            let plain = add_control(
                hwnd,
                "BUTTON",
                "普通五笔模式（不翻译，选字直接上屏）",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                SETTINGS_PLAIN,
            );
            SendMessageW(plain, BM_SETCHECK, plain_mode() as usize, 0);
            add_label(hwnd, "切换快捷键", LBL_HOTKEY);
            add_control(
                hwnd,
                "EDIT",
                &plain_toggle_text(),
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_HOTKEY,
            );
            add_label(hwnd, "每页候选数（1–9）", LBL_PAGE_SIZE);
            add_control(
                hwnd,
                "EDIT",
                &OPTIONS.page_size().to_string(),
                ES_NUMBER as u32 | WS_TABSTOP,
                SETTINGS_PAGE_SIZE,
            );
            SendMessageW(GetDlgItem(hwnd, SETTINGS_PAGE_SIZE), EM_LIMITTEXT, 2, 0);
            add_label(hwnd, UI_SCALE_LABEL, LBL_UI_SCALE);
            let scale = add_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                SETTINGS_UI_SCALE,
            );
            for label in UI_SCALE_CHOICES {
                SendMessageW(scale, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
            }
            let opened = OPTIONS.ui_scale();
            state.ui_scale_opened.set(opened);
            let selected = yiime_core::UI_SCALES
                .iter()
                .position(|percent| *percent == opened)
                .unwrap_or(0);
            SendMessageW(scale, CB_SETCURSEL, selected, 0);
            for (id, text, option_key) in OPTION_BOXES {
                let checkbox = add_control(
                    hwnd,
                    "BUTTON",
                    text,
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                    id,
                );
                // "中文标点" checked = PunctEnglish off.
                let on = option(option_key) != (option_key == Flag::PunctEnglish);
                SendMessageW(checkbox, BM_SETCHECK, on as usize, 0);
            }
            add_label(hwnd, "输入方案", LBL_SCHEME);
            let scheme = add_control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                SETTINGS_SCHEME,
            );
            SendMessageW(
                scheme,
                CB_ADDSTRING,
                0,
                wide("五笔 + 全拼（默认）").as_ptr() as isize,
            );
            for item in Shuangpin::ALL {
                SendMessageW(
                    scheme,
                    CB_ADDSTRING,
                    0,
                    wide(item.label()).as_ptr() as isize,
                );
            }
            let current = OPTIONS
                .shuangpin()
                .and_then(|current| Shuangpin::ALL.iter().position(|item| *item == current))
                .map_or(0, |index| index + 1);
            SendMessageW(scheme, CB_SETCURSEL, current, 0);
            let import_btn = add_control(
                hwnd,
                "BUTTON",
                "导入搜狗/QQ 词库…",
                WS_TABSTOP,
                SETTINGS_IMPORT,
            );
            if state.import_busy.get() {
                EnableWindow(import_btn, 0);
            }
            let history = add_control(
                hwnd,
                "BUTTON",
                "输入记忆：保存翻译过的中文，快捷键找回重新编辑/翻译",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                SETTINGS_HISTORY,
            );
            SendMessageW(
                history,
                BM_SETCHECK,
                option(Flag::HistoryEnabled) as usize,
                0,
            );
            add_label(hwnd, "找回快捷键", LBL_HISTORY_KEY);
            add_control(
                hwnd,
                "EDIT",
                &history_key_text(),
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                SETTINGS_HISTORY_KEY,
            );
            add_control(
                hwnd,
                "BUTTON",
                "清除输入记忆",
                WS_TABSTOP,
                SETTINGS_CLEAR_HISTORY,
            );
            add_control(hwnd, "BUTTON", "用户词库…", WS_TABSTOP, SETTINGS_USER_WORDS);
            add_control(
                hwnd,
                "BUTTON",
                "保存",
                BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
                ID_OK,
            );
            add_control(hwnd, "BUTTON", "取消", WS_TABSTOP, ID_CANCEL);

            apply_child_font(hwnd, font);
            let layout = SettingsLayout::compute(font, dpi, None);
            state.settings_min.set((layout.min_w, layout.min_h));
            size_settings_to_layout(hwnd, state, &layout);
            0
        },
        WM_SIZE => {
            if let Some(state) = unsafe { state_of(owner) }
                && !state.settings_font.get().is_null()
                && w != SIZE_MINIMIZED as usize
            {
                relayout_settings(hwnd, state);
            }
            0
        }
        WM_GETMINMAXINFO => {
            if let Some(state) = unsafe { state_of(owner) } {
                let (min_cw, min_ch) = state.settings_min.get();
                if min_cw > 0 {
                    let dpi = window_dpi(hwnd);
                    let work = work_area(hwnd);
                    let (min_ww, min_wh) = window_size_for_client(min_cw, min_ch, dpi);
                    let floor_h = window_size_for_client(min_cw, scale_px(360, dpi), dpi).1;
                    let info = l as *mut MINMAXINFO;
                    unsafe {
                        (*info).ptMinTrackSize.x = min_ww;
                        (*info).ptMinTrackSize.y = floor_h.min(min_wh).min(work.bottom - work.top);
                        (*info).ptMaxTrackSize.x = work.right - work.left;
                        (*info).ptMaxTrackSize.y = work.bottom - work.top;
                    }
                }
            }
            0
        }
        WM_DPICHANGED => {
            if let Some(state) = unsafe { state_of(owner) } {
                let dpi = ((w >> 16) as u32).max(96);
                let suggested = unsafe { &*(l as *const RECT) };
                let font = settings_dialog_font(dpi);
                let previous = state.settings_font.replace(font);
                if !previous.is_null() {
                    unsafe { DeleteObject(previous as _) };
                }
                apply_child_font(hwnd, font);
                state.settings_dpi.set(dpi);
                let preferred = SettingsLayout::compute(font, dpi, None);
                state.settings_min.set((preferred.min_w, preferred.min_h));
                let (need_w, need_h) =
                    window_size_for_client(preferred.min_w, preferred.min_h, dpi);
                let work = work_area(hwnd);
                let width = (suggested.right - suggested.left)
                    .max(need_w)
                    .min(work.right - work.left);
                let height = (suggested.bottom - suggested.top)
                    .max(need_h.min(work.bottom - work.top))
                    .min(work.bottom - work.top);
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        suggested.left,
                        suggested.top,
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                relayout_settings(hwnd, state);
            }
            0
        }
        WM_VSCROLL => {
            if let Some(state) = unsafe { state_of(owner) } {
                let mut client = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                unsafe {
                    GetClientRect(hwnd, &mut client);
                }
                let content_h = state.settings_min.get().1;
                let max_scroll = (content_h - client.bottom).max(0);
                let code = (w & 0xffff) as i32;
                let pos = ((w >> 16) & 0xffff) as i16 as i32;
                let line = scale_px(24, window_dpi(hwnd));
                let page = (client.bottom - line).max(line);
                let current = state.settings_scroll.get();
                let next = match code {
                    SB_LINEUP => current - line,
                    SB_LINEDOWN => current + line,
                    SB_PAGEUP => current - page,
                    SB_PAGEDOWN => current + page,
                    SB_THUMBTRACK | SB_THUMBPOSITION => pos,
                    SB_TOP => 0,
                    SB_BOTTOM => max_scroll,
                    _ => current,
                }
                .clamp(0, max_scroll);
                if next != current {
                    state.settings_scroll.set(next);
                    relayout_settings(hwnd, state);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if let Some(state) = unsafe { state_of(owner) } {
                let delta = ((w >> 16) & 0xffff) as i16 as i32;
                let line = scale_px(24, window_dpi(hwnd));
                let next = state.settings_scroll.get() - delta * line / 120;
                state.settings_scroll.set(next);
                relayout_settings(hwnd, state);
            }
            0
        }
        WM_COMMAND => unsafe {
            let command_id = (w & 0xffff) as i32;
            if let Some(index) = SETTINGS_LLM_TAB.iter().position(|id| *id == command_id) {
                if let Some(state) = state_of(owner) {
                    select_llm_tab(hwnd, state, index);
                }
                return 0;
            }
            if command_id == SETTINGS_LLM_UP || command_id == SETTINGS_LLM_DOWN {
                if let Some(state) = state_of(owner) {
                    let index = state.settings_llm_tab.get();
                    if command_id == SETTINGS_LLM_UP && index > 0 {
                        swap_llm_profile_fields(hwnd, index - 1, index);
                        select_llm_tab(hwnd, state, index - 1);
                    } else if command_id == SETTINGS_LLM_DOWN && index + 1 < LLM_PROFILE_COUNT {
                        swap_llm_profile_fields(hwnd, index, index + 1);
                        select_llm_tab(hwnd, state, index + 1);
                    }
                }
                return 0;
            }
            match command_id {
                ID_OK => {
                    let Some(state) = state_of(owner) else {
                        DestroyWindow(hwnd);
                        return 0;
                    };
                    let field = |id| get_text(GetDlgItem(hwnd, id)).trim().to_owned();
                    let index = SendMessageW(GetDlgItem(hwnd, SETTINGS_STYLE), CB_GETCURSEL, 0, 0);
                    let engine_index =
                        SendMessageW(GetDlgItem(hwnd, SETTINGS_ENGINE), CB_GETCURSEL, 0, 0);
                    let settings = ApiSettings {
                        keys_unreadable: state.dialog_keys_unreadable.get(),
                        // save() refuses on its own while settings.json is not an object.
                        settings_broken: false,
                        unknown_engine: false,
                        engine: usize::try_from(engine_index)
                            .ok()
                            .and_then(|index| Engine::ALL.get(index).copied())
                            .unwrap_or_default(),
                        allow_fallback: SendMessageW(
                            GetDlgItem(hwnd, SETTINGS_FALLBACK),
                            BM_GETCHECK,
                            0,
                            0,
                        ) == 1,
                        google_key: field(SETTINGS_GOOGLE_KEY),
                        azure_key: field(SETTINGS_AZURE_KEY),
                        azure_region: field(SETTINGS_AZURE_REGION),
                        deepl_key: field(SETTINGS_DEEPL_KEY),
                        url: field(SETTINGS_LLM_URL[0]),
                        model: field(SETTINGS_LLM_MODEL[0]),
                        key: field(SETTINGS_LLM_KEY[0]),
                        disable_thinking: SendMessageW(
                            GetDlgItem(hwnd, SETTINGS_LLM_THINK[0]),
                            BM_GETCHECK,
                            0,
                            0,
                        ) == 1,
                        backup_llm: [
                            read_llm_profile(hwnd, 1, &field),
                            read_llm_profile(hwnd, 2, &field),
                        ],
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
                        SendMessageW(GetDlgItem(hwnd, SETTINGS_HISTORY), BM_GETCHECK, 0, 0) == 1;
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
                    // entered the whole LLM configuration must be valid. A fresh install's
                    // default API URL with no model or key counts as unused.
                    let llm_blank = settings.llm_profiles().iter().all(LlmProfile::is_empty);
                    let api_check = if settings.keys_unreadable || (plain && llm_blank) {
                        Ok(())
                    } else if settings.engine != Engine::Llm && !llm_blank {
                        settings.validate().and_then(|_| settings.validate_llm())
                    } else {
                        settings.validate()
                    };
                    if parse_hotkey(&hotkey).is_none() || parse_hotkey(&history_key).is_none() {
                        message(
                            hwnd,
                            "快捷键无效：用 + 连接 Ctrl/Shift/Alt 与 Space、字母、数字或 F1～F12，且须含 Ctrl 或 Alt（F 键除外）",
                        );
                    } else if parse_hotkey(&hotkey) == parse_hotkey(&history_key)
                        || [parse_hotkey(&hotkey), parse_hotkey(&history_key)]
                            .into_iter()
                            .flatten()
                            .any(yiime_core::conflicts_with_builtin)
                    {
                        message(hwnd, "快捷键与其他输入法动作冲突");
                    } else if let Err(error) = api_check {
                        message(hwnd, error);
                    } else {
                        let saved = match save_settings_merged(
                            &settings,
                            state.dialog_base.borrow().as_ref(),
                        ) {
                            Ok(saved) => saved,
                            Err(error) => {
                                message(hwnd, &format!("保存设置失败：{error}"));
                                return 0;
                            }
                        };
                        let shuangpin = usize::try_from(SendMessageW(
                            GetDlgItem(hwnd, SETTINGS_SCHEME),
                            CB_GETCURSEL,
                            0,
                            0,
                        ))
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|index| Shuangpin::ALL.get(index).copied());
                        let scale = usize::try_from(SendMessageW(
                            GetDlgItem(hwnd, SETTINGS_UI_SCALE),
                            CB_GETCURSEL,
                            0,
                            0,
                        ))
                        .ok()
                        .and_then(|index| yiime_core::UI_SCALES.get(index).copied())
                        .unwrap_or(yiime_core::UI_SCALE);
                        let scale_ok =
                            match yiime_core::ui_scale_to_write(state.ui_scale_opened.get(), scale)
                            {
                                Some(scale) => OPTIONS.set_ui_scale(scale),
                                None => true,
                            };
                        let options_ok = set_plain_mode(plain)
                            && OPTIONS.set_plain_toggle_key(&hotkey)
                            && set_option(Flag::HistoryEnabled, history_on)
                            && OPTIONS.set_page_size(page)
                            && scale_ok
                            && OPTIONS.set_shuangpin(shuangpin)
                            && OPTIONS.set_history_key(&history_key)
                            && OPTION_BOXES.iter().all(|(id, _, option_key)| {
                                let checked =
                                    SendMessageW(GetDlgItem(hwnd, *id), BM_GETCHECK, 0, 0) == 1;
                                set_option(
                                    *option_key,
                                    checked != (*option_key == Flag::PunctEnglish),
                                )
                            });
                        if !options_ok {
                            *state.settings.borrow_mut() = saved;
                            state
                                .settings_stamp
                                .set(settings_dir().map(|dir| yiime_core::SettingsStamp::of(&dir)));
                            message(hwnd, "翻译服务设置已保存，但保存模式设置失败");
                            return 0;
                        }
                        if !history_on && !clear_history() {
                            *state.settings.borrow_mut() = saved;
                            state
                                .settings_stamp
                                .set(settings_dir().map(|dir| yiime_core::SettingsStamp::of(&dir)));
                            message(hwnd, "设置已保存，但清除输入记忆失败");
                            return 0;
                        }
                        *state.settings.borrow_mut() = saved;
                        state
                            .settings_stamp
                            .set(settings_dir().map(|dir| yiime_core::SettingsStamp::of(&dir)));
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
                        relayout(owner, state);
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
                SETTINGS_IMPORT => {
                    if let Some(state) = state_of(owner) {
                        import_dictionary(hwnd, owner, state);
                    }
                }
                SETTINGS_USER_WORDS => {
                    // Open user words in Notepad (creating the file with a short header if
                    // missing); saved changes apply from the next code typed.
                    if let Some(path) = user_words_path() {
                        if !path.exists() {
                            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
                            let _ = std::fs::write(
                                &path,
                                "# YiIME 用户词库：每行「词 编码」（空格或 Tab 分隔），排在该编码（五笔或拼音）的第一位，保存后从下一次输入起生效。\r\n# 例如：\r\n# 中英翻译 khyc\r\n",
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
                *state.dialog_base.borrow_mut() = None;
                state.settings_window.set(null_mut());
                let font = state.settings_font.replace(null_mut());
                if !font.is_null() {
                    DeleteObject(font as _);
                }
                state.settings_scroll.set(0);
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

/// Picks an exported Sogou/QQ user dictionary (.txt) or a Sogou cell dictionary (.scel) and
/// appends its words to user_words.txt; they apply from the next code typed (every YiIME process
/// reloads the file when it changes).
unsafe fn import_dictionary(hwnd: HWND, owner: HWND, state: &State) {
    if state.import_busy.get() {
        return;
    }
    let mut buffer = vec![0u16; 1024];
    let filter = wide("词库文件 (*.txt;*.scel)\0*.txt;*.scel\0所有文件\0*.*\0");
    let title = wide("导入搜狗/QQ 用户词库或搜狗细胞词库");
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = hwnd;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrFile = buffer.as_mut_ptr();
    dialog.nMaxFile = buffer.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        return;
    }
    let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len]));
    let Some(target) = user_words_path() else {
        return;
    };
    state.import_busy.set(true);
    unsafe {
        let btn = GetDlgItem(hwnd, SETTINGS_IMPORT);
        if !btn.is_null() {
            EnableWindow(btn, 0);
        }
    }
    let tx = state.import_tx.clone();
    let owner_w = owner as usize;
    std::thread::spawn(move || {
        let result = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| yiime_core::import_words(&bytes).map_err(str::to_owned))
            .and_then(|words| {
                yiime_core::import_into(&target, &words)
                    .map(|added| (added, words.len()))
                    .map_err(|e| e.to_string())
            });
        if tx.send(result).is_ok() {
            unsafe { PostMessageW(owner_w as HWND, WM_IMPORT_DONE, 0, 0) };
        }
    });
}
