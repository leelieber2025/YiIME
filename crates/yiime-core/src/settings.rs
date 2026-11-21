//! Set up the model. Storage is through the trait: switch/shortcut key/number of candidates per
//! page [`OptionStore`] (`HKCU\Software\InputTranslation` on Windows, shared by TSF DLL and EXE),
//! API configuration is written in settings.json, and the key is encrypted by [`Sealer`] and stored
//! separately (DPAPI on Windows).

use crate::hotkey::{DEFAULT_PLAIN_TOGGLE, parse_hotkey};
use crate::style::TranslationStyle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

pub const DEFAULT_URL: &str = "https://api.openai.com/v1/chat/completions";
pub const DEFAULT_HISTORY_KEY: &str = "Ctrl+Shift+Z";
/// Default value and allowed range for the number of candidates per page.
pub const PAGE_SIZE: usize = 9;
pub const PAGE_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=9;
pub const SETTINGS_FILE: &str = "settings.json";
/// The encrypted API key file name (the file name from v11 will be used and will be read as usual
/// after the upgrade).
pub const KEY_FILE: &str = "key.dpapi";
/// User thesaurus: Each line of "word code" is ranked at the top of the code.
pub const USER_WORDS_FILE: &str = "user_words.txt";

/// Boolean switch: storage name and default value are conventions between TSF DLL and EXE and are
/// only defined here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// Ordinary Wubi mode (no translation, word selection goes directly to the screen).
    PlainMode,
    PunctEnglish,
    Traditional,
    AutoCommit4,
    PushFifth,
    SemicolonSelect,
    Learn,
    HistoryEnabled,
    /// Fuzzy sound (off by default).
    FuzzyPinyin,
    /// When composing characters, `,` `.` also turns the page (default off: Wubi commonly used
    /// commas and periods top the screen).
    CommaPeriodPaging,
    /// `[` `]` Determine characters based on words (on by default, single character candidates are
    /// still punctuation marks).
    BracketCharPick,
    /// Chinese quotation marks, brackets, and book title numbers are automatically matched (off by
    /// default).
    AutoPair,
}

impl Flag {
    pub const ALL: [Self; 12] = [
        Self::PlainMode,
        Self::PunctEnglish,
        Self::Traditional,
        Self::AutoCommit4,
        Self::PushFifth,
        Self::SemicolonSelect,
        Self::Learn,
        Self::HistoryEnabled,
        Self::FuzzyPinyin,
        Self::CommaPeriodPaging,
        Self::BracketCharPick,
        Self::AutoPair,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::PlainMode => "PlainMode",
            Self::PunctEnglish => "PunctEnglish",
            Self::Traditional => "Traditional",
            Self::AutoCommit4 => "AutoCommit4",
            Self::PushFifth => "PushFifth",
            Self::SemicolonSelect => "SemicolonSelect",
            Self::Learn => "Learn",
            Self::HistoryEnabled => "HistoryEnabled",
            Self::FuzzyPinyin => "FuzzyPinyin",
            Self::CommaPeriodPaging => "CommaPeriodPaging",
            Self::BracketCharPick => "BracketCharPick",
            Self::AutoPair => "AutoPair",
        }
    }
    pub fn default_on(self) -> bool {
        matches!(
            self,
            Self::AutoCommit4
                | Self::PushFifth
                | Self::SemicolonSelect
                | Self::HistoryEnabled
                | Self::BracketCharPick
        )
    }
}

/// Key-value storage (DWORD and short strings). The implementer is responsible for the platform
/// details, and None will be returned if read fails.
pub trait OptionStore {
    fn get_u32(&self, name: &str) -> Option<u32>;
    fn set_u32(&self, name: &str, value: u32) -> bool;
    fn get_string(&self, name: &str) -> Option<String>;
    fn set_string(&self, name: &str, value: &str) -> bool;
}

/// Typed settings are read and written; the storage is read every time, so changes made by another
/// process (TSF DLL) can be seen immediately.
pub struct Options<S>(pub S);

