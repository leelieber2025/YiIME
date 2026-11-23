use super::*;

pub(super) unsafe fn create_font(height: i32) -> HFONT {
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

/// Match the inline candidate bar: DWM small rounded corners, or a region fallback.
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
        let region = unsafe { CreateRoundRectRgn(0, 0, width, height, 8, 8) };
        if !region.is_null() {
            unsafe {
                SetWindowRgn(hwnd, region, 1);
            }
        }
    }
}
fn fill_round(hdc: HDC, rect: RECT, color: u32, radius: i32) {
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
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush as _);
        DeleteObject(pen as _);
    }
}
fn stroke_border(hdc: HDC, client: RECT, color: u32, radius: i32) {
    let pen = unsafe { CreatePen(PS_SOLID, 1, color) };
    let old_pen = unsafe { SelectObject(hdc, pen as _) };
    let old_brush = unsafe { SelectObject(hdc, GetStockObject(NULL_BRUSH)) };
    unsafe {
        RoundRect(
            hdc,
            client.left,
            client.top,
            client.right,
            client.bottom,
            radius,
            radius,
        );
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(pen as _);
    }
}

/// Resizes and repositions the box for the draft, code and candidates.
pub(super) unsafe fn relayout(hwnd: HWND, state: &State) {
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
                    format!("{} {}", index + 1, candidate.text),
                    composition.hint(candidate),
                )
            })
            .collect();
        for (label, hint) in labels {
            let mut w = unsafe { text_width(hdc, &label) };
            if !hint.is_empty() {
                unsafe {
                    SelectObject(hdc, state.small_font as _);
                    w += text_width(hdc, &hint) + 2;
                    SelectObject(hdc, state.font as _);
                }
            }
            composition.hits.push((x, x + w));
            x += w + GAP;
        }
        width = width.max(x + PAD + 4);
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
        apply_window_shape(hwnd, width, height);
        MoveWindow(state.edit, PAD, PAD, edit_w, draft_h, 1);
        // Keep the caret visible (scroll to it when the draft exceeds the maximum lines).
        SendMessageW(state.edit, EM_SCROLLCARET, 0, 0);
        InvalidateRect(hwnd, null(), 1);
    }
}
pub(super) unsafe fn paint(hwnd: HWND, state: &State) {
    let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut client);
        let bg = CreateSolidBrush(BACKGROUND);
        FillRect(hdc, &client, bg);
        DeleteObject(bg as _);
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
                let rect = RECT {
                    left: left - 4,
                    top: row,
                    right: right + 4,
                    bottom: row + LINE_H - 2,
                };
                unsafe {
                    fill_round(hdc, rect, HIGHLIGHT, 4);
                    let accent = CreateSolidBrush(ACCENT);
                    let bar = RECT {
                        left: rect.left + 1,
                        top: rect.top + 3,
                        right: rect.left + 3,
                        bottom: rect.bottom - 3,
                    };
                    FillRect(hdc, &bar, accent);
                    DeleteObject(accent as _);
                }
            }
            let color = if candidate.complete { INK } else { MUTED };
            let label = format!("{} {}", index + 1, candidate.text);
            unsafe {
                draw_text(hdc, left, row + 2, color, &label);
            }
            let hint = composition.hint(candidate);
            if !hint.is_empty() {
                unsafe {
                    let x = left + text_width(hdc, &label) + 2;
                    let previous = SelectObject(hdc, state.small_font as _);
                    draw_text(hdc, x, row + 5, MUTED, &hint);
                    SelectObject(hdc, previous);
                }
            }
        }
        if composition.page_count() > 1 {
            unsafe {
                SelectObject(hdc, state.small_font as _);
                let label = composition.page_label();
                let w = text_width(hdc, &label);
                draw_text(hdc, client.right - PAD - w, top + 5, MUTED, &label);
            }
        }
    } else {
        // Enter translates, Shift+Enter commits Chinese; errors stay on this red status line.
        let status = state.status.borrow();
        let (text, color) = match status.as_ref() {
            Some((text, true)) => (text.as_str(), RED),
            Some((text, false)) => (text.as_str(), BLUE),
            None => (hint(state), MUTED),
        };
        unsafe {
            SelectObject(hdc, state.small_font as _);
            draw_text(hdc, PAD, top + 5, color, text);
        }
    }
    unsafe {
        SelectObject(hdc, old);
        stroke_border(hdc, client, BORDER, 8);
        EndPaint(hwnd, &paint);
    }
}
