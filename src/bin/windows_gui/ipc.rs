use super::*;

/// Ends the session of `receiver` in the input box (`editor`) once its owner process `pid`
/// exits (WM_TSF_EXIT with the receiver as wParam and the pid as lParam, so the box can tell a
/// stale notice for a reused window handle from its current host).
pub(super) fn watch_tsf_owner(receiver: HWND, pid: u32, editor: HWND) {
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
        PostMessageW(editor as HWND, WM_TSF_EXIT, receiver, pid as isize);
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
/// Commits `text` into the text field the DLL parked under `id` (COPYDATA dwData 5). Empty text
/// only releases that field. A session that started afterwards is left running.
pub(super) unsafe fn send_tsf_held(receiver: HWND, sender: HWND, id: u64, text: &str) -> bool {
    let bytes = id.to_le_bytes();
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    units.extend(text.encode_utf16());
    let packet = COPYDATASTRUCT {
        dwData: 5,
        cbData: (units.len() * 2) as u32,
        lpData: units.as_mut_ptr() as _,
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
