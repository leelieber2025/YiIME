/// Writes `bytes` to a temporary file next to `path`, then renames it over `path`, so a crash or
/// full disk never leaves a truncated file.
pub fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write;
    let Some(dir) = path.parent() else {
        return false;
    };
    let _ = std::fs::create_dir_all(dir);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
    let written = std::fs::File::create(&temp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if written.is_err() || std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}

pub(crate) fn lock(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::fs::OpenOptions;
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    #[cfg(windows)]
    let lock = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match options.open(&lock_name) {
                Ok(file) => break file,
                // ERROR_SHARING_VIOLATION: another YiIME process holds the lock file.
                Err(error)
                    if error.raw_os_error() == Some(32) && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
    };
    #[cfg(not(windows))]
    let lock = options.open(lock_name)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        unsafe extern "C" {
            fn flock(fd: i32, op: i32) -> i32;
        }
        // LOCK_EX; the kernel releases the lock on close, including after a crash.
        if unsafe { flock(lock.as_raw_fd(), 2) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(lock)
}
