use super::*;

/// Chinese/English mode button left of the taskbar IME icon (GUID_LBI_INPUTMODE, as in Microsoft
/// Pinyin/Wubi). Clicking toggles the mode; it watches the OPENCLOSE compartment to refresh when
/// the mode changes by Ctrl+Space or other means.
#[implement(ITfLangBarItemButton, ITfSource, ITfCompartmentEventSink)]
pub(super) struct InputModeButton {
    pub(super) manager: ITfThreadMgr,
    pub(super) client_id: u32,
    /// Shared with the TextService so the button refreshes when plain/translation mode changes
    /// elsewhere.
    pub(super) sink: LangBarSink,
}
pub(super) type LangBarSink = Rc<RefCell<Option<ITfLangBarItemSink>>>;
pub(super) fn refresh_sink(sink: &LangBarSink) {
    let sink = sink.borrow().clone();
    if let Some(sink) = sink {
        let _ = unsafe { sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP) };
    }
}
const MENU_CHINESE: u32 = 1;
const MENU_ENGLISH: u32 = 2;
const MENU_TRANSLATE: u32 = 3;
const MENU_PLAIN: u32 = 4;
const MENU_PUNCT: u32 = 5;
const MENU_TRAD: u32 = 6;
const MENU_HISTORY: u32 = 7;
const MENU_SETTINGS: u32 = 8;
impl InputModeButton_Impl {
    fn chinese(&self) -> bool {
        keyboard_mode(&self.manager).unwrap_or(true)
    }
    /// Menu owner: the window of the focused context, else this thread's foreground/focus window
    /// (as Weasel does).
    fn menu_owner(&self) -> HWND {
        let from_context = unsafe {
            self.manager
                .GetFocus()
                .and_then(|document| document.GetTop())
                .and_then(|context| context.GetActiveView())
                .and_then(|view| view.GetWnd())
        }
        .ok()
        .map(|hwnd| hwnd.0 as HWND)
        .filter(|hwnd| !hwnd.is_null());
        from_context
            .or_else(|| {
                let foreground = unsafe { GetForegroundWindow() };
                let thread = unsafe { GetWindowThreadProcessId(foreground, null_mut()) };
                (thread == unsafe { GetCurrentThreadId() }).then_some(foreground)
            })
            .or_else(|| Some(unsafe { GetFocus() }))
            .filter(|hwnd| !hwnd.is_null())
            .unwrap_or(null_mut())
    }
    /// Right-click menu of the taskbar mode icon: Chinese/English, translation/plain Wubi,
    /// punctuation, Simplified/Traditional, history, settings.
    fn show_menu(&self, x: i32, y: i32) {
        let owner = self.menu_owner();
        if owner.is_null() {
            log("右键菜单：找不到本线程的窗口");
            return;
        }
        let chinese = self.chinese();
        let plain = plain_mode();
        let checked = |on: bool| if on { MF_CHECKED } else { 0 };
        let history = if option(Flag::HistoryEnabled) {
            let key = OPTIONS.history_key();
            format!("输入记忆…\t{key}")
        } else {
            "输入记忆…（已关闭）".to_owned()
        };
        let items: [Option<(u32, String, u32)>; 11] = [
            Some((MENU_CHINESE, "中文".into(), checked(chinese))),
            Some((MENU_ENGLISH, "英文".into(), checked(!chinese))),
            None,
            Some((
                MENU_TRANSLATE,
                format!("翻译模式（译）\t{}", plain_toggle_text()),
                checked(!plain),
            )),
            Some((
                MENU_PLAIN,
                format!("普通五笔模式（中）\t{}", plain_toggle_text()),
                checked(plain),
            )),
            None,
            Some((
                MENU_PUNCT,
                "中文标点\tCtrl+.".into(),
                checked(!option(Flag::PunctEnglish)),
            )),
            Some((
                MENU_TRAD,
                "繁体输出\tCtrl+Shift+F".into(),
                checked(option(Flag::Traditional)),
            )),
            None,
            Some((MENU_HISTORY, history, 0)),
            Some((MENU_SETTINGS, "设置…".into(), 0)),
        ];
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        for item in items {
            match item {
                Some((id, text, flags)) => unsafe {
                    AppendMenuW(menu, MF_STRING | flags, id as usize, wide(&text).as_ptr());
                },
                None => unsafe {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                },
            }
        }
        // The icon is on the taskbar, so the menu opens upward; TPM_RETURNCMD returns the choice
        // synchronously.
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let id = unsafe { TrackPopupMenuEx(menu, flags, x, y, owner, std::ptr::null()) } as u32;
        unsafe {
            DestroyMenu(menu);
        }
        log(&format!("右键菜单选择 {id}"));
        match id {
            MENU_CHINESE | MENU_ENGLISH => {
                set_keyboard_mode(&self.manager, self.client_id, id == MENU_CHINESE);
            }
            MENU_TRANSLATE | MENU_PLAIN => {
                let plain = id == MENU_PLAIN;
                set_plain_mode(plain);
                if !chinese {
                    set_keyboard_mode(&self.manager, self.client_id, true);
                }
                // Arm the route even if the flag was already in this state: 译 opens the
                // draft box, 普通五笔 closes it.
                notify_plain_mode(plain);
            }
            MENU_PUNCT => {
                toggle_option(Flag::PunctEnglish);
            }
            MENU_TRAD => {
                toggle_option(Flag::Traditional);
            }
            MENU_HISTORY => open_tray_window(true),
            MENU_SETTINGS => open_tray_window(false),
            _ => {}
        }
    }
    fn refresh(&self) {
        refresh_sink(&self.sink);
    }
}
/// Settings switches in HKCU\\Software\\YiIME, shared with the input-box EXE (names and
/// defaults defined in yiime-core).
struct Registry;
impl OptionStore for Registry {
    fn get_u32(&self, name: &str) -> Option<u32> {
        CURRENT_USER
            .open(STATE_KEY)
            .and_then(|key| key.get_u32(name))
            .ok()
    }
    fn set_u32(&self, name: &str, value: u32) -> bool {
        CURRENT_USER
            .create(STATE_KEY)
            .and_then(|key| key.set_u32(name, value))
            .is_ok()
    }
    fn get_string(&self, name: &str) -> Option<String> {
        CURRENT_USER
            .open(STATE_KEY)
            .and_then(|key| key.get_string(name))
            .ok()
    }
    fn set_string(&self, name: &str, value: &str) -> bool {
        CURRENT_USER
            .create(STATE_KEY)
            .and_then(|key| key.set_string(name, value))
            .is_ok()
    }
}
const OPTIONS: Options<Registry> = Options(Registry);
pub(super) fn history_key_text() -> String {
    OPTIONS.history_key()
}
pub(super) fn option(flag: Flag) -> bool {
    OPTIONS.flag(flag)
}
pub(super) fn toggle_option(flag: Flag) -> bool {
    OPTIONS.toggle(flag).is_some()
}
/// 译/普通五笔, read through the session cell so packaged hosts (Notepad), whose HKCU is a
/// private copy, follow a switch made in explorer.exe's language bar or anywhere else.
pub(super) fn plain_mode() -> bool {
    crate::plain_cell::read(option(Flag::PlainMode), |plain| {
        OPTIONS.set_flag(Flag::PlainMode, plain)
    })
}
pub(super) fn set_plain_mode(on: bool) -> bool {
    let ok = OPTIONS.set_flag(Flag::PlainMode, on);
    crate::plain_cell::wrote(on);
    ok
}
pub(super) fn page_size() -> usize {
    OPTIONS.page_size()
}
pub(super) fn ui_scale() -> u32 {
    OPTIONS.ui_scale()
}
pub(super) fn shuangpin_scheme() -> Option<yiime_core::Shuangpin> {
    OPTIONS.shuangpin()
}
/// Chinese punctuation without the input box (US-layout virtual key + Shift, mapping in
/// yiime-core); `.` `:` after digits stay half-width. Quotes return None and are alternated by the
/// caller.
pub(super) fn chinese_punctuation(
    key: usize,
    shift: bool,
    after_digit: bool,
) -> Option<&'static str> {
    let key = u16::try_from(key).ok()?;
    us_key_char(key, shift).and_then(|ch| full_width(ch, after_digit))
}
/// Parses shortcuts such as `Ctrl+Shift+Space` (rules in yiime-core) into a TSF preserved key.
pub(super) fn parse_hotkey(text: &str) -> Option<TF_PRESERVEDKEY> {
    let hotkey = yiime_core::parse_hotkey(text)?;
    let mut modifiers = 0u32;
    if hotkey.ctrl {
        modifiers |= TF_MOD_CONTROL;
    }
    if hotkey.shift {
        modifiers |= TF_MOD_SHIFT;
    }
    if hotkey.alt {
        modifiers |= TF_MOD_ALT;
    }
    Some(TF_PRESERVEDKEY {
        uVKey: hotkey.vk as u32,
        uModifiers: modifiers,
    })
}
pub(super) fn plain_toggle_text() -> String {
    OPTIONS.plain_toggle_key()
}
fn light_taskbar() -> bool {
    windows_registry::CURRENT_USER
        .open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_u32("SystemUsesLightTheme"))
        .is_ok_and(|value| value != 0)
}
impl ITfLangBarItem_Impl for InputModeButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        com_guard("GetInfo", || {
            if info.is_null() {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            let mut description = [0u16; 32];
            for (slot, unit) in description.iter_mut().zip("中英模式".encode_utf16()) {
                *slot = unit;
            }
            unsafe {
                *info = TF_LANGBARITEMINFO {
                    clsidService: CLSID,
                    guidItem: GUID_LBI_INPUTMODE,
                    // As Microsoft's IMEs do: BTN_BUTTON only; OnClick(RIGHT) opens the menu itself
                    // (the Win10/11 input indicator no longer calls InitMenu).
                    dwStyle: TF_LBI_STYLE_BTN_BUTTON,
                    ulSort: 0,
                    szDescription: description,
                };
            }
            Ok(())
        })
    }
    fn GetStatus(&self) -> Result<u32> {
        com_guard("GetStatus", || Ok(0))
    }
    fn Show(&self, _show: BOOL) -> Result<()> {
        com_guard("Show", || Ok(()))
    }
    fn GetTooltipString(&self) -> Result<BSTR> {
        com_guard("GetTooltipString", || {
            let toggle = plain_toggle_text();
            let extras = format!(
                " · {} · {}",
                if option(Flag::PunctEnglish) {
                    "英文标点"
                } else {
                    "中文标点"
                },
                if option(Flag::Traditional) {
                    "繁体"
                } else {
                    "简体"
                }
            );
            Ok(BSTR::from(
                match (self.chinese(), plain_mode()) {
                    (false, _) => "英文模式（单击、Shift 或 Ctrl+Space 切到中文）".to_owned(),
                    (true, false) => {
                        format!(
                            "译：翻译模式（单击/Shift：中英切换；{toggle}：普通五笔模式；右键：菜单）"
                        )
                    }
                    (true, true) => format!(
                        "中：普通五笔模式，不翻译（单击/Shift：中英切换；{toggle}：翻译模式；右键：菜单）"
                    ),
                } + &extras,
            ))
        })
    }
}
impl ITfLangBarItemButton_Impl for InputModeButton_Impl {
    fn OnClick(
        &self,
        click: TfLBIClick,
        point: &windows::Win32::Foundation::POINT,
        _area: *const RECT,
    ) -> Result<()> {
        com_guard("OnClick", || {
            log(&format!("模式按钮 OnClick {}", click.0));
            if click == TF_LBI_CLK_RIGHT {
                self.show_menu(point.x, point.y);
            } else {
                set_keyboard_mode(&self.manager, self.client_id, !self.chinese());
            }
            self.refresh();
            Ok(())
        })
    }
    fn InitMenu(&self, _menu: Ref<ITfMenu>) -> Result<()> {
        com_guard("InitMenu", || Ok(()))
    }
    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        com_guard("OnMenuSelect", || Ok(()))
    }
    fn GetIcon(&self) -> Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
        com_guard("GetIcon", || {
            // Resources 101/102/105: light taskbar (dark glyph) 中/英/译; 103/104/106: dark taskbar (white
            // glyph). Translation mode shows 译, plain Wubi 中, English 英. The caller destroys the icon.
            let light = light_taskbar();
            let id = match (self.chinese(), plain_mode()) {
                (false, _) => {
                    if light {
                        102
                    } else {
                        104
                    }
                }
                (true, true) => {
                    if light {
                        101
                    } else {
                        103
                    }
                }
                (true, false) => {
                    if light {
                        105
                    } else {
                        106
                    }
                }
            };
            let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
            let icon = unsafe {
                LoadImageW(
                    MODULE.load(Ordering::SeqCst) as _,
                    std::ptr::without_provenance::<u16>(id),
                    IMAGE_ICON,
                    size,
                    size,
                    0,
                )
            };
            if icon.is_null() {
                log(&format!("模式图标资源 {id} 加载失败"));
                Err(E_FAIL.into())
            } else {
                Ok(windows::Win32::UI::WindowsAndMessaging::HICON(icon))
            }
        })
    }
    fn GetText(&self) -> Result<BSTR> {
        com_guard("GetText", || {
            Ok(BSTR::from(match (self.chinese(), plain_mode()) {
                (false, _) => "英",
                (true, true) => "中",
                (true, false) => "译",
            }))
        })
    }
}
impl ITfSource_Impl for InputModeButton_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Ref<IUnknown>) -> Result<u32> {
        com_guard("AdviseSink", || {
            if riid.is_null() || unsafe { *riid } != ITfLangBarItemSink::IID {
                return Err(HRESULT(0x8004_0202_u32 as i32).into()); // CONNECT_E_CANNOTCONNECT
            }
            let sink: ITfLangBarItemSink = punk.ok()?.cast()?;
            *self.sink.borrow_mut() = Some(sink);
            Ok(LANGBAR_SINK_COOKIE)
        })
    }
    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        com_guard("UnadviseSink", || {
            if cookie != LANGBAR_SINK_COOKIE {
                return Err(HRESULT(0x8004_0200_u32 as i32).into()); // CONNECT_E_NOCONNECTION
            }
            self.sink.borrow_mut().take();
            Ok(())
        })
    }
}
impl ITfCompartmentEventSink_Impl for InputModeButton_Impl {
    fn OnChange(&self, guid: *const GUID) -> Result<()> {
        com_guard("OnChange", || {
            if guid.is_null() {
                return Ok(());
            }
            let guid = unsafe { *guid };
            if guid == GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION {
                // The taskbar or touch keyboard changed the NATIVE bit: follow it with OPENCLOSE
                // (skipping equal values to avoid ping-pong).
                if let Some(mode) = conversion_mode(&self.manager) {
                    let native = mode & TF_CONVERSIONMODE_NATIVE as i32 != 0;
                    if native != self.chinese() {
                        set_keyboard_mode(&self.manager, self.client_id, native);
                    }
                }
                self.refresh();
            } else if guid == GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
                self.refresh();
                if !self.chinese() {
                    notify_english_mode();
                }
            }
            Ok(())
        })
    }
}
