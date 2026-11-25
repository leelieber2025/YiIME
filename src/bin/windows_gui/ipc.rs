use super::*;

/// Ends the session of `receiver` in the input box (`editor`) once its owner process `pid`
/// exits (`WAIT_OBJECT_0`, posting `WM_TSF_EXIT` with receiver as `wParam` and `pid` as `lParam`).
/// Returns `false` synchronously if `OpenProcess` fails; if `WaitForSingleObject` returns anything
/// other than `WAIT_OBJECT_0` (e.g. `WAIT_FAILED`), posts `WM_TSF_WATCH_FAILED` instead of
/// treating a live host as exited.
pub(super) fn watch_tsf_owner(receiver: HWND, pid: u32, editor: HWND) -> bool {
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return false;
    }
    let process = process as usize;
    let editor = editor as usize;
    let receiver = receiver as usize;
    std::thread::spawn(move || unsafe {
        let wait = WaitForSingleObject(process as _, u32::MAX);
        CloseHandle(process as _);
        if wait == 0 {
            PostMessageW(editor as HWND, WM_TSF_EXIT, receiver, pid as isize);
        } else {
            PostMessageW(editor as HWND, WM_TSF_WATCH_FAILED, receiver, pid as isize);
        }
    });
    true
}
pub(super) unsafe fn send_tsf_text(receiver: HWND, sender: HWND, text: &str) -> bool {
    matches!(
        unsafe { send_commit(receiver, sender, text, 0) },
        Some(1 | 2)
    )
}
/// Protocol 4. None means no reply; 0 rejected, 1 inserted, 2 queued.
pub(super) unsafe fn send_commit(
    receiver: HWND,
    sender: HWND,
    text: &str,
    id: u32,
) -> Option<usize> {
    let mut units = vec![id as u16, (id >> 16) as u16];
    units.extend(text.encode_utf16());
    let packet = COPYDATASTRUCT {
        dwData: 6,
        cbData: (units.len() * 2) as u32,
        lpData: units.as_mut_ptr() as _,
    };
    let mut result = 0;
    (unsafe {
        SendMessageTimeoutW(
            receiver,
            WM_COPYDATA,
            sender as usize,
            &packet as *const _ as isize,
            SMTO_ABORTIFHUNG,
            3000,
            &mut result,
        )
    } != 0)
        .then_some(result)
}
/// Commits `text` into the text field the DLL parked under `id` (COPYDATA dwData 5, carrying
/// `[u64 held_id][u32 request][utf16 text]`). Empty text only releases that field. Returns
/// `None` on no reply, or `Some(0|1|2)` (rejected / landed / queued).
pub(super) unsafe fn send_tsf_held(
    receiver: HWND,
    sender: HWND,
    id: u64,
    request: u32,
    text: &str,
) -> Option<usize> {
    let bytes = id.to_le_bytes();
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    units.push(request as u16);
    units.push((request >> 16) as u16);
    units.extend(text.encode_utf16());
    let packet = COPYDATASTRUCT {
        dwData: 5,
        cbData: (units.len() * 2) as u32,
        lpData: units.as_mut_ptr() as _,
    };
    let mut result = 0usize;
    (unsafe {
        SendMessageTimeoutW(
            receiver,
            WM_COPYDATA,
            sender as usize,
            &packet as *const _ as isize,
            SMTO_ABORTIFHUNG,
            3000,
            &mut result,
        )
    } != 0)
        .then_some(result)
}
/// Asks the TSF DLL to switch its application to English input (COPYDATA dwData 4), as a Shift
/// tap there would.
pub(super) unsafe fn send_tsf_english(receiver: HWND, sender: HWND) -> bool {
    let packet = COPYDATASTRUCT {
        dwData: 4,
        cbData: 0,
        lpData: null_mut(),
    };
    let mut accepted = 0usize;
    unsafe {
        SendMessageTimeoutW(
            receiver,
            WM_COPYDATA,
            sender as usize,
            &packet as *const _ as isize,
            SMTO_ABORTIFHUNG,
            1000,
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
