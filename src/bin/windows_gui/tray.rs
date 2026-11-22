use super::*;

fn tray_tip() -> String {
    if plain_mode() {
        format!(
            "{APP_NAME} {} · 普通五笔模式（不翻译）",
            env!("CARGO_PKG_VERSION")
        )
    } else {
        format!(
            "{APP_NAME} {} · 翻译模式（单击或右键：菜单）",
            env!("CARGO_PKG_VERSION")
        )
    }
}
pub(super) unsafe fn update_tray_tip(hwnd: HWND) {
    let mut data = unsafe { tray_data(hwnd) };
    data.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_wide(&mut data.szTip, &tray_tip());
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn copy_wide(target: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
    target[..units.len()].copy_from_slice(&units);
    target[units.len()] = 0;
}
pub(super) unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data
}
pub(super) unsafe fn add_tray(hwnd: HWND, balloon: Option<&str>) {
    let mut data = unsafe { tray_data(hwnd) };
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = unsafe { app_icon(GetSystemMetrics(SM_CXSMICON)) };
    copy_wide(&mut data.szTip, &tray_tip());
    if let Some(text) = balloon {
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_wide(&mut data.szInfoTitle, APP_NAME);
        copy_wide(&mut data.szInfo, text);
    }
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &data);
        // Version 4: right-click sends WM_CONTEXTMENU, left-click NIN_SELECT, with the message
        // in the low 16 bits of lParam.
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        TRAY_V4.store(
            Shell_NotifyIconW(NIM_SETVERSION, &data) != 0,
            Ordering::Relaxed,
        );
    }
}
pub(super) unsafe fn tray_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        let checked = if plain_mode() {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        // The same shortcut toggles both ways.
        let toggle = plain_toggle_text();
        let translate_checked = if plain_mode() {
            MF_UNCHECKED
        } else {
            MF_CHECKED
        };
        AppendMenuW(
            menu,
            MF_STRING | translate_checked,
            TRAY_TRANSLATE,
            wide(&format!("翻译模式（译）\t{toggle}")).as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | checked,
            TRAY_PLAIN,
            wide(&format!("普通五笔模式（中，不翻译）\t{toggle}")).as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING,
            TRAY_HISTORY,
            wide(&format!("输入记忆…\t{}", history_key_text())).as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, TRAY_SETTINGS, wide("设置…").as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, TRAY_EXIT, wide("退出").as_ptr());
        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        match command as usize {
            TRAY_SETTINGS => open_settings(hwnd),
            TRAY_HISTORY => open_history(),
            TRAY_PLAIN | TRAY_TRANSLATE => {
                set_plain_mode(command as usize == TRAY_PLAIN);
                if let Some(state) = state_of(hwnd) {
                    state.last_plain.set(plain_mode());
                }
                update_tray_tip(hwnd);
            }
            TRAY_EXIT => {
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}
