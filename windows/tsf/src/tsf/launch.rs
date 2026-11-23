//! Start the companion `YiIME.exe` when the TIP needs it and the process is not running.
//!
//! The installer also puts a shortcut in the all-users Startup folder (see `yiime.iss`), so a
//! login starts the tray via Explorer/`ShellExecute`. Mid-session (after a reboot before the next
//! login finishes, a crash, or an install that has not rebooted) the DLL starts the EXE next to
//! itself the same way qingjian does: `ShellExecuteW` (not `CreateProcess`), a per-process cooldown,
//! and a cross-process mutex so many host processes do not spawn a pile of servers.
//!
//! Only ordinary Medium-integrity desktop apps launch the shared service. Elevated / high-integrity
//! hosts keep using a private editor (`open_editor` `--private`). LogonUI, the UAC consent UI, and
//! AppContainer hosts are refused: a refused launch must not swallow keys.
//!
//! The installer holds `Global\YiIMEInstaller` for the whole setup/uninstall (see `yiime.iss`
//! `HoldInstallerMutex`); this module does not start the EXE while that mutex exists, so the
//! installer can replace the file.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use yiime_core::MEDIUM_INTEGRITY;

use super::{companion_exe, log, wide};

/// Same process: do not retry a launch on every key.
const LAUNCH_COOLDOWN: Duration = Duration::from_secs(5);

/// Cross-process: only one host starts the shared EXE at a time.
const LAUNCH_MUTEX: &str = "Local\\YiIMECompanionLaunch";

/// Held by the installer for the whole setup/uninstall (`yiime.iss` `HoldInstallerMutex`).
const INSTALLER_MUTEX: &str = "Global\\YiIMEInstaller";

static LAST_LAUNCH: Mutex<Option<(Instant, LaunchOutcome)>> = Mutex::new(None);

/// Result of asking the TIP to start the shared companion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LaunchOutcome {
    /// `ShellExecute` was issued; the EXE should listen shortly.
    Requested,
    /// Another attempt is already in flight (cooldown or the launch mutex).
    InFlight,
    /// This host cannot start the shared EXE, or the last attempt was refused and is still cooling
    /// down. Callers must not swallow keys.
    Refused,
}

/// Start the shared `YiIME.exe` (optional command-line, already formatted for `ShellExecute`).
pub(super) fn launch_companion(parameters: &str) -> LaunchOutcome {
    if let Some(outcome) = cooldown_outcome() {
        return outcome;
    }
    let outcome = attempt_launch(parameters);
    remember(outcome);
    outcome
}

fn attempt_launch(parameters: &str) -> LaunchOutcome {
    if !host_is_plain_desktop_app() {
        log("宿主进程不是普通桌面应用（完整性级别非 Medium），不拉起共享输入框服务");
        return LaunchOutcome::Refused;
    }
    if installer_running() {
        log("安装程序正在运行，不拉起 YiIME.exe");
        return LaunchOutcome::Refused;
    }
    let Some(exe) = companion_exe() else {
        log("找不到与 DLL 同目录（或 CurrentDir）的 YiIME.exe，不拉起");
        return LaunchOutcome::Refused;
    };
    let Some(_mutex) = launch_mutex() else {
        return LaunchOutcome::InFlight;
    };
    let file = wide(&exe.to_string_lossy());
    let workdir = exe
        .parent()
        .map(|dir| wide(&dir.to_string_lossy()))
        .unwrap_or_else(|| wide(""));
    let verb = wide("open");
    let params = wide(parameters);
    let params_ptr = if parameters.is_empty() {
        std::ptr::null()
    } else {
        params.as_ptr()
    };
    // Return immediately: blocking the UI thread can make TSF detach the TIP.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params_ptr,
            workdir.as_ptr(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if (code as isize) > 32 {
        log("已请求启动 YiIME.exe");
        LaunchOutcome::Requested
    } else {
        log(&format!("启动 YiIME.exe 失败，返回值 {}", code as isize));
        LaunchOutcome::Refused
    }
}

/// `None` = cooldown has passed and a new attempt may run.
fn cooldown_outcome() -> Option<LaunchOutcome> {
    let Ok(last) = LAST_LAUNCH.lock() else {
        return Some(LaunchOutcome::Refused);
    };
    let (at, outcome) = (*last)?;
    if at.elapsed() >= LAUNCH_COOLDOWN {
        return None;
    }
    // A refused attempt must not be reported as InFlight, or the caller would swallow keys.
    Some(if outcome == LaunchOutcome::Refused {
        LaunchOutcome::Refused
    } else {
        LaunchOutcome::InFlight
    })
}

fn remember(outcome: LaunchOutcome) {
    if let Ok(mut last) = LAST_LAUNCH.lock() {
        *last = Some((Instant::now(), outcome));
    }
}

/// Whether this host can use the shared draft-box service. AppContainer hosts such as
/// SearchHost / Start menu cannot launch or talk to YiIME.exe (and often cannot read
/// HKCU\Software\YiIME, so PlainMode falls back to its default, 译).
pub(super) fn draft_box_usable() -> bool {
    host_is_plain_desktop_app()
}

fn host_is_plain_desktop_app() -> bool {
    crate::process_security::process_security(0)
        .is_some_and(|host| !host.elevated && host.integrity == MEDIUM_INTEGRITY)
}

fn installer_running() -> bool {
    let name = wide(INSTALLER_MUTEX);
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    unsafe {
        CloseHandle(handle);
    }
    true
}

fn launch_mutex() -> Option<LaunchMutex> {
    let name = wide(LAUNCH_MUTEX);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return None;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        return None;
    }
    Some(LaunchMutex(handle))
}

struct LaunchMutex(HANDLE);

impl Drop for LaunchMutex {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::super::EDITOR_EXE;

    fn companion_exe_path(module: &Path) -> PathBuf {
        module.with_file_name(EDITOR_EXE)
    }

    #[test]
    fn companion_exe_sits_next_to_the_dll() {
        assert_eq!(
            companion_exe_path(Path::new(
                r"C:\Program Files\YiIME\6C8AB52678CD-FCE3F9CF99CC\yiime_tsf.dll"
            )),
            Path::new(r"C:\Program Files\YiIME\6C8AB52678CD-FCE3F9CF99CC\YiIME.exe")
        );
    }
}
