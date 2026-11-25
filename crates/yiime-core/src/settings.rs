//! Settings model. Switches, shortcuts and page size go through [`OptionStore`]
//! (`HKCU\Software\YiIME` on Windows, shared by the TSF DLL and the EXE); API settings
//! live in settings.json, and keys are encrypted with [`Sealer`] and stored separately (DPAPI on
//! Windows).

use crate::hotkey::{DEFAULT_PLAIN_TOGGLE, parse_hotkey};
use crate::shuangpin::Shuangpin;
use crate::style::TranslationStyle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

pub const DEFAULT_URL: &str = "https://api.openai.com/v1/chat/completions";
pub const DEFAULT_HISTORY_KEY: &str = "Ctrl+Shift+Z";
/// Default and allowed range of candidates per page.
pub const PAGE_SIZE: usize = 9;
pub const PAGE_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=9;
pub const SETTINGS_FILE: &str = "settings.json";
/// Encrypted API key file.
pub const KEY_FILE: &str = "key.dpapi";
/// User words: `word code` per line, ranked first under that code.
pub const USER_WORDS_FILE: &str = "user_words.txt";

/// Boolean switches. Storage names and defaults are shared by the TSF DLL and the EXE and defined
/// only here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// Plain Wubi mode: no translation, picks commit directly.
    PlainMode,
    PunctEnglish,
    Traditional,
    AutoCommit4,
    PushFifth,
    SemicolonSelect,
    Learn,
    HistoryEnabled,
    FuzzyPinyin,
    PinyinFirst,
    /// `,` `.` also page while composing (off by default: in Wubi they usually commit the first
    /// candidate).
    CommaPeriodPaging,
    /// `[` `]` pick a character from a word (on by default; with a single-character candidate they
    /// stay punctuation).
    BracketCharPick,
    /// Auto-close Chinese quotes, brackets and title marks (off by default).
    AutoPair,
    /// Show only common characters: no CJK extension/compatibility characters, no rare initials
    /// matches (on by default).
    CommonOnly,
}

impl Flag {
    pub const ALL: [Self; 14] = [
        Self::PlainMode,
        Self::PunctEnglish,
        Self::Traditional,
        Self::AutoCommit4,
        Self::PushFifth,
        Self::SemicolonSelect,
        Self::Learn,
        Self::HistoryEnabled,
        Self::FuzzyPinyin,
        Self::PinyinFirst,
        Self::CommaPeriodPaging,
        Self::BracketCharPick,
        Self::AutoPair,
        Self::CommonOnly,
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
            Self::PinyinFirst => "PinyinFirst",
            Self::CommaPeriodPaging => "CommaPeriodPaging",
            Self::BracketCharPick => "BracketCharPick",
            Self::AutoPair => "AutoPair",
            Self::CommonOnly => "CommonOnly",
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
                | Self::CommonOnly
        )
    }
}

/// 译/普通五笔 mode as kept in a session-wide shared cell next to the registry. Packaged desktop
/// apps such as Win11 Notepad get a private copy of HKCU writes, so a switch made in another
/// process (the language bar on the taskbar runs in explorer.exe) never reaches them through the
/// registry, and their own switches never leave. The cell carries the mode between all of them.
pub mod plain_cell {
    /// No process has published the mode yet.
    pub const UNSET: u32 = 0;
    const TRANSLATE: u32 = 1;
    const PLAIN: u32 = 2;
    /// Written by a host whose registry writes are private; the next ordinary process stores it
    /// in HKCU.
    const PENDING: u32 = 0x100;

