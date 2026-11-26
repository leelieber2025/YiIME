use super::candidate_draw::{apply_window_shape, draw_text, fill_round, stroke_border, text_width};
use super::*;

pub(super) use super::candidate_draw::{create_font, scale_px, window_dpi};

/// Monitor DPI times the composition-window scale. 100% is the monitor DPI.
fn box_dpi(hwnd: HWND) -> u32 {
    yiime_core::scaled_dpi(window_dpi(hwnd), OPTIONS.ui_scale())
}

/// Pixel length that stays the same at 100% and grows with the composition-window scale.
fn zoom(px: i32) -> i32 {
    (i64::from(px) * i64::from(OPTIONS.ui_scale()) / 100) as i32
}

/// Recreate the draft fonts when the window's DPI or the composition-window scale changes.
pub(super) unsafe fn ensure_fonts(hwnd: HWND, state: &State) -> u32 {
    let dpi = box_dpi(hwnd);
    if dpi == state.font_dpi.get() && !state.font.get().is_null() {
        return dpi;
    }
    let font = unsafe { create_font(scale_px(14, dpi)) };
    let small = unsafe { create_font(scale_px(11, dpi)) };
    let previous = state.font.replace(font);
    let previous_small = state.small_font.replace(small);
    if !previous.is_null() {
        unsafe { DeleteObject(previous as _) };
    }
    if !previous_small.is_null() {
        unsafe { DeleteObject(previous_small as _) };
    }
    state.font_dpi.set(dpi);
    if !state.edit.is_null() {
        unsafe { SendMessageW(state.edit, WM_SETFONT, font as usize, 1) };
    }
    dpi
}
pub(super) fn scaled_line_h(state: &State) -> i32 {
    scale_px(LINE_H, state.font_dpi.get().max(96))
}

