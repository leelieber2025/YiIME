//! Reads a process's elevation, integrity level, session and user (shared by the TSF DLL and the
//! input-box EXE via `#[path]`). Any failure gives `None`, which callers treat as untrusted.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
    TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenIntegrityLevel, TokenUser,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Security context of process `pid` (0 = this process).
pub fn process_security(pid: u32) -> Option<yiime_core::ProcessSecurity> {
    let pid = if pid == 0 { std::process::id() } else { pid };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut token: HANDLE = std::ptr::null_mut();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token) != 0;
        CloseHandle(process);
        if !opened {
            return None;
        }
        let result = token_security(token, pid);
        CloseHandle(token);
        result
    }
}

unsafe fn token_info(token: HANDLE, class: i32) -> Option<Vec<u64>> {
    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 || needed > 64 * 1024 {
            return None;
        }
        // u64 storage keeps the structures aligned.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        (GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) != 0)
            .then_some(buffer)
    }
}

unsafe fn token_security(token: HANDLE, pid: u32) -> Option<yiime_core::ProcessSecurity> {
    unsafe {
        let elevation = token_info(token, TokenElevation)?;
        let elevated = (*elevation.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated != 0;
        let label = token_info(token, TokenIntegrityLevel)?;
        let sid = (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        let integrity = *GetSidSubAuthority(sid, u32::from(count) - 1);
        let user = token_info(token, TokenUser)?;
        let user_sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let length = GetLengthSid(user_sid) as usize;
        let user = std::slice::from_raw_parts(user_sid.cast::<u8>(), length).to_vec();
        let mut session = 0u32;
        if ProcessIdToSessionId(pid, &mut session) == 0 {
            return None;
        }
        Some(yiime_core::ProcessSecurity {
            elevated,
            integrity,
            session,
            user,
        })
    }
}