    pub fn encode(plain: bool, private_registry: bool) -> u32 {
        (if plain { PLAIN } else { TRANSLATE }) | if private_registry { PENDING } else { 0 }
    }
    /// The mode in a cell, or None when unset or unreadable.
    pub fn decode(cell: u32) -> Option<bool> {
        match cell & !PENDING {
            TRANSLATE => Some(false),
            PLAIN => Some(true),
            _ => None,
        }
    }
    /// Result of reading the mode: the mode, the cell value to store (if it changes) and whether
    /// the registry should take the cell's mode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Read {
        pub plain: bool,
        pub store: Option<u32>,
        pub adopt: bool,
    }
    /// Mode from the cell and this process's registry view. An ordinary process trusts HKCU and
    /// publishes it, except for a pending switch from a private-registry host, which it adopts. A
    /// private-registry host trusts the cell once someone has published it.
    pub fn read(cell: u32, registry: bool, private_registry: bool) -> Read {
        let shared = decode(cell);
        if private_registry {
            return Read {
                plain: shared.unwrap_or(registry),
                store: None,
                adopt: false,
            };
        }
        match shared {
            Some(plain) if cell & PENDING != 0 => Read {
                plain,
                store: Some(encode(plain, false)),
                adopt: true,
            },
            _ => Read {
                plain: registry,
                store: (cell != encode(registry, false)).then(|| encode(registry, false)),
                adopt: false,
            },
        }
    }
}

/// Key-value storage (DWORDs and short strings). Implementations handle the platform; reads return
/// None on failure.
pub trait OptionStore {
    fn get_u32(&self, name: &str) -> Option<u32>;
    fn set_u32(&self, name: &str, value: u32) -> bool;
    fn get_string(&self, name: &str) -> Option<String>;
    fn set_string(&self, name: &str, value: &str) -> bool;
}

/// Typed settings. Storage is read on every access, so changes from another process (the TSF DLL)
/// apply immediately.
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
    /// Flips a switch and returns the new value (None if the write fails).
    pub fn toggle(&self, flag: Flag) -> Option<bool> {
        let on = !self.flag(flag);
        self.set_flag(flag, on).then_some(on)
    }
    /// Candidates per page, clamped to the allowed range; the default when missing.
    pub fn page_size(&self) -> usize {
        self.0.get_u32("PageSize").map_or(PAGE_SIZE, |value| {
            (value as usize).clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end())
        })
    }
    pub fn set_page_size(&self, size: usize) -> bool {
        let size = size.clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end());
        self.0.set_u32("PageSize", size as u32)
    }
    /// Shuangpin layout (`Shuangpin` string: `xiaohe`, `microsoft`, `ziranma`); None (the default)
    /// is Wubi + full pinyin.
    pub fn shuangpin(&self) -> Option<Shuangpin> {
        self.0
            .get_string("Shuangpin")
            .and_then(|id| Shuangpin::from_id(&id))
    }
    pub fn set_shuangpin(&self, scheme: Option<Shuangpin>) -> bool {
        self.0
            .set_string("Shuangpin", scheme.map_or("", Shuangpin::id))
    }
    /// Shortcut for toggling plain/translation mode; the default when unset or invalid.
    pub fn plain_toggle_key(&self) -> String {
        self.hotkey("PlainToggleKey", DEFAULT_PLAIN_TOGGLE)
    }
    pub fn set_plain_toggle_key(&self, text: &str) -> bool {
        self.0.set_string("PlainToggleKey", text)
    }
    /// Shortcut for input history; the default when unset or invalid.
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

/// Parses the page-size field in the settings window: an error if not a number, otherwise clamped
/// to 1–9.
pub fn parse_page_size(text: &str) -> Result<usize, &'static str> {
    text.trim()
        .parse::<usize>()
        .map(|value| value.clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end()))
        .map_err(|_| "每页候选数请填 1～9 的数字")
}

/// In-memory storage (tests and the CLI).
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

/// Encryption (per-user DPAPI on Windows). Returns None on failure.
pub trait Sealer {
    fn seal(&self, data: &[u8]) -> Option<Vec<u8>>;
    fn open(&self, data: &[u8]) -> Option<Vec<u8>>;
}

/// No encryption, for tests and the CLI; never for real keys.
pub struct PlainSealer;

