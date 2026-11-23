use super::*;

thread_local! {
    /// History window and the entries behind its list (the tray process is single-threaded).
    static HISTORY_WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
    static HISTORY_VIEW: RefCell<Vec<HistoryEntry>> = const { RefCell::new(Vec::new()) };
}
unsafe fn fill_history(window: HWND) {
    let entries = load_history();
    unsafe {
        let list = GetDlgItem(window, HISTORY_LIST);
        SendMessageW(list, LB_RESETCONTENT, 0, 0);
        for entry in &entries {
            SendMessageW(
                list,
                LB_ADDSTRING,
                0,
                wide(&history_label(entry)).as_ptr() as isize,
            );
        }
        if !entries.is_empty() {
            SendMessageW(list, LB_SETCURSEL, 0, 0);
        }
        set_text(
            GetDlgItem(window, HISTORY_STATUS),
            &if !option(Flag::HistoryEnabled) {
                "输入记忆已关闭（设置里可打开）".to_owned()
            } else if entries.is_empty() {
                "还没有输入记忆：翻译上屏后会自动记住。".to_owned()
            } else {
                format!(
                    "共 {} 条（最多 {HISTORY_CAP} 条，仅存本机、DPAPI 加密）。“再次使用”后在应用里按 {} 即可找回编辑。",
                    entries.len(),
                    history_key_text()
                )
            },
        );
    }
    HISTORY_VIEW.with(|view| *view.borrow_mut() = entries);
}
/// Tray menu "输入记忆…": lists recent translations; copy the Chinese or English, or queue one as
/// the next recall.
pub(super) unsafe fn open_history() {
    let existing = HISTORY_WINDOW.with(Cell::get);
    if !existing.is_null() {
        unsafe {
            fill_history(existing);
            ShowWindow(existing, SW_SHOWNORMAL);
            SetForegroundWindow(existing);
        }
        return;
    }
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let ex_style = WS_EX_TOPMOST | WS_EX_APPWINDOW;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 640,
        bottom: 380,
    };
    unsafe {
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);
    }
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let window = unsafe {
        CreateWindowExW(
            ex_style,
            wide(HISTORY_CLASS).as_ptr(),
            wide("YiIME 译 · 输入记忆").as_ptr(),
            style,
            (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - height) / 3,
            width,
            height,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        )
    };
    if window.is_null() {
        return;
    }
    HISTORY_WINDOW.with(|cell| cell.set(window));
    unsafe {
        fill_history(window);
        ShowWindow(window, SW_SHOWNORMAL);
        SetForegroundWindow(window);
    }
}
pub(super) unsafe extern "system" fn history_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let selected = |hwnd: HWND| -> Option<HistoryEntry> {
        let index = unsafe { SendMessageW(GetDlgItem(hwnd, HISTORY_LIST), LB_GETCURSEL, 0, 0) };
        let index = usize::try_from(index).ok()?;
        HISTORY_VIEW.with(|view| view.borrow().get(index).cloned())
    };
    match msg {
        WM_CREATE => unsafe {
            control(
                hwnd,
                "LISTBOX",
                "",
                (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32 | WS_VSCROLL | WS_BORDER | WS_TABSTOP,
                12,
                12,
                616,
                282,
                HISTORY_LIST,
            );
            for (index, (id, text)) in [
                (HISTORY_COPY_ZH, "复制中文"),
                (HISTORY_COPY_EN, "复制译文"),
                (HISTORY_REUSE, "再次使用"),
                (HISTORY_CLOSE, "关闭"),
            ]
            .into_iter()
            .enumerate()
            {
                control(
                    hwnd,
                    "BUTTON",
                    text,
                    WS_TABSTOP,
                    12 + index as i32 * 100,
                    304,
                    92,
                    28,
                    id,
                );
            }
            control(hwnd, "STATIC", "", 0, 12, 342, 616, 32, HISTORY_STATUS);
            0
        },
        WM_COMMAND => unsafe {
            let id = (w & 0xffff) as i32;
            let notification = ((w >> 16) & 0xffff) as u32;
            let status = GetDlgItem(hwnd, HISTORY_STATUS);
            match id {
                HISTORY_COPY_ZH | HISTORY_COPY_EN => {
                    if let Some(entry) = selected(hwnd) {
                        let text = if id == HISTORY_COPY_ZH {
                            &entry.chinese
                        } else {
                            &entry.english
                        };
                        set_text(
                            status,
                            if set_clipboard(hwnd, text) {
                                "已复制到剪贴板。"
                            } else {
                                "复制失败。"
                            },
                        );
                    }
                }
                HISTORY_REUSE => {
                    reuse_history(hwnd, selected(hwnd));
                }
                HISTORY_LIST if notification == LBN_DBLCLK => {
                    reuse_history(hwnd, selected(hwnd));
                }
                HISTORY_CLOSE => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        },
        WM_CLOSE => unsafe {
            DestroyWindow(hwnd);
            0
        },
        WM_DESTROY => {
            HISTORY_WINDOW.with(|cell| cell.set(null_mut()));
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
/// "再用一次": move the entry to the front (the next recall loads it) and copy its Chinese.
unsafe fn reuse_history(window: HWND, entry: Option<HistoryEntry>) {
    let Some(entry) = entry else {
        return;
    };
    let mut entries = load_history();
    entries.retain(|item| item.chinese != entry.chinese);
    entries.insert(0, entry.clone());
    let saved = option(Flag::HistoryEnabled) && save_history(&entries);
    unsafe {
        let copied = set_clipboard(window, &entry.chinese);
        fill_history(window);
        set_text(
            GetDlgItem(window, HISTORY_STATUS),
            &format!(
                "{}{}",
                if saved {
                    format!(
                        "已设为下一次找回的草稿：回到应用按 {} 即可编辑、重新翻译。",
                        history_key_text()
                    )
                } else {
                    String::new()
                },
                if copied { "中文已复制。" } else { "" }
            ),
        );
    }
}
