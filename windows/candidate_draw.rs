//! Shared drawing for the draft-box candidate row and the in-document candidate bar.
//! Colors, the YaHei font, DPI scaling and the rounded highlight are one copy so the two
//! windows stay the same. Each window still places its own controls.

use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, GetStockObject, GetTextExtentPoint32W, HFONT,
    NULL_BRUSH, OUT_DEFAULT_PRECIS, PS_SOLID, RoundRect, SelectObject, SetTextColor, SetWindowRgn,
    TextOutW,
};
use windows_sys::Win32::Graphics::Gdi::{HDC, HGDIOBJ};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};

// COLORREF is 0x00BBGGRR. Compact Win11 Wubi palette.
// Each window uses part of this palette, so the other inclusion would warn.
#[allow(dead_code)]
pub const BLUE: u32 = 0x00C0_6000;
pub const MUTED: u32 = 0x0070_7070;
#[allow(dead_code)]
pub const RED: u32 = 0x0000_00C0;
pub const INK: u32 = 0x0020_2020;
#[allow(dead_code)]
pub const GRAY: u32 = 0x0080_8080;
#[allow(dead_code)]
pub const DISABLED: u32 = 0x00B0_B0B0;
pub const HIGHLIGHT: u32 = 0x00EB_EBEB;
pub const ACCENT: u32 = 0x005C_5C5C;
pub const BACKGROUND: u32 = 0x00F7_F7F7;
pub const BORDER: u32 = 0x00D8_D8D8;
#[allow(dead_code)]
pub const SEPARATOR: u32 = 0x00E0_E0E0;
#[allow(dead_code)]
pub const DIVIDER: u32 = SEPARATOR;

pub fn window_dpi(hwnd: HWND) -> u32 {
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

pub fn scale_px(px: i32, dpi: u32) -> i32 {
    (i64::from(px) * i64::from(dpi) / 96).max(1) as i32
}

pub unsafe fn create_font(height: i32) -> HFONT {
    let face: Vec<u16> = "Microsoft YaHei UI".encode_utf16().chain(Some(0)).collect();
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
            face.as_ptr(),
        )
    }
}

pub unsafe fn text_width(hdc: HDC, text: &str) -> i32 {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe {
        GetTextExtentPoint32W(hdc, units.as_ptr(), units.len() as i32, &mut size);
    }
    size.cx
}

pub unsafe fn draw_text(hdc: HDC, x: i32, y: i32, color: u32, text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        SetTextColor(hdc, color);
        TextOutW(hdc, x, y, units.as_ptr(), units.len() as i32);
    }
}

pub fn fill_round(hdc: HDC, rect: RECT, color: u32, radius: i32) {
    let brush = unsafe { CreateSolidBrush(color) };
    let pen = unsafe { CreatePen(PS_SOLID, 1, color) };
    let old_brush = unsafe { SelectObject(hdc, brush as HGDIOBJ) };
    let old_pen = unsafe { SelectObject(hdc, pen as HGDIOBJ) };
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
        DeleteObject(brush as HGDIOBJ);
        DeleteObject(pen as HGDIOBJ);
    }
}

pub fn stroke_border(hdc: HDC, client: RECT, color: u32, radius: i32) {
    let pen = unsafe { CreatePen(PS_SOLID, 1, color) };
    let old_pen = unsafe { SelectObject(hdc, pen as HGDIOBJ) };
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
        DeleteObject(pen as HGDIOBJ);
    }
}

pub fn apply_window_shape(hwnd: HWND, width: i32, height: i32) {
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
            unsafe {
                SetWindowRgn(hwnd, region, 1);
            }
        }
    }
}
