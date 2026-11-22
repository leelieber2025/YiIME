use super::*;

/// Ends the session of `receiver` in the input box (`editor`) once the application owning that
/// receiver exits (WM_TSF_EXIT with the receiver as wParam).
pub(super) fn watch_tsf_owner(receiver: HWND, editor: HWND) {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(receiver, &mut pid);
    }
    if pid == 0 {
        return;
    }
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return;
    }
    let process = process as usize;
    let editor = editor as usize;
    let receiver = receiver as usize;
    std::thread::spawn(move || unsafe {
        WaitForSingleObject(process as _, u32::MAX);
        CloseHandle(process as _);
        PostMessageW(editor as HWND, WM_TSF_EXIT, receiver, 0);
    });
}
pub(super) unsafe fn send_tsf_text(receiver: HWND, sender: HWND, text: &str) -> bool {
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    let packet = COPYDATASTRUCT {
        dwData: 0,
        cbData: (utf16.len() * 2) as u32,
        lpData: utf16.as_mut_ptr() as _,
    };
    let mut accepted = 0usize;
    unsafe {
        SendMessageTimeoutW(
            receiver,
            WM_COPYDATA,
            sender as usize,
            &packet as *const _ as isize,
            SMTO_ABORTIFHUNG,
            3000,
            &mut accepted,
        ) != 0
            && accepted != 0
    }
}
pub(super) unsafe fn notify_tsf_ready(receiver: HWND, sender: HWND) {
    let packet = COPYDATASTRUCT {
        dwData: 1,
        cbData: 0,
        lpData: null_mut(),
    };
    for _ in 0..20 {
        let mut accepted = 0usize;
        if unsafe {
            SendMessageTimeoutW(
                receiver,
                WM_COPYDATA,
                sender as usize,
                &packet as *const _ as isize,
                SMTO_ABORTIFHUNG,
                200,
                &mut accepted,
            )
        } != 0
            && accepted != 0
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
