use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

static NOTED_INLINE_FALLBACK: AtomicBool = AtomicBool::new(false);

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        com_guard("OnSetFocus", || {
            self.after_digit.set(false);
            self.tested_key.borrow_mut().take();
            self.shift_down.set(None);
            if !foreground.as_bool() {
                // Losing the key sink here can also be a same-window document change; cancel
                // only when the thread itself loses focus (OnKillThreadFocus) or a key arrives
                // from another field (wants_key).
                // This application's thread lost the keyboard (also when the input box itself
                // takes it; the box then ignores the notice because it has the foreground).
                self.notify_focus_moved();
            }
            if foreground.as_bool() {
                self.sync_plain_mode();
            }
            if foreground.as_bool()
                && let Some(manager) = self.thread_manager.borrow().as_ref()
                && let Ok(document) = unsafe { manager.GetFocus() }
                && let Ok(context) = unsafe { document.GetTop() }
            {
                self.probe_scope(context);
            }
            Ok(())
        })
    }
    fn OnTestKeyDown(
        &self,
        context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        com_guard("OnTestKeyDown", || {
            self.track_key_down(key.0);
            let consume = context.as_ref().is_some_and(|context| {
                self.should_consume(context, key.0) || self.punctuation(context, key.0).is_some()
            });
            if !consume && !is_shift(key.0) {
                let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                self.after_digit.set(
                    (!shift && (0x30..=0x39).contains(&key.0)) || (0x60..=0x69).contains(&key.0),
                );
            }
            *self.tested_key.borrow_mut() = context
                .as_ref()
                .map(|context| (context.clone(), key.0, consume));
            Ok((consume || is_shift(key.0)).into())
        })
    }
    fn OnKeyDown(
        &self,
        context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        com_guard("OnKeyDown", || {
            self.track_key_down(key.0);
            if is_shift(key.0) {
                return Ok(false.into());
            }
            let Some(context) = context.as_ref() else {
                return Ok(false.into());
            };
            let tested = self.tested_key.borrow_mut().take();
            let consume = tested
                .as_ref()
                .filter(|(tested_context, tested_key, _)| {
                    tested_context == context && *tested_key == key.0
                })
                .map(|(_, _, consume)| *consume)
                .unwrap_or_else(|| self.should_consume(context, key.0));
            // Composing in a field just found to be a password field: commit the letters as
            // typed, then this key goes to the application.
            if self.finish_in_password(context) {
                return Ok(false.into());
            }
            if !self.buffering()
                && !self.should_consume(context, key.0)
                && let Some(text) = self.punctuation(context, key.0)
            {
                let inserted = self.insert_direct(context, text);
                if inserted && key.0 == 0xDE {
                    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                    let mut quotes = self.quotes.get();
                    quotes.next(if shift { '"' } else { '\'' });
                    self.quotes.set(quotes);
                }
                self.after_digit.set(false);
                return Ok(inserted.into());
            }
            // Remember digits so a following `.` `:` stays half-width (3.5, 10:30).
            let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
            self.after_digit
                .set((!shift && (0x30..=0x39).contains(&key.0)) || (0x60..=0x69).contains(&key.0));
            // Let the host receive a key if it no longer belongs to this input session.
            if !consume {
                return Ok(false.into());
            }
            if self.buffering() {
                self.pending_keys
                    .borrow_mut()
                    .push(typed_letter(key.0 as u8));
                if !self.editor_window.get().is_null() {
                    self.allow_editor_foreground();
                    unsafe {
                        PostMessageW(self.receiver.get(), WM_APP + 1, 0, 0);
                    }
                }
                return Ok(true.into());
            }
            // 译 mode (PlainMode=false): letters go to the draft box. 普通五笔 keeps
            // in-document composition. AppContainer hosts (SearchHost / Start menu) cannot
            // open the box and often cannot read PlainMode from HKCU, so they would otherwise
            // stay in 译 and hand Latin letters back to the search box. Keep SampleIME-style
            // in-document composition there, as e255d84 did. File Explorer (search box, rename,
            // explorer.exe in general) is treated the same way.
            if !plain_mode() && (0x41..=0x5a).contains(&key.0) && self.draft_box_usable(context) {
                if self.inline.active() {
                    self.inline.cancel(self.client_id.get());
                }
                self.probe_new_field(context);
                // The read may have finished just now (synchronously): never open the box on a
                // password field.
                if self.password_field(context) {
                    return Ok(false.into());
                }
                let opened = self.open_editor(context.clone(), key.0 as u8);
                if !opened {
                    self.note_pass(Some(yiime_core::PassReason::BoxUnavailable));
                    log("翻译模式：草稿框未能打开，按键交还应用");
                }
                return Ok(opened.into());
            }
            if !plain_mode()
                && (0x41..=0x5a).contains(&key.0)
                && !self.draft_box_usable(context)
                && !NOTED_INLINE_FALLBACK.swap(true, Ordering::Relaxed)
            {
                log("翻译模式：此宿主不能打开草稿框，改在文档内组字");
            }
            let typed = self
                .inline
                .active()
                .then(|| self.composing_text(key.0))
                .flatten();
            let quote = key.0 == 0xDE && typed.is_some() && !option(Flag::PunctEnglish);
            if self.inline.key(context, self.client_id.get(), key.0, typed) {
                if quote && !self.inline.active() {
                    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                    let mut quotes = self.quotes.get();
                    quotes.next(if shift { '"' } else { '\'' });
                    self.quotes.set(quotes);
                }
                return Ok(true.into());
            }
            // The host did not accept the composition session. Fall back to the draft box
            // rather than swallowing a key OnTestKeyDown already claimed. Skip the box in
            // hosts that cannot use it, and give the key back.
            if !self.inline.active()
                && (0x41..=0x5a).contains(&key.0)
                && self.draft_box_usable(context)
            {
                self.probe_new_field(context);
                if self.password_field(context) {
                    return Ok(false.into());
                }
                let opened = self.open_editor(context.clone(), key.0 as u8);
                if !opened {
                    self.note_pass(Some(yiime_core::PassReason::BoxUnavailable));
                    log("组字会话未被受理，草稿框也未能打开：按键交还应用");
                }
                return Ok(opened.into());
            }
            Ok(false.into())
        })
    }
    fn OnTestKeyUp(
        &self,
        _context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        com_guard("OnTestKeyUp", || {
            // Ask TSF to deliver the corresponding OnKeyUp; that callback changes the mode.
            Ok(is_shift(key.0).into())
        })
    }
    fn OnKeyUp(
        &self,
        _context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        com_guard("OnKeyUp", || {
            self.track_key_up(key.0);
            Ok(false.into())
        })
    }
    fn OnPreservedKey(&self, context: Ref<ITfContext>, guid: *const GUID) -> Result<BOOL> {
        com_guard("OnPreservedKey", || {
            // Not in the editor process: the box handles these two shortcuts itself and keeps its code
            // and draft.
            if running_in_editor() || guid.is_null() {
                return Ok(false.into());
            }
            let guid = unsafe { *guid };
            if guid == EDITOR_KEY {
                log("保留键：Ctrl+Alt+T 打开草稿框");
                self.inline.cancel(self.client_id.get());
                if let Some(context) = context.as_ref() {
                    self.probe_new_field(context);
                    let opened = self.open_editor(context.clone(), 0);
                    if !opened {
                        log("保留键：草稿框未能打开");
                    }
                    return Ok(opened.into());
                }
                log("保留键：没有焦点上下文，草稿框未打开");
                return Ok(false.into());
            }
            if guid == TOGGLE_KEY {
                self.inline.cancel(self.client_id.get());
            }
            if guid == PLAIN_KEY {
                log("保留键：切换普通五笔/翻译模式");
            }
            if guid == HISTORY_KEY {
                // Passed through to the application when history is off, a session is active, in
                // English mode or in a password field.
                self.refresh_editor();
                let Some(context) = context.as_ref() else {
                    return Ok(false.into());
                };
                if !option(Flag::HistoryEnabled)
                    || self.buffering()
                    || !self.keyboard_open()
                    || TextService::disabled(context)
                    || self.es_password()
                    || self.known_private(context)
                {
                    return Ok(false.into());
                }
                self.probe_new_field(context);
                return Ok(self.open_editor(context.clone(), RECALL).into());
            }
            let toggled = (guid == TOGGLE_KEY && self.toggle_keyboard_mode())
                || (guid == PLAIN_KEY && self.toggle_plain_mode())
                || (guid == PUNCT_KEY && toggle_option(Flag::PunctEnglish))
                || (guid == TRAD_KEY && toggle_option(Flag::Traditional));
            if toggled {
                self.refresh_mode_button();
            }
            Ok(toggled.into())
        })
    }
}