impl Sealer for PlainSealer {
    fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
        Some(data.to_vec())
    }
    fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
        Some(data.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSettings {
    /// Translation engine; Azure by default (free Google until an Azure key is set).
    pub engine: Engine,
    /// When the engine fails, try another service (the configured LLM, or free Google after
    /// Azure). Off by default: text is only ever sent to the service the user picked.
    pub allow_fallback: bool,
    pub url: String,
    pub model: String,
    pub key: String,
    pub style: TranslationStyle,
    pub custom: String,
    /// Google Cloud Translation v2 API key (stored encrypted in [`GOOGLE_KEY_FILE`]).
    pub google_key: String,
    /// Azure Translator key (stored encrypted in [`AZURE_KEY_FILE`]) and resource region.
    pub azure_key: String,
    pub azure_region: String,
    /// DeepL API key (stored encrypted in `DEEPL_KEY_FILE`). A key ending in `:fx` uses the free
    /// endpoint.
    pub deepl_key: String,
    /// Loading the encrypted bundle failed. Preserve its ciphertext on unrelated saves.
    pub keys_unreadable: bool,
}

/// Encrypted Google Cloud key file (separate from the LLM key).
pub const GOOGLE_KEY_FILE: &str = "google.dpapi";
/// Encrypted Azure Translator key file.
pub const AZURE_KEY_FILE: &str = "azure.dpapi";
/// Encrypted DeepL API key file.
pub const DEEPL_KEY_FILE: &str = "deepl.dpapi";

/// Translation engine; `id` is what settings.json stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Engine {
    /// Azure Translator (F0 free tier); without a key, free Google is used.
    #[default]
    Azure,
    /// Free translate.googleapis.com endpoint (no key, unofficial, may be rate-limited).
    GoogleFree,
    /// Google Cloud Translation v2 (API key required).
    GoogleCloud,
    /// DeepL API (`EN-US`). A key ending in `:fx` uses the free endpoint.
    DeepL,
    /// OpenAI-compatible chat completions API (supports translation style and custom instructions).
    Llm,
}

impl Engine {
    pub const ALL: [Self; 5] = [
        Self::Azure,
        Self::GoogleFree,
        Self::GoogleCloud,
        Self::DeepL,
        Self::Llm,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Azure => "azure",
            Self::GoogleFree => "google_free",
            Self::GoogleCloud => "google_cloud",
            Self::DeepL => "deepl",
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
            Self::Azure => "Azure 翻译（F0 免费，默认）",
            Self::GoogleFree => "Google 翻译（免费）",
            Self::GoogleCloud => "Google Cloud 翻译（需密钥）",
            Self::DeepL => "DeepL（美式英文，需密钥）",
            Self::Llm => "大模型（OpenAI 兼容）",
        }
    }
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            allow_fallback: false,
            url: DEFAULT_URL.to_owned(),
            model: String::new(),
            key: String::new(),
            style: TranslationStyle::default(),
            custom: String::new(),
            google_key: String::new(),
            azure_key: String::new(),
            azure_region: String::new(),
            deepl_key: String::new(),
            keys_unreadable: false,
        }
    }
}

fn open_key_bundle(blob: &serde_json::Value, sealer: &dyn Sealer) -> Option<[String; 4]> {
    let bytes = serde_json::from_value::<Vec<u8>>(blob.clone()).ok()?;
    serde_json::from_slice(&sealer.open(&bytes)?).ok()
}

