#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("此界面仅支持 Windows");
}

#[cfg(windows)]
#[path = "windows_gui/dictionary.rs"]
mod dictionary;

/// Floating input box at the text cursor: compose a Chinese draft with Wubi/pinyin, press Enter to
/// translate and send the English back to the original application. Settings open in a separate
/// window from the tray menu.
#[cfg(windows)]
#[path = "windows_gui/app.rs"]
mod app;

#[cfg(windows)]
fn main() {
    app::run();
}