impl<S: OptionStore> Options<S> {
    pub fn flag(&self, flag: Flag) -> bool {
        self.0
            .get_u32(flag.name())
            .map_or(flag.default_on(), |value| value != 0)
    }
    pub fn set_flag(&self, flag: Flag, on: bool) -> bool {
        self.0.set_u32(flag.name(), on as u32)
    }
    /// Flips the switch; returns the new value (None if the write fails).
    pub fn toggle(&self, flag: Flag) -> Option<bool> {
        let on = !self.flag(flag);
        self.set_flag(flag, on).then_some(on)
    }
    /// Number of candidates per page (out-of-range values ​​are clamped into the range, default
    /// values ​​when missing).
    pub fn page_size(&self) -> usize {
        self.0.get_u32("PageSize").map_or(PAGE_SIZE, |value| {
            (value as usize).clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end())
        })
    }
    pub fn set_page_size(&self, size: usize) -> bool {
        let size = size.clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end());
        self.0.set_u32("PageSize", size as u32)
    }
    /// Shortcut text for switching normal/translation mode; if not set or invalid, the default
    /// value is used.
    pub fn plain_toggle_key(&self) -> String {
        self.hotkey("PlainToggleKey", DEFAULT_PLAIN_TOGGLE)
    }
    pub fn set_plain_toggle_key(&self, text: &str) -> bool {
        self.0.set_string("PlainToggleKey", text)
    }
    /// Enter the memory shortcut text; use the default value if it is not set or is invalid.
    pub fn history_key(&self) -> String {
        self.hotkey("HistoryKey", DEFAULT_HISTORY_KEY)
    }
    pub fn set_history_key(&self, text: &str) -> bool {
        self.0.set_string("HistoryKey", text)
    }
    fn hotkey(&self, name: &str, default: &str) -> String {
        self.0
            .get_string(name)
            .filter(|text| parse_hotkey(text).is_some())
            .unwrap_or_else(|| default.to_owned())
    }
}

/// "Number of candidates per page" input box in the settings window: an error will be reported if
/// it is not a number, and the number will be clamped to 1–9.
pub fn parse_page_size(text: &str) -> Result<usize, &'static str> {
    text.trim()
        .parse::<usize>()
        .map(|value| value.clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end()))
        .map_err(|_| "每页候选数请填 1～9 的数字")
}

/// In-memory storage (for testing and headless CLI).
#[derive(Default)]
pub struct MemoryStore {
    numbers: RefCell<HashMap<String, u32>>,
    strings: RefCell<HashMap<String, String>>,
}

impl OptionStore for MemoryStore {
    fn get_u32(&self, name: &str) -> Option<u32> {
        self.numbers.borrow().get(name).copied()
    }
    fn set_u32(&self, name: &str, value: u32) -> bool {
        self.numbers.borrow_mut().insert(name.to_owned(), value);
        true
    }
    fn get_string(&self, name: &str) -> Option<String> {
        self.strings.borrow().get(name).cloned()
    }
    fn set_string(&self, name: &str, value: &str) -> bool {
        self.strings
            .borrow_mut()
            .insert(name.to_owned(), value.to_owned());
        true
    }
}

/// Encryption/decryption (per current user's DPAPI on Windows). Returns None on failure.
pub trait Sealer {
    fn seal(&self, data: &[u8]) -> Option<Vec<u8>>;
    fn open(&self, data: &[u8]) -> Option<Vec<u8>>;
}

/// No encryption (for testing with the headless CLI; do not use for real keys).
pub struct PlainSealer;

impl Sealer for PlainSealer {
    fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
        Some(data.to_vec())
    }
    fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
        Some(data.to_vec())
    }
}

/// Translate API configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSettings {
    /// Translation engine; the default is free Google Translate. If a large model is configured
    /// when it fails, it will automatically switch to the large model.
    pub engine: Engine,
    pub url: String,
    pub model: String,
    pub key: String,
    pub style: TranslationStyle,
    pub custom: String,
    /// API key for Google Cloud Translation v2 (stored encrypted at [`GOOGLE_KEY_FILE`]).
    pub google_key: String,
}

/// The encrypted file name of the Google Cloud key (stored separately from the large model key).
pub const GOOGLE_KEY_FILE: &str = "google.dpapi";

