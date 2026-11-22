use super::*;

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, foreground: BOOL) -> Result<()> {
        com_guard("OnSetFocus", || {
            self.tested_key.borrow_mut().take();
            self.shift_down.set(None);
            if !foreground.as_bool() {
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
            *self.tested_key.borrow_mut() = context
                .as_ref()
                .map(|context| (context.clone(), key.0, consume));
            Ok(consume.into())
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
            if !self.buffering()
                && !self.should_consume(context, key.0)
                && let Some(text) = self.punctuation(context, key.0)
            {
                if key.0 == 0xDE {
                    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
                    let mut quotes = self.quotes.get();
                    quotes.next(if shift { '"' } else { '\'' });
                    self.quotes.set(quotes);
                }
                self.after_digit.set(false);
                return Ok(self.insert_direct(context, text).into());
            }
            // Remember digits so a following `.` `:` stays half-width (3.5, 10:30).
            let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
            self.after_digit
                .set((!shift && (0x30..=0x39).contains(&key.0)) || (0x60..=0x69).contains(&key.0));
            // Only letters go to the box; keys whose conditions changed since OnTestKeyDown (e.g. eaten
            // by punctuation) are released.
            if !consume || !(0x41..=0x5a).contains(&key.0) {
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
            Ok(self.open_editor(context.clone(), key.0 as u8).into())
        })
    }
    fn OnTestKeyUp(
        &self,
        _context: Ref<ITfContext>,
        key: windows::Win32::Foundation::WPARAM,
        _data: windows::Win32::Foundation::LPARAM,
    ) -> Result<BOOL> {
        com_guard("OnTestKeyUp", || {
            // A Shift released without being eaten reaches the host as usual; the toggle happens once
            // (track_key_up clears the record).
            self.track_key_up(key.0);
            Ok(false.into())
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
                    || self.password_scope(context)
                {
                    return Ok(false.into());
                }
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
