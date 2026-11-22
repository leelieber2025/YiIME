use super::*;

/// Opens the settings window (or brings it forward).
pub(super) unsafe fn open_settings(owner: HWND) {
    let Some(state) = (unsafe { state_of(owner) }) else {
        return;
    };
    let existing = state.settings_window.get();
    if !existing.is_null() {
        unsafe {
            ShowWindow(existing, SW_SHOWNORMAL);
            SetForegroundWindow(existing);
        }
        return;
    }
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let ex_style = WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_DLGMODALFRAME;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 440,
        bottom: 524 + OPTION_SHIFT + ENGINE_SHIFT,
    };
    unsafe {
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);
    }
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let (x, y) = unsafe {
        (
            (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - height) / 3,
        )
    };
    let window = unsafe {
        CreateWindowExW(
            ex_style,
            wide("InputTranslationSettings").as_ptr(),
            wide(concat!("YiIME 译 · 设置 · v", env!("CARGO_PKG_VERSION"))).as_ptr(),
            style,
            x,
            y,
            width,
            height,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            owner as *const std::ffi::c_void,
        )
    };
    if window.is_null() {
        return;
    }
    state.settings_window.set(window);
    unsafe {
        ShowWindow(window, SW_SHOWNORMAL);
        SetForegroundWindow(window);
        let url = GetDlgItem(window, SETTINGS_URL);
        SetFocus(url);
        SendMessageW(url, EM_SETSEL, 0, -1);
    }
}
pub(super) unsafe extern "system" fn settings_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    let owner = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as HWND;
    match msg {
        WM_CREATE => unsafe {
            let create = &*(l as *const CREATESTRUCTW);
            let owner = create.lpCreateParams as HWND;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, owner as isize);
            let Some(state) = state_of(owner) else {
                return -1;
            };
            let settings = state.settings.borrow().clone();
            label(
                hwnd,
                "大模型 API 地址（OpenAI 兼容；选 Google 时作备用，可不填）",
                16,
                12,
                408,
            );
            control(
                hwnd,
                "EDIT",
                &settings.url,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                34,
                408,
                24,
                SETTINGS_URL,
            );
            label(hwnd, "模型", 16, 68, 196);
            control(
                hwnd,
                "EDIT",
                &settings.model,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                90,
                196,
                24,
                SETTINGS_MODEL,
            );
            label(hwnd, "API 密钥（Windows 加密保存）", 228, 68, 196);
            control(
                hwnd,
                "EDIT",
                &settings.key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                228,
                90,
                196,
                24,
                SETTINGS_KEY,
            );
            label(hwnd, "翻译风格", 16, 124, 196);
            let combo = control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                16,
                146,
                196,
                220,
                SETTINGS_STYLE,
            );
            for style in TranslationStyle::ALL {
                SendMessageW(
                    combo,
                    CB_ADDSTRING,
                    0,
                    wide(style.label()).as_ptr() as isize,
                );
            }
            let selected = TranslationStyle::ALL
                .iter()
                .position(|style| *style == settings.style)
                .unwrap_or(0);
            SendMessageW(combo, CB_SETCURSEL, selected, 0);
            label(
                hwnd,
                "自定义要求（可选，追加到翻译提示词，例如“保留产品名不翻译”）",
                16,
                182,
                408,
            );
            control(
                hwnd,
                "EDIT",
                &settings.custom,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                204,
                408,
                24,
                SETTINGS_CUSTOM,
            );
            let plain = control(
                hwnd,
                "BUTTON",
                "普通五笔模式（不翻译，选字直接上屏）",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                16,
                242,
                408,
                24,
                SETTINGS_PLAIN,
            );
            SendMessageW(plain, BM_SETCHECK, plain_mode() as usize, 0);
            label(
                hwnd,
                "切换快捷键（如 Ctrl+Shift+Space；重新切到本输入法后生效）",
                16,
                272,
                408,
            );
            control(
                hwnd,
                "EDIT",
                &plain_toggle_text(),
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                294,
                196,
                24,
                SETTINGS_HOTKEY,
            );
            label(hwnd, "每页候选数（1–9）", 228, 298, 128);
            control(
                hwnd,
                "EDIT",
                &OPTIONS.page_size().to_string(),
                ES_NUMBER as u32 | WS_TABSTOP,
                360,
                294,
                48,
                24,
                SETTINGS_PAGE_SIZE,
            );
            SendMessageW(GetDlgItem(hwnd, SETTINGS_PAGE_SIZE), EM_LIMITTEXT, 2, 0);
            for (index, (id, text, option_key)) in OPTION_BOXES.iter().enumerate() {
                let x = if index % 2 == 0 { 16 } else { 228 };
                let y = 330 + (index as i32 / 2) * 26;
                let checkbox = control(
                    hwnd,
                    "BUTTON",
                    text,
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                    x,
                    y,
                    204,
                    24,
                    *id,
                );
                // "中文标点" checked = PunctEnglish off.
                let on = option(*option_key) != (*option_key == Flag::PunctEnglish);
                SendMessageW(checkbox, BM_SETCHECK, on as usize, 0);
            }
            let history = control(
                hwnd,
                "BUTTON",
                "输入记忆：保存翻译过的中文，快捷键找回重新编辑/翻译",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                16,
                412 + OPTION_SHIFT,
                408,
                24,
                SETTINGS_HISTORY,
            );
            SendMessageW(
                history,
                BM_SETCHECK,
                option(Flag::HistoryEnabled) as usize,
                0,
            );
            label(hwnd, "找回快捷键", 16, 446 + OPTION_SHIFT, 80);
            control(
                hwnd,
                "EDIT",
                &history_key_text(),
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                100,
                442 + OPTION_SHIFT,
                150,
                24,
                SETTINGS_HISTORY_KEY,
            );
            control(
                hwnd,
                "BUTTON",
                "清除输入记忆",
                WS_TABSTOP,
                272,
                440 + OPTION_SHIFT,
                152,
                28,
                SETTINGS_CLEAR_HISTORY,
            );
            control(
                hwnd,
                "BUTTON",
                "用户词库…",
                WS_TABSTOP,
                16,
                482 + OPTION_SHIFT,
                100,
                28,
                SETTINGS_USER_WORDS,
            );
            control(
                hwnd,
                "BUTTON",
                "保存",
                BS_DEFPUSHBUTTON as u32 | WS_TABSTOP,
                256,
                482 + OPTION_SHIFT,
                80,
                28,
                ID_OK,
            );
            control(
                hwnd,
                "BUTTON",
                "取消",
                WS_TABSTOP,
                344,
                482 + OPTION_SHIFT,
                80,
                28,
                ID_CANCEL,
            );
            // Insert the Translation Engine row at the top: move the existing controls down and
            // put the new ones first in tab order.
            let mut child = GetWindow(hwnd, GW_CHILD);
            while !child.is_null() {
                let mut rect: RECT = std::mem::zeroed();
                GetWindowRect(child, &mut rect);
                MapWindowPoints(null_mut(), hwnd, &mut rect as *mut RECT as *mut POINT, 2);
                SetWindowPos(
                    child,
                    null_mut(),
                    rect.left,
                    rect.top + ENGINE_SHIFT,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                child = GetWindow(child, GW_HWNDNEXT);
            }
            label(hwnd, "翻译引擎", 16, 12, 196);
            let engine = control(
                hwnd,
                "COMBOBOX",
                "",
                CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP,
                16,
                34,
                196,
                160,
                SETTINGS_ENGINE,
            );
            for item in Engine::ALL {
                SendMessageW(
                    engine,
                    CB_ADDSTRING,
                    0,
                    wide(item.label()).as_ptr() as isize,
                );
            }
            let selected = Engine::ALL
                .iter()
                .position(|item| *item == settings.engine)
                .unwrap_or(0);
            SendMessageW(engine, CB_SETCURSEL, selected, 0);
            label(hwnd, "Google Cloud 密钥（可选，加密保存）", 228, 12, 196);
            let google_key = control(
                hwnd,
                "EDIT",
                &settings.google_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                228,
                34,
                196,
                24,
                SETTINGS_GOOGLE_KEY,
            );
            label(hwnd, "Azure 翻译密钥（加密保存）", 16, 68, 196);
            let azure_key = control(
                hwnd,
                "EDIT",
                &settings.azure_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                90,
                196,
                24,
                SETTINGS_AZURE_KEY,
            );
            label(hwnd, "Azure 区域（如 eastus，全局可留空）", 228, 68, 196);
            let azure_region = control(
                hwnd,
                "EDIT",
                &settings.azure_region,
                ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                228,
                90,
                196,
                24,
                SETTINGS_AZURE_REGION,
            );
            label(
                hwnd,
                "DeepL 密钥（加密保存；以 :fx 结尾为免费版，译文为美式英文）",
                16,
                124,
                408,
            );
            let deepl_key = control(
                hwnd,
                "EDIT",
                &settings.deepl_key,
                ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32 | WS_TABSTOP,
                16,
                146,
                196,
                24,
                SETTINGS_DEEPL_KEY,
            );
            let fallback = control(
                hwnd,
                "BUTTON",
                "失败时允许改用其他翻译服务",
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                228,
                146,
                196,
                24,
                SETTINGS_FALLBACK,
            );
            SendMessageW(fallback, BM_SETCHECK, settings.allow_fallback as usize, 0);
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            SetWindowPos(engine, HWND_TOP, 0, 0, 0, 0, flags);
            SetWindowPos(google_key, engine, 0, 0, 0, 0, flags);
            SetWindowPos(azure_key, google_key, 0, 0, 0, 0, flags);
            SetWindowPos(azure_region, azure_key, 0, 0, 0, 0, flags);
            SetWindowPos(deepl_key, azure_region, 0, 0, 0, 0, flags);
            SetWindowPos(fallback, deepl_key, 0, 0, 0, 0, flags);
            0
        },
        WM_COMMAND => unsafe {
            match (w & 0xffff) as i32 {
                ID_OK => {
                    let Some(state) = state_of(owner) else {
                        DestroyWindow(hwnd);
                        return 0;
                    };
                    let field = |id| get_text(GetDlgItem(hwnd, id)).trim().to_owned();
                    let index = SendMessageW(GetDlgItem(hwnd, SETTINGS_STYLE), CB_GETCURSEL, 0, 0);
                    let engine_index =
                        SendMessageW(GetDlgItem(hwnd, SETTINGS_ENGINE), CB_GETCURSEL, 0, 0);
                    let settings = ApiSettings {
                        engine: usize::try_from(engine_index)
                            .ok()
                            .and_then(|index| Engine::ALL.get(index).copied())
                            .unwrap_or_default(),
                        allow_fallback: SendMessageW(
                            GetDlgItem(hwnd, SETTINGS_FALLBACK),
                            BM_GETCHECK,
                            0,
                            0,
                        ) == 1,
                        google_key: field(SETTINGS_GOOGLE_KEY),
                        azure_key: field(SETTINGS_AZURE_KEY),
                        azure_region: field(SETTINGS_AZURE_REGION),
                        deepl_key: field(SETTINGS_DEEPL_KEY),
                        url: field(SETTINGS_URL),
                        model: field(SETTINGS_MODEL),
                        key: field(SETTINGS_KEY),
                        style: usize::try_from(index)
                            .ok()
                            .and_then(|index| TranslationStyle::ALL.get(index).copied())
                            .unwrap_or_default(),
                        custom: field(SETTINGS_CUSTOM)
                            .chars()
                            .take(MAX_CUSTOM_LEN)
                            .collect(),
                    };
                    let plain =
                        SendMessageW(GetDlgItem(hwnd, SETTINGS_PLAIN), BM_GETCHECK, 0, 0) == 1; // BST_CHECKED
                    let hotkey = field(SETTINGS_HOTKEY);
                    // Page size must be a number; values outside 1–9 are clamped and written
                    // back to the field.
                    let page = match parse_page_size(&field(SETTINGS_PAGE_SIZE)) {
                        Ok(page) => page,
                        Err(error) => {
                            message(hwnd, error);
                            return 0;
                        }
                    };
                    SetWindowTextW(
                        GetDlgItem(hwnd, SETTINGS_PAGE_SIZE),
                        wide(&page.to_string()).as_ptr(),
                    );
                    let history_key = field(SETTINGS_HISTORY_KEY);
                    let history_on =
                        SendMessageW(GetDlgItem(hwnd, SETTINGS_HISTORY), BM_GETCHECK, 0, 0) == 1;
                    // Turning history off: confirm, then delete saved history; on cancel the
                    // settings are not saved.
                    let history_has_data = history_path().is_some_and(|path| path.exists());
                    if !history_on
                        && history_has_data
                        && MessageBoxW(
                            hwnd,
                            wide("关闭输入记忆会删除已保存的全部记忆，确定吗？").as_ptr(),
                            wide(APP_NAME).as_ptr(),
                            MB_YESNO | MB_ICONQUESTION,
                        ) != IDYES
                    {
                        return 0;
                    }
                    // Plain Wubi mode never translates, so API settings are not required. With
                    // Google selected the LLM is an optional fallback, but once a key is
                    // entered the whole LLM configuration must be valid.
                    let api_check = if plain && settings.key.is_empty() {
                        Ok(())
                    } else if settings.engine != Engine::Llm && !settings.key.trim().is_empty() {
                        settings.validate().and_then(|_| settings.validate_llm())
                    } else {
                        settings.validate()
                    };
                    if parse_hotkey(&hotkey).is_none() || parse_hotkey(&history_key).is_none() {
                        message(
                            hwnd,
                            "快捷键无效：用 + 连接 Ctrl/Shift/Alt 与 Space、字母、数字或 F1～F12，且须含 Ctrl 或 Alt（F 键除外）",
                        );
                    } else if let Err(error) = api_check {
                        message(hwnd, error);
                    } else if !set_plain_mode(plain)
                        || !OPTIONS.set_plain_toggle_key(&hotkey)
                        || !set_option(Flag::HistoryEnabled, history_on)
                        || !OPTIONS.set_page_size(page)
                        || !OPTIONS.set_history_key(&history_key)
                        || (!history_on && !clear_history())
                        || !OPTION_BOXES.iter().all(|(id, _, option_key)| {
                            let checked =
                                SendMessageW(GetDlgItem(hwnd, *id), BM_GETCHECK, 0, 0) == 1;
                            set_option(*option_key, checked != (*option_key == Flag::PunctEnglish))
                        })
                    {
                        message(hwnd, "保存模式设置失败");
                    } else if let Err(error) = save_settings(&settings) {
                        message(hwnd, &format!("保存设置失败：{error}"));
                    } else {
                        *state.settings.borrow_mut() = settings;
                        if state
                            .status
                            .borrow()
                            .as_ref()
                            .is_some_and(|(_, error)| *error)
                        {
                            set_status(
                                owner,
                                state,
                                Some(("设置已保存，按 Enter 翻译".into(), false)),
                            );
                        }
                        DestroyWindow(hwnd);
                    }
                }
                ID_CANCEL => {
                    DestroyWindow(hwnd);
                }
                SETTINGS_CLEAR_HISTORY => {
                    if MessageBoxW(
                        hwnd,
                        wide("删除全部输入记忆？").as_ptr(),
                        wide(APP_NAME).as_ptr(),
                        MB_YESNO | MB_ICONQUESTION,
                    ) == IDYES
                    {
                        message(
                            hwnd,
                            if clear_history() {
                                "输入记忆已清除"
                            } else {
                                "清除失败"
                            },
                        );
                    }
                }
                SETTINGS_USER_WORDS => {
                    // Open user words in Notepad (creating the file with a short header if
                    // missing); changes apply the next time the box opens.
                    if let Some(path) = user_words_path() {
                        if !path.exists() {
                            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
                            let _ = std::fs::write(
                                &path,
                                "# YiIME 用户词库：每行「词 编码」（空格或 Tab 分隔），排在该五笔编码的第一位。\r\n# 例如：\r\n# 中英翻译 khyc\r\n",
                            );
                        }
                        let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
                    }
                }
                _ => {}
            }
            0
        },
        WM_CLOSE => unsafe {
            DestroyWindow(hwnd);
            0
        },
        WM_DESTROY => unsafe {
            if let Some(state) = state_of(owner) {
                state.settings_window.set(null_mut());
                if IsWindowVisible(owner) != 0 {
                    SetForegroundWindow(owner);
                    SetFocus(state.edit);
                }
            }
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