/// Below the cursor, or above it if it does not fit; always inside the monitor's work area.
unsafe fn place(anchor: RECT, width: i32, height: i32, dpi: u32) -> (i32, i32) {
    let work = unsafe { work_area(anchor) };
    let gap = scale_px(4, dpi);
    let mut y = anchor.bottom + gap;
    if y + height > work.bottom {
        y = anchor.top - height - gap;
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
pub(super) unsafe fn relayout(hwnd: HWND, state: &State) {
    let dpi = unsafe { ensure_fonts(hwnd, state) };
    let pad = scale_px(PAD, dpi);
    let gap = scale_px(GAP, dpi);
    let line_h = scale_px(LINE_H, dpi);
    let edit_h = scale_px(EDIT_H, dpi);
    let min_w = scale_px(MIN_W, dpi);
    let draft = unsafe { get_text(state.edit) };
    let status = state.status.borrow().clone();
    let mut composition = state.composition.borrow_mut();
    let composing = !composition.code.is_empty();
    let hdc = unsafe { GetDC(hwnd) };
    let old = unsafe { SelectObject(hdc, state.font.get() as _) };
    let mut width =
        (unsafe { text_width(hdc, &draft) } + zoom(24)).min(zoom(DRAFT_MAX_W)) + 2 * pad;
    composition.hits.clear();
    if composing {
        let mut x = pad;
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
                    SelectObject(hdc, state.small_font.get() as _);
                    w += text_width(hdc, &hint) + 2;
                    SelectObject(hdc, state.font.get() as _);
                }
            }
            composition.hits.push((x, x + w, 0));
            x += w + gap;
        }
        width = width.max(x + pad + 4);
        let code = unsafe { text_width(hdc, &composition.code) };
        unsafe {
            SelectObject(hdc, state.small_font.get() as _);
        }
        let pages = unsafe { text_width(hdc, &composition.page_label()) };
        width = width.max(code + pages + 3 * pad + gap);
    } else {
        unsafe {
            SelectObject(hdc, state.small_font.get() as _);
        }
        let line = status.map_or(hint(state).to_owned(), |(text, _)| text);
        width = width.max(unsafe { text_width(hdc, &line) } + 2 * pad);
    }
    // Wubi hints can make the row wider than MAX_W: allow up to the work-area width so the 8th
    // and 9th candidates are not cut off.
    let max_w = unsafe {
        let work = work_area(state.anchor.get());
        (work.right - work.left - 16).max(1)
    };
    let width = width.clamp(min_w.min(max_w), max_w) + 2;
    // Multi-line draft: wrap at the edit-box width to compute its height (excluding the edit
    // control's padding).
    let edit_w = (width - 2 - 2 * pad).max(1);
    let mut x = pad;
    let mut candidate_line = 0;
    for hit in &mut composition.hits {
        let item_width = (hit.1 - hit.0).min(edit_w);
        if x != pad && x + item_width > width - pad {
            candidate_line += 1;
            x = pad;
        }
        *hit = (x, x + item_width, candidate_line);
        x += item_width + gap;
    }
    let draft_h = unsafe {
        SelectObject(hdc, state.font.get() as _);
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
            right: edit_w - zoom(10),
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
        (lines * line + 4).max(edit_h)
    };
    unsafe {
        SelectObject(hdc, old);
        ReleaseDC(hwnd, hdc);
    }
    state.draft_h.set(draft_h);
    let lines = if composing { 2 + candidate_line } else { 1 };
    let height = pad + draft_h + 3 + lines * line_h + pad + 2;
    composition.row_top = pad + draft_h + 3 + line_h;
    drop(composition);
    let (x, y) = unsafe { place(state.anchor.get(), width, height, dpi) };
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
        apply_window_shape(hwnd, width, height);
        MoveWindow(state.edit, pad, pad, edit_w, draft_h, 1);
        // Keep the caret visible (scroll to it when the draft exceeds the maximum lines).
        SendMessageW(state.edit, EM_SCROLLCARET, 0, 0);
        InvalidateRect(hwnd, null(), 1);
    }
}
pub(super) unsafe fn paint(hwnd: HWND, state: &State) {
    let dpi = box_dpi(hwnd);
    let pad = scale_px(PAD, dpi);
    let line_h = scale_px(LINE_H, dpi);
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
            left: pad,
            top: pad + state.draft_h.get() + 1,
            right: client.right - pad,
            bottom: pad + state.draft_h.get() + 2,
        };
        FillRect(hdc, &line, separator);
        DeleteObject(separator as _);
        SetBkMode(hdc, TRANSPARENT as _);
    }
    let top = pad + state.draft_h.get() + 3;
    let composition = state.composition.borrow();
    let old = unsafe { SelectObject(hdc, state.font.get() as _) };
    if !composition.code.is_empty() {
        unsafe {
            draw_text(hdc, pad, top + 2, BLUE, &composition.code);
        }
        for (index, (candidate, (left, right, line))) in composition
            .page_items()
            .iter()
            .zip(composition.hits.iter().copied())
            .enumerate()
        {
            let row = composition.row_top + line * line_h;
            if index == 0 {
                let rect = RECT {
                    left: left - 4,
                    top: row,
                    right: right + 4,
                    bottom: row + line_h - 2,
                };
                unsafe {
                    fill_round(hdc, rect, HIGHLIGHT, scale_px(4, dpi));
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
                    let previous = SelectObject(hdc, state.small_font.get() as _);
                    draw_text(hdc, x, row + 5, MUTED, &hint);
                    SelectObject(hdc, previous);
                }
            }
        }
        if composition.page_count() > 1 {
            unsafe {
                SelectObject(hdc, state.small_font.get() as _);
                let label = composition.page_label();
                let w = text_width(hdc, &label);
                draw_text(hdc, client.right - pad - w, top + 5, MUTED, &label);
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
            SelectObject(hdc, state.small_font.get() as _);
            draw_text(hdc, pad, top + 5, color, text);
        }
    }
    unsafe {
        SelectObject(hdc, old);
        stroke_border(hdc, client, BORDER, scale_px(8, dpi));
        EndPaint(hwnd, &paint);
    }
}