impl ApiSettings {
    /// Reads settings.json and the encrypted keys under `dir`; missing fields take defaults.
    pub fn load(dir: &Path, sealer: &dyn Sealer) -> Self {
        let mut settings = Self::default();
        let value = match std::fs::read(dir.join(SETTINGS_FILE)) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => Some(value),
                Err(_) => {
                    settings.keys_unreadable = true;
                    return settings;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                settings.keys_unreadable = true;
                return settings;
            }
        };
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
            if let Some(text) = value["azure_region"].as_str() {
                settings.azure_region = text.to_owned();
            }
            if let Some(text) = value["engine"].as_str() {
                settings.engine = Engine::from_id(text);
            }
            if let Some(on) = value["allow_fallback"].as_bool() {
                settings.allow_fallback = on;
            }
            if let Some(blob) = value.get("sealed_keys") {
                if let Some([key, google, azure, deepl]) = open_key_bundle(blob, sealer) {
                    settings.key = key;
                    settings.google_key = google;
                    settings.azure_key = azure;
                    settings.deepl_key = deepl;
                } else {
                    settings.keys_unreadable = true;
                }
                return settings;
            }
        }
        for (file, key) in [
            (KEY_FILE, &mut settings.key),
            (GOOGLE_KEY_FILE, &mut settings.google_key),
            (AZURE_KEY_FILE, &mut settings.azure_key),
            (DEEPL_KEY_FILE, &mut settings.deepl_key),
        ] {
            match std::fs::read(dir.join(file)) {
                Ok(bytes) => match sealer.open(&bytes).and_then(|v| String::from_utf8(v).ok()) {
                    Some(clear) => *key = clear,
                    None => settings.keys_unreadable = true,
                },
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    settings.keys_unreadable = true
                }
                Err(_) => {}
            }
        }
        settings
    }
    pub fn save(&self, dir: &Path, sealer: &dyn Sealer) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let keys = serde_json::to_vec(&[
            self.key.as_str(),
            self.google_key.trim(),
            self.azure_key.trim(),
            self.deepl_key.trim(),
        ])?;
        let file = dir.join(SETTINGS_FILE);
        let _lock = crate::storage::lock(&file)?;
        let old: serde_json::Value = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
            Err(e) => return Err(e),
        };
        if old.get("sealed_keys").is_none() {
            if self.keys_unreadable {
                return Err(std::io::Error::other("请重新打开设置后再迁移原密钥"));
            }
            // A failed migration must leave the legacy key files intact.
            for name in [KEY_FILE, GOOGLE_KEY_FILE, AZURE_KEY_FILE, DEEPL_KEY_FILE] {
                match std::fs::read(dir.join(name)) {
                    Ok(bytes)
                        if sealer
                            .open(&bytes)
                            .and_then(|v| String::from_utf8(v).ok())
                            .is_none() =>
                    {
                        return Err(std::io::Error::other("原密钥无法读取，未迁移设置"));
                    }
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
        }
        let encrypted = match old.get("sealed_keys") {
            Some(blob) if self.keys_unreadable || open_key_bundle(blob, sealer).is_none() => {
                if [
                    &self.key,
                    &self.azure_key,
                    &self.google_key,
                    &self.deepl_key,
                ]
                .iter()
                .any(|k| !k.is_empty())
                    || old["url"].as_str() != Some(self.url.as_str())
                {
                    return Err(std::io::Error::other(
                        "原密钥暂时无法解密，已保留原设置；请恢复密钥后再更改密钥或 API 地址",
                    ));
                }
                blob.clone()
            }
            _ => serde_json::to_value(
                sealer
                    .seal(&keys)
                    .ok_or_else(|| std::io::Error::other("密钥加密失败"))?,
            )?,
        };
        let snapshot = serde_json::to_vec(&serde_json::json!({
            "sealed_keys": encrypted,
            "url": self.url,
            "model": self.model,
            "style": self.style.id(),
            "custom": self.custom,
            "engine": self.engine.id(),
            "allow_fallback": self.allow_fallback,
            "azure_region": self.azure_region.trim(),
        }))?;
        if !crate::write_atomically(&dir.join(SETTINGS_FILE), &snapshot) {
            return Err(std::io::Error::other("设置替换失败"));
        }
        for legacy in [KEY_FILE, GOOGLE_KEY_FILE, AZURE_KEY_FILE, DEEPL_KEY_FILE] {
            let _ = std::fs::remove_file(dir.join(legacy));
        }
        Ok(())
    }
    /// Checks before sending a request. Free Google needs nothing; Google Cloud needs a key; see
    /// [`Self::validate_llm`] for the LLM.
    pub(crate) fn check_key_access(&self) -> Result<(), &'static str> {
        if self.keys_unreadable && self.engine != Engine::GoogleFree {
            return Err("翻译密钥暂时无法读取，已保留原文");
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        self.check_key_access()?;
        match self.engine {
            Engine::Azure | Engine::GoogleFree => Ok(()),
            Engine::GoogleCloud if self.google_key.trim().is_empty() => {
                Err("请先在“设置”中填写 Google Cloud 翻译的 API 密钥")
            }
            Engine::GoogleCloud => Ok(()),
            Engine::DeepL if self.deepl_key.trim().is_empty() => {
                Err("请先在“设置”中填写 DeepL 的 API 密钥")
            }
            Engine::DeepL => Ok(()),
            Engine::Llm => self.validate_llm(),
        }
    }
    /// LLM usable: URL, model and key are set; HTTPS only, except HTTP to this machine for
    /// testing (see [`llm_url_allowed`]).
    pub fn validate_llm(&self) -> Result<(), &'static str> {
        if self.url.trim().is_empty() || self.model.trim().is_empty() || self.key.trim().is_empty()
        {
            Err("请先在“设置”中填写 API 地址、模型和密钥")
        } else if !llm_url_allowed(&self.url) {
            Err("API 地址请使用 HTTPS（不能带用户名或密码）；仅本机测试允许 HTTP")
        } else {
            Ok(())
        }
    }
}

