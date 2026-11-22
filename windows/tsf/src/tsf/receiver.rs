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
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return 0;
    }
    if message == WM_COPYDATA {
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        if pointer.is_null() || l == 0 {
            return 0;
        }
        let mut sender_pid = 0;
        unsafe {
            GetWindowThreadProcessId(w as HWND, &mut sender_pid);
        }
        // Only the editor service this instance started or verified (see find_editor_service).
        if sender_pid == 0 || unsafe { (&(*pointer)).editor_pid.get() } != sender_pid {
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
        return unsafe { (*pointer).commit(text) as LRESULT };
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
        unsafe {
            KillTimer(hwnd, READY_TIMER);
        }
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextService_Impl };
        // The editor never became ready: end the session, give the buffered letters back to the
        // host and return to normal.
        if !pointer.is_null()
            && unsafe { (&*pointer).editor_open.get() && (&*pointer).editor_window.get().is_null() }
        {
            unsafe {
                // A shared service that never answers is restarted on the next activation.
                let service = &*pointer;
                service
                    .health
                    .borrow_mut()
                    .timed_out(service.editor_pid.get());
                OutputDebugStringW(wide("YiIME: 编辑器就绪超时，已归还缓存按键").as_ptr());
                {
                    let mut editor = (&*pointer).editor.borrow_mut();
                    if let Some(child) = editor.as_mut() {
                        let _ = child.kill();
                    }
                }
                (*pointer).restore_pending_keys();
                (*pointer).reset_editor();
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
