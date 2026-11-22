#[derive(Default)]
pub struct Draft {
    text: String,
    cursor: usize,
    translated: Option<String>,
}

impl Draft {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn translation(&self) -> Option<&str> {
        self.translated.as_deref()
    }

    pub fn replace_text(&mut self, text: &str) {
        if self.text != text {
            self.text = text.to_owned();
            self.cursor = text.chars().count();
            self.translated = None;
        }
    }

    pub fn accept_translation(&mut self, source: &str, english: String) -> Result<(), String> {
        if source != self.text {
            return Err("中文草稿已改变".into());
        }
        if english.trim().is_empty() {
            return Err("译文为空".into());
        }
        self.translated = Some(english);
        Ok(())
    }

    pub fn insert(&mut self, text: &str) {
        let byte = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(at, _)| at);
        self.text.insert_str(byte, text);
        self.cursor += text.chars().count();
        self.translated = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((byte, _)) = self.text.char_indices().nth(self.cursor - 1) {
            self.text.remove(byte);
            self.cursor -= 1;
            self.translated = None;
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.text.chars().count());
    }

    pub fn translate_with(
        &mut self,
        translator: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("原文为空".into());
        }
        let english = translator(&self.text)?;
        let source = self.text.clone();
        self.accept_translation(&source, english)
    }

    pub fn take_english(&mut self) -> Option<String> {
        let english = self.translated.take()?;
        self.text.clear();
        self.cursor = 0;
        Some(english)
    }
}