/// Whether the API key may be sent to `address`: parsed as a URL (not prefix-matched, so
/// `http://localhost:80@evil.example/` is the host `evil.example`). `https://` with a host is
/// allowed; `http://` only when the host really is this machine (`localhost`, 127.0.0.0/8 or
/// `[::1]`). Addresses with a user name or password are refused either way.
pub fn llm_url_allowed(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address.trim()) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain(host))) => host.eq_ignore_ascii_case("localhost"),
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_cell_carries_mode_across_private_registries() {
        use super::plain_cell::{UNSET, encode, read};
        // An ordinary process publishes HKCU; a packaged host follows the cell, not its copy.
        let first = read(UNSET, true, false);
        assert_eq!((first.plain, first.adopt), (true, false));
        let cell = first.store.unwrap_or(UNSET);
        assert!(read(cell, false, true).plain);
        assert!(!read(UNSET, false, true).plain);
        // Same value: nothing to store.
        assert_eq!(read(cell, true, false).store, None);
        // The language bar in explorer.exe switches to 译 (registry and cell).
        let cell = encode(false, false);
        assert!(!read(cell, true, true).plain);
        // A packaged host switches to 普通五笔: the next ordinary reader adopts it into HKCU.
        let cell = encode(true, true);
        let adopted = read(cell, false, false);
        assert_eq!(
            (adopted.plain, adopted.adopt, adopted.store),
            (true, true, Some(encode(true, false)))
        );
        // Stale or unknown cells give way to HKCU in ordinary processes.
        assert!(!read(0x7, false, false).plain);
        assert_eq!(read(0x7, false, false).store, Some(encode(false, false)));
    }
    #[test]
    fn llm_url_is_parsed_not_prefix_matched() {
        for good in [
            "https://api.example.com/v1",
            "HTTPS://API.EXAMPLE.COM/v1",
            "http://localhost:8080/v1",
            "http://LOCALHOST/v1",
            "http://127.0.0.1:11434/v1",
            "http://127.1.2.3:8080/v1",
            "http://[::1]:8765/v1",
            "  http://localhost:1234/v1  ",
        ] {
            assert!(llm_url_allowed(good), "{good}");
        }
        for bad in [
            // Prefix tricks: user info hides the real host.
            "http://localhost:80@evil.example/v1",
            "http://127.0.0.1:80@evil.example/v1",
            "http://[::1]:80@evil.example/v1",
            "http://localhost@evil.example/v1",
            "http://user:pass@localhost:8080/v1",
            "https://user:pass@api.example.com/v1",
            // Lookalike hosts.
            "http://localhost.evil.com/v1",
            "http://localhost.evil.com:8080/v1",
            "http://127.0.0.1.evil.com/v1",
            "http://evil.com/localhost/v1",
            "http://evil.com?http://localhost:1/",
            "http://[::ffff:8.8.8.8]/v1",
            "http://0.0.0.0:8080/v1",
            "http://10.0.0.1:8080/v1",
            "http://example.com",
            // Other schemes and junk.
            "ftp://localhost/v1",
            "file:///etc/passwd",
            "localhost:8080/v1",
            "https://",
            "",
        ] {
            assert!(!llm_url_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn flags_defaults_page_size_and_hotkeys() {
        let options = Options(MemoryStore::default());
        let defaults: Vec<_> = Flag::ALL.iter().map(|flag| options.flag(*flag)).collect();
        assert_eq!(
            defaults,
            [
                false, false, false, true, true, true, false, true, false, false, false, true,
                false, true
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
    fn save_after_transient_key_failure_preserves_all_keys_and_endpoint() {
        struct Locked;
        impl Sealer for Locked {
            fn seal(&self, _: &[u8]) -> Option<Vec<u8>> {
                None
            }
            fn open(&self, _: &[u8]) -> Option<Vec<u8>> {
                None
            }
        }
        let dir = std::env::temp_dir().join(format!("yiime-key-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = ApiSettings {
            key: "test-only".into(),
            ..ApiSettings::default()
        };
        original.save(&dir, &PlainSealer).unwrap();
        let mut draft = ApiSettings::load(&dir, &Locked);
        assert!(draft.keys_unreadable);
        draft.custom = "another style".into();
        draft.save(&dir, &PlainSealer).unwrap();
        let roundtrip = ApiSettings::load(&dir, &PlainSealer);
        assert_eq!(roundtrip.key, original.key);
        assert_eq!(roundtrip.custom, draft.custom);
        draft.url.push_str("/different");
        assert!(draft.save(&dir, &PlainSealer).is_err());
        assert_eq!(ApiSettings::load(&dir, &PlainSealer), roundtrip);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_settings_json_does_not_look_like_missing_keys() {
        let dir = std::env::temp_dir().join(format!("yiime-bad-json-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let broken = b"{not json";
        std::fs::write(dir.join(SETTINGS_FILE), broken).unwrap();
        let loaded = ApiSettings::load(&dir, &PlainSealer);
        assert!(loaded.keys_unreadable);
        assert!(loaded.check_key_access().is_err());
        let mut draft = loaded;
        draft.custom = "x".into();
        assert!(draft.save(&dir, &PlainSealer).is_err());
        assert_eq!(std::fs::read(dir.join(SETTINGS_FILE)).unwrap(), broken);
        std::fs::remove_dir_all(dir).unwrap();
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
            allow_fallback: true,
            url: "https://example.com/v1".into(),
            model: "m".into(),
            key: "secret".into(),
            style: TranslationStyle::Academic,
            custom: "x".into(),
            google_key: "gkey".into(),
            azure_key: "akey".into(),
            azure_region: "eastus".into(),
            deepl_key: "dkey:fx".into(),
            keys_unreadable: false,
        };
        settings.save(&dir, &Xor).unwrap();
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
        assert_eq!(ApiSettings::default().engine, Engine::Azure);
        assert!(ApiSettings::default().validate().is_ok());
        let cloud = ApiSettings {
            engine: Engine::GoogleCloud,
            ..ApiSettings::default()
        };
        assert!(cloud.validate().is_err());
        assert!(json.contains("\"engine\":\"llm\""));
        let mut cleared = settings.clone();
        cleared.google_key.clear();
        cleared.deepl_key.clear();
        cleared.save(&dir, &Xor).unwrap();
        assert!(!dir.join(GOOGLE_KEY_FILE).exists());
        assert!(!dir.join(DEEPL_KEY_FILE).exists());
        let deepl = ApiSettings {
            engine: Engine::DeepL,
            ..ApiSettings::default()
        };
        assert!(deepl.validate().is_err());
        let deepl = ApiSettings {
            engine: Engine::DeepL,
            deepl_key: "k".into(),
            ..ApiSettings::default()
        };
        assert!(deepl.validate().is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
