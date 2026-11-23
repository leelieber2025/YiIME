use super::*;

pub(super) unsafe fn create_receiver(service: *mut c_void) -> HWND {
    let instance = MODULE.load(Ordering::SeqCst) as _;
    let class = wide(WINDOW_CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(receiver_proc),
        hInstance: instance,
        lpszClassName: class.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&wc) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return null_mut();
    }
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide("").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            service,
        )
    }
}
unsafe extern "system" fn receiver_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match std::panic::catch_unwind(|| unsafe { receiver_proc_impl(hwnd, message, w, l) }) {
        Ok(result) => result,
        Err(_) => {
            let _ = std::panic::catch_unwind(|| log("panic in receiver_proc"));
            0
        }
    }
}

unsafe fn receiver_proc_impl(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_CREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as _);
        }
        return 0;
    }
    if message == WM_COPYDATA {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if pointer.is_null() || l == 0 {
            return 0;
        }
        // Only the editor service this instance started, found, or is waiting to bind
        // after a ShellExecuteW launch (see bind_editor_sender).
        if !unsafe { (*pointer).bind_editor_sender(w as HWND) } {
            return 0;
        }
        let packet = unsafe { &*(l as *const COPYDATASTRUCT) };
        if packet.dwData == 1 {
            let ready = unsafe { (*pointer).editor_ready(w as HWND) };
            if ready {
                unsafe {
                    PostMessageW(hwnd, WM_APP + 1, 0, 0);
                }
            }
            return ready as LRESULT;
        }
        // The input box was empty and the user tapped Shift: switch to English like a Shift tap
        // in the application (the box has already closed).
        if packet.dwData == 4 {
            return unsafe { (*pointer).switch_to_english() } as LRESULT;
        }
        // Chinese source of a translation whose text field was left (u64 id, then UTF-16 text).
        // Empty text only releases that field. A newer session is not finished.
        if packet.dwData == 5 {
            if packet.lpData.is_null()
                || (packet.cbData as usize) < 8
                || !(packet.cbData - 8).is_multiple_of(2)
            {
                return 0;
            }
            let id = unsafe { std::ptr::read_unaligned(packet.lpData as *const u64) };
            let units = unsafe {
                std::slice::from_raw_parts(
                    packet.lpData.cast::<u8>().add(8).cast::<u16>(),
                    (packet.cbData as usize - 8) / 2,
                )
            };
            let Ok(text) = String::from_utf16(units) else {
                return 0;
            };
            return unsafe { (*pointer).commit_held(id, text) } as LRESULT;
        }
        if packet.dwData != 0 {
            return 0;
        }
        if packet.cbData == 0 {
            return unsafe { (*pointer).commit(String::new()) as LRESULT };
        }
        if packet.cbData as usize > 1_000_000
            || !packet.cbData.is_multiple_of(2)
            || packet.lpData.is_null()
        {
            return 0;
        }
        let units = unsafe {
            std::slice::from_raw_parts(packet.lpData as *const u16, packet.cbData as usize / 2)
        };
        let Ok(text) = String::from_utf16(units) else {
            return 0;
        };
        if unsafe { (*pointer).waiting_for_editor() } {
            let _ = unsafe { (*pointer).editor_ready(w as HWND) };
        }
        return unsafe { (*pointer).commit(text) as LRESULT };
    }
    if message == WM_SCOPE_READ || message == WM_CARET_READ {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if !pointer.is_null() {
            if message == WM_SCOPE_READ {
                unsafe { (*pointer).scope_arrived() };
            } else {
                unsafe { (*pointer).caret_arrived() };
            }
        }
        return 0;
    }
    if message == WM_TRANSLATE_MODE {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if !pointer.is_null() {
            unsafe { (*pointer).apply_plain_mode(w != 0) };
        }
        return 0;
    }
    if message == WM_APP + 9 {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if !pointer.is_null() {
            unsafe { (*pointer).notify_box_english() };
        }
        return 0;
    }
    if message == WM_APP + 1 {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if !pointer.is_null() && unsafe { !(*pointer).flush_keys() } {
            unsafe {
                SetTimer(hwnd, FLUSH_TIMER, 100, None);
            }
        }
        return 0;
    }
    if message == WM_TIMER && w == READY_TIMER {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        // Polled while the editor starts: if it exited or never became ready, end the session,
        // give the buffered letters (first one included) back to the host and return to normal.
        let waiting = !pointer.is_null()
            && unsafe {
                let service = &*pointer;
                service.check_startup();
                service.waiting_for_editor()
            };
        if !waiting {
            unsafe {
                KillTimer(hwnd, READY_TIMER);
            }
        }
        return 0;
    }
    if message == WM_TIMER && w == FLUSH_TIMER {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if pointer.is_null() || unsafe { (*pointer).flush_keys() } {
            unsafe {
                KillTimer(hwnd, FLUSH_TIMER);
            }
        } else if unsafe { (&*pointer).flush_attempts.get() } >= 10 {
            unsafe {
                KillTimer(hwnd, FLUSH_TIMER);
                {
                    let mut editor = (&*pointer).editor.borrow_mut();
                    if let Some(child) = editor.as_mut() {
                        let _ = child.kill();
                    }
                }
                (*pointer).restore_pending_keys();
                (*pointer).reset_editor();
            }
        } else {
            unsafe {
                let service = &*pointer;
                service.flush_attempts.set(service.flush_attempts.get() + 1);
            }
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