/// Translation engine. `id` is written to settings.json.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Engine {
    /// Free interface of translate.googleapis.com (no key required, unofficial, may be limited).
    #[default]
    GoogleFree,
    /// Google Cloud Translation v2 (requires API key).
    GoogleCloud,
    /// OpenAI compatible large model interface (supports translation style and custom
    /// requirements).
    Llm,
}

impl Engine {
    pub const ALL: [Self; 3] = [Self::GoogleFree, Self::GoogleCloud, Self::Llm];
    pub fn id(self) -> &'static str {
        match self {
            Self::GoogleFree => "google_free",
            Self::GoogleCloud => "google_cloud",
            Self::Llm => "llm",
        }
    }
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|engine| engine.id() == id)
            .unwrap_or_default()
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::GoogleFree => "Google 翻译（免费，默认）",
            Self::GoogleCloud => "Google Cloud 翻译（需密钥）",
            Self::Llm => "大模型（OpenAI 兼容）",
        }
    }
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            url: DEFAULT_URL.to_owned(),
            model: String::new(),
            key: String::new(),
            style: TranslationStyle::default(),
            custom: String::new(),
            google_key: String::new(),
        }
    }
}

impl ApiSettings {
    /// Read settings.json and encryption key under `dir`; take default values ​​for missing fields.
    pub fn load(dir: &Path, sealer: &dyn Sealer) -> Self {
        let mut settings = Self::default();
        let value = std::fs::read(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let Some(value) = value {
            if let Some(text) = value["url"].as_str() {
                settings.url = text.to_owned();
            }
            if let Some(text) = value["model"].as_str() {
                settings.model = text.to_owned();
            }
            if let Some(text) = value["style"].as_str() {
                settings.style = TranslationStyle::from_id(text);
            }
            if let Some(text) = value["custom"].as_str() {
                settings.custom = text.to_owned();
            }
            if let Some(text) = value["engine"].as_str() {
                settings.engine = Engine::from_id(text);
            }
        }
        let key = std::fs::read(dir.join(KEY_FILE))
            .ok()
            .and_then(|bytes| sealer.open(&bytes))
            .and_then(|clear| String::from_utf8(clear).ok());
        if let Some(text) = key {
            settings.key = text;
        }
        let google = std::fs::read(dir.join(GOOGLE_KEY_FILE))
            .ok()
            .and_then(|bytes| sealer.open(&bytes))
            .and_then(|clear| String::from_utf8(clear).ok());
        if let Some(text) = google {
            settings.google_key = text;
        }
        settings
    }
    pub fn save(&self, dir: &Path, sealer: &dyn Sealer) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let encrypted = sealer
            .seal(self.key.as_bytes())
            .ok_or_else(|| std::io::Error::other("密钥加密失败"))?;
        std::fs::write(dir.join(KEY_FILE), encrypted)?;
        let google = dir.join(GOOGLE_KEY_FILE);
        if self.google_key.trim().is_empty() {
            if google.exists() {
                std::fs::remove_file(&google)?;
            }
        } else {
            let sealed = sealer
                .seal(self.google_key.trim().as_bytes())
                .ok_or_else(|| std::io::Error::other("密钥加密失败"))?;
            std::fs::write(google, sealed)?;
        }
        std::fs::write(
            dir.join(SETTINGS_FILE),
            serde_json::to_vec(&serde_json::json!({
                "url": self.url,
                "model": self.model,
                "style": self.style.id(),
                "custom": self.custom,
                "engine": self.engine.id(),
            }))?,
        )
    }
    /// Check before sending request: Press the selected engine. Free Google requires no
    /// configuration; Google Cloud requires a key; see [`Self::validate_llm`] for a large model.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.engine {
            Engine::GoogleFree => Ok(()),
            Engine::GoogleCloud if self.google_key.trim().is_empty() => {
                Err("请先在“设置”中填写 Google Cloud 翻译的 API 密钥")
            }
            Engine::GoogleCloud => Ok(()),
            Engine::Llm => self.validate_llm(),
        }
    }
    /// Large models are available: address, model, and key are required; only HTTPS is allowed, and
    /// HTTP is available for the local test address.
    pub fn validate_llm(&self) -> Result<(), &'static str> {
        let lower = self.url.trim().to_ascii_lowercase();
        if self.url.trim().is_empty() || self.model.trim().is_empty() || self.key.trim().is_empty()
        {
            Err("请先在“设置”中填写 API 地址、模型和密钥")
        } else if !lower.starts_with("https://")
            && !lower.starts_with("http://127.0.0.1:")
            && !lower.starts_with("http://localhost:")
            && !lower.starts_with("http://[::1]:")
        {
            Err("API 地址请使用 HTTPS；仅本机测试允许 HTTP")
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_defaults_page_size_and_hotkeys() {
        let options = Options(MemoryStore::default());
        let defaults: Vec<_> = Flag::ALL.iter().map(|flag| options.flag(*flag)).collect();
        assert_eq!(
            defaults,
            [
                false, false, false, true, true, true, false, true, false, false, true, false
            ]
        );
        assert_eq!(options.toggle(Flag::Learn), Some(true));
        assert!(options.flag(Flag::Learn));
        assert_eq!(options.page_size(), 9);
        options.0.set_u32("PageSize", 15);
        assert_eq!(options.page_size(), 9);
        options.0.set_u32("PageSize", 0);
        assert_eq!(options.page_size(), 1);
        assert!(options.set_page_size(5));
        assert_eq!(options.page_size(), 5);
        assert_eq!(parse_page_size(" 12 "), Ok(9));
        assert_eq!(parse_page_size("0"), Ok(1));
        assert!(parse_page_size("x").is_err());
        assert_eq!(options.plain_toggle_key(), "Ctrl+Shift+Space");
        options.set_plain_toggle_key("Shift+Space"); // Invalid: return to default
        assert_eq!(options.plain_toggle_key(), "Ctrl+Shift+Space");
        options.set_history_key("Alt+H");
        assert_eq!(options.history_key(), "Alt+H");
    }

    #[test]
    fn api_settings_round_trip_and_validation() {
        struct Xor;
        impl Sealer for Xor {
            fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
                Some(data.iter().map(|b| b ^ 0x5A).collect())
            }
            fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
                self.seal(data)
            }
        }
        let dir = std::env::temp_dir().join(format!("yiime-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ApiSettings::load(&dir, &Xor), ApiSettings::default());
        let settings = ApiSettings {
            engine: Engine::Llm,
            url: "https://example.com/v1".into(),
            model: "m".into(),
            key: "secret".into(),
            style: TranslationStyle::Academic,
            custom: "x".into(),
            google_key: "gkey".into(),
        };
        settings.save(&dir, &Xor).unwrap();
        let raw = std::fs::read(dir.join(KEY_FILE)).unwrap();
        assert_ne!(raw, b"secret");
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!json.contains("secret") && json.contains("academic"));
        assert_eq!(ApiSettings::load(&dir, &Xor), settings);
        assert!(settings.validate().is_ok());
        let mut bad = settings.clone();
        bad.url = "http://example.com".into();
        assert!(bad.validate().is_err());
        bad.url = "http://localhost:8080/v1".into();
        assert!(bad.validate().is_ok());
        bad.key = " ".into();
        assert!(bad.validate().is_err());
        // The default engine is Free Google, which requires no configuration; Google Cloud requires
        // a key.
        assert_eq!(ApiSettings::default().engine, Engine::GoogleFree);
        assert!(ApiSettings::default().validate().is_ok());
        let cloud = ApiSettings {
            engine: Engine::GoogleCloud,
            ..ApiSettings::default()
        };
        assert!(cloud.validate().is_err());
        let raw = std::fs::read(dir.join(GOOGLE_KEY_FILE)).unwrap();
        assert_ne!(raw, b"gkey");
        assert!(json.contains("\"engine\":\"llm\""));
        let mut cleared = settings.clone();
        cleared.google_key.clear();
        cleared.save(&dir, &Xor).unwrap();
        assert!(!dir.join(GOOGLE_KEY_FILE).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
