//! Session-wide cell for the 译/普通五笔 mode (shared by the TSF DLL and the input-box EXE via
//! `#[path]`). Packaged desktop apps such as Win11 Notepad write HKCU into a private copy, so the
//! registry alone cannot carry a switch between them and other processes; see
//! [`yiime_core::plain_cell`]. Any failure leaves the registry as the only source.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
};

const CELL_NAME: &str = "Local\\YiIME-PlainMode-1";

struct Cell(Option<&'static AtomicU32>);

fn cell() -> Option<&'static AtomicU32> {
    static CELL: OnceLock<Cell> = OnceLock::new();
    CELL.get_or_init(|| {
        let name: Vec<u16> = CELL_NAME.encode_utf16().chain(Some(0)).collect();
        // The mapping (and its handle) lives for the whole process, keeping the cell alive.
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                4096,
                name.as_ptr(),
            )
        };
        if mapping.is_null() {
            return Cell(None);
        }
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 4) };
        if view.Value.is_null() {
            return Cell(None);
        }
        // Page-aligned, zero-filled and never unmapped.
        Cell(Some(unsafe { &*(view.Value as *const AtomicU32) }))
    })
    .0
}

/// This process has package identity, so its HKCU writes may stay private to its package.
fn private_registry() -> bool {
    static PRIVATE: OnceLock<bool> = OnceLock::new();
    *PRIVATE.get_or_init(|| {
        let mut len = 0u32;
        let status = unsafe { GetCurrentPackageFullName(&mut len, std::ptr::null_mut()) };
        status == ERROR_INSUFFICIENT_BUFFER
    })
}

/// The current mode, given this process's registry view; `adopt` stores a mode the registry
/// should take (a switch made in a packaged host).
pub fn read(registry: bool, adopt: impl FnOnce(bool) -> bool) -> bool {
    let Some(cell) = cell() else {
        return registry;
    };
    let read =
        yiime_core::plain_cell::read(cell.load(Ordering::Acquire), registry, private_registry());
    let adopted = !read.adopt || adopt(read.plain);
    if let Some(value) = yiime_core::plain_cell::store_after_adopt(read, adopted) {
        cell.store(value, Ordering::Release);
    }
    read.plain
}

/// Publishes a switch made in this process (after writing it to the registry).
pub fn wrote(plain: bool) {
    if let Some(cell) = cell() {
        cell.store(
            yiime_core::plain_cell::encode(plain, private_registry()),
            Ordering::Release,
        );
    }
}
