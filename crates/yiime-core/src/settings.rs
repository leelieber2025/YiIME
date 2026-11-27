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
/// Composition-window scale, in percent. 100 is the size the boxes have always used.
pub const UI_SCALE: u32 = 100;
pub const UI_SCALES: [u32; 5] = [100, 125, 150, 175, 200];
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

    /// Cell value to store after calling the caller's HKCU `adopt` callback. If `read.adopt` was
    /// set and `adopt` failed (`false`), do NOT clear `PENDING` so the next ordinary process retries.
    pub fn store_after_adopt(read: Read, adopted: bool) -> Option<u32> {
        if !read.adopt || adopted {
            read.store
        } else {
            None
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
    /// Scale of the Wubi candidate bar and the translation box. An unknown value is 100%.
    pub fn ui_scale(&self) -> u32 {
        self.0
            .get_u32("UiScale")
            .filter(|value| UI_SCALES.contains(value))
            .unwrap_or(UI_SCALE)
    }
    pub fn set_ui_scale(&self, percent: u32) -> bool {
        UI_SCALES.contains(&percent) && self.0.set_u32("UiScale", percent)
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

/// Monitor DPI after the composition-window scale. 100% returns `dpi` (at least 96).
/// Scale to write on save. `None` when the control is still the value this window opened with,
/// so a second settings window does not put an older percent back over a newer one.
pub fn ui_scale_to_write(opened: u32, selected: u32) -> Option<u32> {
    (selected != opened).then_some(selected)
}

pub fn scaled_dpi(dpi: u32, percent: u32) -> u32 {
    let percent = if UI_SCALES.contains(&percent) {
        percent
    } else {
        UI_SCALE
    };
    let dpi = dpi.max(96);
    (u64::from(dpi) * u64::from(percent) / 100).max(96) as u32
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

/// Maximum number of OpenAI-compatible model profiles (primary and backups).
pub const LLM_PROFILE_COUNT: usize = 3;

/// One OpenAI-compatible LLM endpoint. An all-empty profile is an unused slot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmProfile {
    pub url: String,
    pub model: String,
    pub key: String,
    /// When true, send `reasoning_effort: "none"` and drop it if the endpoint refuses (400/422).
    pub disable_thinking: bool,
}

impl LlmProfile {
    pub fn is_empty(&self) -> bool {
        // The default OpenAI URL is a placeholder in a fresh install. With no model or key it
        // is not a configured profile, so other settings can be saved without filling those in.
        self.model.trim().is_empty()
            && self.key.trim().is_empty()
            && (self.url.trim().is_empty() || self.url.trim() == DEFAULT_URL)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.is_empty() {
            return Ok(());
        }
        if self.url.trim().is_empty() || self.model.trim().is_empty() || self.key.trim().is_empty()
        {
            Err("请填写该模型的 API 地址、模型和密钥")
        } else if !llm_url_allowed(&self.url) {
            Err("API 地址请使用 HTTPS（不能带用户名或密码）；仅本机测试允许 HTTP")
        } else {
            Ok(())
        }
    }

    pub fn is_ready(&self) -> bool {
        self.validate().is_ok() && !self.is_empty()
    }
}

/// Default for a missing `disable_thinking` field: on for DeepInfra, off otherwise.
/// Existing DeepInfra setups therefore keep sending `reasoning_effort: "none"`.
pub fn default_disable_thinking(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint.trim()) else {
        return false;
    };
    let deepinfra = url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "deepinfra.com" || host.ends_with(".deepinfra.com")
    });
    deepinfra || (cfg!(test) && url.path().starts_with("/deepinfra-test/"))
}

/// Status line when translation used a later profile in the saved order (1-based index).
pub fn llm_backup_hint(slot: usize) -> Option<String> {
    (slot > 0).then(|| format!("已切换到备用模型 {}", slot + 1))
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
    /// Primary profile: send `reasoning_effort: "none"` when true.
    pub disable_thinking: bool,
    /// Optional backup profiles, in saved order after the primary (`url` / `model` / `key`).
    pub backup_llm: [LlmProfile; 2],
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
    /// settings.json exists but is not a JSON object (`[]`, `null`, a number…): nothing in it can
    /// be trusted, so translation stops instead of using the defaults, and the file is kept.
    pub settings_broken: bool,
    /// `engine` is present in settings.json with an unknown id or non-string value (e.g. a config
    /// from a newer version): stop instead of falling back to Azure / free Google.
    pub unknown_engine: bool,
}

/// settings.json is valid JSON but not an object: translation stops and the file is never
/// overwritten until the user restores it.
pub const SETTINGS_BROKEN: &str =
    "设置文件 settings.json 已损坏，已保留原文；请从备份恢复该文件或删除后重新设置";
/// `engine` in settings.json is present with an unknown id or wrong type: translation stops and
/// asks the user to pick a service again instead of sending text to an unselected service.
pub const UNKNOWN_ENGINE: &str =
    "设置里的翻译服务不可用（可能是新版本配置或格式错误），已保留原文；请在设置里重新选择翻译服务";

/// Encrypted Google Cloud key file (separate from the LLM key).
pub const GOOGLE_KEY_FILE: &str = "google.dpapi";
/// Encrypted Azure Translator key file.
pub const AZURE_KEY_FILE: &str = "azure.dpapi";
/// Encrypted DeepL API key file.
pub const DEEPL_KEY_FILE: &str = "deepl.dpapi";
/// Registry DWORD marker inside `HKCU\Software\YiIME` showing legacy 0.1.x values were migrated.
pub const LEGACY_MIGRATION_MARKER: &str = "MigratedFromInputTranslation";

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
    pub fn parse_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.id() == id)
    }
    pub fn from_id(id: &str) -> Self {
        Self::parse_id(id).unwrap_or_default()
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
            disable_thinking: default_disable_thinking(DEFAULT_URL),
            backup_llm: Default::default(),
            style: TranslationStyle::default(),
            custom: String::new(),
            google_key: String::new(),
            azure_key: String::new(),
            azure_region: String::new(),
            deepl_key: String::new(),
            keys_unreadable: false,
            settings_broken: false,
            unknown_engine: false,
        }
    }
}

fn open_key_bundle(blob: &serde_json::Value, sealer: &dyn Sealer) -> Option<[String; 6]> {
    let bytes = serde_json::from_value::<Vec<u8>>(blob.clone()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&sealer.open(&bytes)?).ok()?;
    let arr = value.as_array()?;
    // Legacy 0.2.5 bundles have exactly four strings; current bundles have six. Wrong lengths
    // or a non-string entry (null / number / object) are unreadable, so a later save will not
    // invent empty keys and overwrite the original ciphertext.
    if arr.len() != 4 && arr.len() != 6 {
        return None;
    }
    if !arr.iter().all(|item| item.is_string()) {
        return None;
    }
    let at = |i: usize| arr.get(i).and_then(|v| v.as_str()).unwrap_or("").to_owned();
    Some([at(0), at(1), at(2), at(3), at(4), at(5)])
}

/// Primary and backup LLM URLs as stored on disk (top-level `url`, then `llm_models`).
fn llm_urls_from_value(value: &serde_json::Value) -> [String; LLM_PROFILE_COUNT] {
    let mut urls = [
        value["url"].as_str().unwrap_or("").to_owned(),
        String::new(),
        String::new(),
    ];
    let Some(arr) = value.get("llm_models").and_then(|v| v.as_array()) else {
        return urls;
    };
    for (i, item) in arr.iter().take(LLM_PROFILE_COUNT).enumerate() {
        if !item.is_object() {
            continue;
        }
        if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
            urls[i] = url.to_owned();
        } else if i > 0 {
            urls[i].clear();
        }
    }
    urls
}

fn thinking_flag(value: &serde_json::Value, url: &str, key: &str) -> bool {
    value
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| default_disable_thinking(url))
}

impl ApiSettings {
    /// Reads settings.json and the encrypted keys under `dir`; missing fields take defaults.
    pub fn load(dir: &Path, sealer: &dyn Sealer) -> Self {
        let value = match std::fs::read(dir.join(SETTINGS_FILE)) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) if value.is_object() => Some(value),
                // Valid JSON with another root (`[]`, `null`): unlike a missing file (first run)
                // this is a damaged configuration; defaults would silently pick another service.
                Ok(_) => {
                    return Self {
                        keys_unreadable: true,
                        settings_broken: true,
                        ..Self::default()
                    };
                }
                Err(_) => {
                    return Self {
                        keys_unreadable: true,
                        ..Self::default()
                    };
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                return Self {
                    keys_unreadable: true,
                    ..Self::default()
                };
            }
        };
        Self::from_json_value(value.as_ref(), dir, sealer)
    }

    fn from_json_value(value: Option<&serde_json::Value>, dir: &Path, sealer: &dyn Sealer) -> Self {
        let mut settings = Self::default();
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
            match value.get("engine") {
                None => {} // Missing field (older config): default Azure applies.
                Some(v) => match v.as_str().and_then(Engine::parse_id) {
                    Some(engine) => settings.engine = engine,
                    None => settings.unknown_engine = true,
                },
            }
            if let Some(on) = value["allow_fallback"].as_bool() {
                settings.allow_fallback = on;
            }
            let mut backup_keys = [String::new(), String::new()];
            if let Some(blob) = value.get("sealed_keys") {
                if let Some([key, google, azure, deepl, backup1, backup2]) =
                    open_key_bundle(blob, sealer)
                {
                    settings.key = key;
                    settings.google_key = google;
                    settings.azure_key = azure;
                    settings.deepl_key = deepl;
                    backup_keys = [backup1, backup2];
                } else {
                    settings.keys_unreadable = true;
                }
                settings.apply_llm_fields(value, backup_keys);
                return settings;
            }
            // Legacy per-file keys below, then llm_models if present.
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
        if let Some(value) = value {
            settings.apply_llm_fields(value, Default::default());
        }
        settings
    }

    /// The three model profiles in saved order (slot 0 is also `url` / `model` / `key`).
    pub fn llm_profiles(&self) -> [LlmProfile; LLM_PROFILE_COUNT] {
        [
            LlmProfile {
                url: self.url.clone(),
                model: self.model.clone(),
                key: self.key.clone(),
                disable_thinking: self.disable_thinking,
            },
            self.backup_llm[0].clone(),
            self.backup_llm[1].clone(),
        ]
    }

    pub fn set_llm_profiles(&mut self, profiles: [LlmProfile; LLM_PROFILE_COUNT]) {
        self.url = profiles[0].url.clone();
        self.model = profiles[0].model.clone();
        self.key = profiles[0].key.clone();
        self.disable_thinking = profiles[0].disable_thinking;
        self.backup_llm = [profiles[1].clone(), profiles[2].clone()];
    }

    fn apply_llm_fields(&mut self, value: &serde_json::Value, backup_keys: [String; 2]) {
        self.disable_thinking = thinking_flag(value, &self.url, "disable_thinking");
        let Some(arr) = value.get("llm_models").and_then(|v| v.as_array()) else {
            self.backup_llm = Default::default();
            return;
        };
        let mut loaded = self.llm_profiles();
        for (i, item) in arr.iter().take(LLM_PROFILE_COUNT).enumerate() {
            if !item.is_object() {
                continue;
            }
            if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                loaded[i].url = url.to_owned();
            } else if i > 0 {
                loaded[i].url.clear();
            }
            if let Some(model) = item.get("model").and_then(|v| v.as_str()) {
                loaded[i].model = model.to_owned();
            } else if i > 0 {
                loaded[i].model.clear();
            }
            loaded[i].disable_thinking = thinking_flag(item, &loaded[i].url, "disable_thinking");
        }
        loaded[1].key = backup_keys[0].clone();
        loaded[2].key = backup_keys[1].clone();
        self.set_llm_profiles(loaded);
    }

    /// (url, model) identity of a configured profile. Unused slots do not match each other.
    fn llm_identity(profile: &LlmProfile) -> Option<(String, String)> {
        if profile.is_empty() {
            None
        } else {
            Some((
                profile.url.trim().to_owned(),
                profile.model.trim().to_owned(),
            ))
        }
    }

    fn pick_profile_field(cur: &str, orig: &str, on_disk: &str) -> String {
        if cur == orig {
            on_disk.to_owned()
        } else {
            cur.to_owned()
        }
    }

    fn merge_one_profile(cur: &LlmProfile, orig: &LlmProfile, on_disk: &LlmProfile) -> LlmProfile {
        LlmProfile {
            url: Self::pick_profile_field(&cur.url, &orig.url, &on_disk.url),
            model: Self::pick_profile_field(&cur.model, &orig.model, &on_disk.model),
            key: Self::pick_profile_field(&cur.key, &orig.key, &on_disk.key),
            disable_thinking: if cur.disable_thinking == orig.disable_thinking {
                on_disk.disable_thinking
            } else {
                cur.disable_thinking
            },
        }
    }

    /// Merge dialog edits with a concurrent save. `url` / `model` / `key` stay together: a key
    /// typed in one window is applied to the same (url, model) on disk, even if the other window
    /// reordered the list, instead of being written into a different slot.
    fn merge_llm_profiles(
        base: &[LlmProfile; LLM_PROFILE_COUNT],
        current: &[LlmProfile; LLM_PROFILE_COUNT],
        disk: &[LlmProfile; LLM_PROFILE_COUNT],
    ) -> [LlmProfile; LLM_PROFILE_COUNT] {
        if current == base {
            return disk.clone();
        }
        if disk == base {
            return current.clone();
        }
        let ids = |profiles: &[LlmProfile; LLM_PROFILE_COUNT]| {
            [
                Self::llm_identity(&profiles[0]),
                Self::llm_identity(&profiles[1]),
                Self::llm_identity(&profiles[2]),
            ]
        };
        let this_reordered = ids(current) != ids(base);
        let mut result = if this_reordered {
            current.clone()
        } else {
            disk.clone()
        };
        for i in 0..LLM_PROFILE_COUNT {
            let orig = &base[i];
            let cur = &current[i];
            if !this_reordered && cur == orig {
                continue;
            }
            let Some(orig_id) = Self::llm_identity(orig) else {
                continue;
            };
            let disk_prof = disk
                .iter()
                .find(|profile| Self::llm_identity(profile).as_ref() == Some(&orig_id));
            let cur_prof = current
                .iter()
                .find(|profile| Self::llm_identity(profile).as_ref() == Some(&orig_id))
                .unwrap_or(cur);
            let Some(slot) = result
                .iter()
                .position(|profile| Self::llm_identity(profile).as_ref() == Some(&orig_id))
            else {
                // The other window replaced this identity; do not attach this window's key to a
                // different profile.
                continue;
            };
            if let Some(on_disk) = disk_prof {
                result[slot] = Self::merge_one_profile(cur_prof, orig, on_disk);
            }
        }
        result
    }

    pub fn save(&self, dir: &Path, sealer: &dyn Sealer) -> std::io::Result<()> {
        self.save_merged(None, dir, sealer).map(|_| ())
    }

    /// Saves under the file lock, preserving fields on disk that were not changed relative to
    /// `base` (the snapshot loaded when the dialog opened), and preserving any unknown JSON keys.
    pub fn save_merged(
        &self,
        base: Option<&Self>,
        dir: &Path,
        sealer: &dyn Sealer,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file = dir.join(SETTINGS_FILE);
        if self.unknown_engine {
            return Err(std::io::Error::other(UNKNOWN_ENGINE));
        }
        let _lock = crate::storage::lock(&file)?;
        let old: serde_json::Value = match std::fs::read(&file) {
            Ok(bytes) => {
                let old: serde_json::Value = serde_json::from_slice(&bytes)?;
                // An existing file must be an object; anything else is kept for the user to restore.
                if self.settings_broken || !old.is_object() {
                    return Err(std::io::Error::other(SETTINGS_BROKEN));
                }
                old
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
            Err(e) => return Err(e),
        };
        let merged = if old.is_object() {
            if let Some(base) = base {
                let disk = Self::from_json_value(Some(&old), dir, sealer);
                if disk.unknown_engine && !base.unknown_engine {
                    return Err(std::io::Error::other(UNKNOWN_ENGINE));
                }
                let pick_str = |cur: &String, orig: &String, on_disk: &String| -> String {
                    if cur == orig {
                        on_disk.clone()
                    } else {
                        cur.clone()
                    }
                };
                let profiles = Self::merge_llm_profiles(
                    &base.llm_profiles(),
                    &self.llm_profiles(),
                    &disk.llm_profiles(),
                );
                Self {
                    engine: if self.engine == base.engine && !disk.unknown_engine {
                        disk.engine
                    } else {
                        self.engine
                    },
                    allow_fallback: if self.allow_fallback == base.allow_fallback {
                        disk.allow_fallback
                    } else {
                        self.allow_fallback
                    },
                    url: profiles[0].url.clone(),
                    model: profiles[0].model.clone(),
                    key: profiles[0].key.clone(),
                    disable_thinking: profiles[0].disable_thinking,
                    backup_llm: [profiles[1].clone(), profiles[2].clone()],
                    style: if self.style == base.style {
                        disk.style
                    } else {
                        self.style
                    },
                    custom: pick_str(&self.custom, &base.custom, &disk.custom),
                    google_key: pick_str(&self.google_key, &base.google_key, &disk.google_key),
                    azure_key: pick_str(&self.azure_key, &base.azure_key, &disk.azure_key),
                    azure_region: pick_str(
                        &self.azure_region,
                        &base.azure_region,
                        &disk.azure_region,
                    ),
                    deepl_key: pick_str(&self.deepl_key, &base.deepl_key, &disk.deepl_key),
                    keys_unreadable: disk.keys_unreadable
                        && self.key == base.key
                        && self.google_key == base.google_key
                        && self.azure_key == base.azure_key
                        && self.deepl_key == base.deepl_key
                        && self.backup_llm == base.backup_llm,
                    settings_broken: false,
                    unknown_engine: false,
                }
            } else {
                self.clone()
            }
        } else {
            self.clone()
        };
        let keys = serde_json::to_vec(&[
            merged.key.as_str(),
            merged.google_key.trim(),
            merged.azure_key.trim(),
            merged.deepl_key.trim(),
            merged.backup_llm[0].key.as_str(),
            merged.backup_llm[1].key.as_str(),
        ])?;
        if old.get("sealed_keys").is_none() {
            if merged.keys_unreadable {
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
            Some(blob) if merged.keys_unreadable || open_key_bundle(blob, sealer).is_none() => {
                if [
                    merged.key.as_str(),
                    merged.azure_key.as_str(),
                    merged.google_key.as_str(),
                    merged.deepl_key.as_str(),
                    merged.backup_llm[0].key.as_str(),
                    merged.backup_llm[1].key.as_str(),
                ]
                .iter()
                .any(|k| !k.is_empty())
                    || llm_urls_from_value(&old)
                        != [
                            merged.url.clone(),
                            merged.backup_llm[0].url.clone(),
                            merged.backup_llm[1].url.clone(),
                        ]
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
        let mut object = old.as_object().cloned().unwrap_or_default();
        object.insert("sealed_keys".into(), encrypted);
        object.insert("url".into(), serde_json::Value::String(merged.url.clone()));
        object.insert(
            "model".into(),
            serde_json::Value::String(merged.model.clone()),
        );
        object.insert(
            "style".into(),
            serde_json::Value::String(merged.style.id().to_owned()),
        );
        object.insert(
            "custom".into(),
            serde_json::Value::String(merged.custom.clone()),
        );
        object.insert(
            "engine".into(),
            serde_json::Value::String(merged.engine.id().to_owned()),
        );
        object.insert(
            "allow_fallback".into(),
            serde_json::Value::Bool(merged.allow_fallback),
        );
        object.insert(
            "azure_region".into(),
            serde_json::Value::String(merged.azure_region.trim().to_owned()),
        );
        object.insert(
            "disable_thinking".into(),
            serde_json::Value::Bool(merged.disable_thinking),
        );
        object.insert(
            "llm_models".into(),
            serde_json::Value::Array(
                merged
                    .llm_profiles()
                    .into_iter()
                    .map(|profile| {
                        serde_json::json!({
                            "url": profile.url,
                            "model": profile.model,
                            "disable_thinking": profile.disable_thinking,
                        })
                    })
                    .collect(),
            ),
        );
        let snapshot = serde_json::to_vec(&serde_json::Value::Object(object))?;
        if !crate::write_atomically(&dir.join(SETTINGS_FILE), &snapshot) {
            return Err(std::io::Error::other("设置替换失败"));
        }
        for legacy in [KEY_FILE, GOOGLE_KEY_FILE, AZURE_KEY_FILE, DEEPL_KEY_FILE] {
            let _ = std::fs::remove_file(dir.join(legacy));
        }
        Ok(merged)
    }
    /// Checks before sending a request. Free Google needs nothing; Google Cloud needs a key; see
    /// [`Self::validate_llm`] for the LLM.
    pub(crate) fn check_key_access(&self) -> Result<(), &'static str> {
        if self.settings_broken {
            return Err(SETTINGS_BROKEN);
        }
        if self.unknown_engine {
            return Err(UNKNOWN_ENGINE);
        }
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
        let profiles = self.llm_profiles();
        for profile in &profiles {
            profile.validate()?;
        }
        if profiles.iter().any(LlmProfile::is_ready) {
            Ok(())
        } else {
            Err("请先在“设置”中填写 API 地址、模型和密钥")
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
        assert_eq!(options.ui_scale(), UI_SCALE);
        assert!(!options.set_ui_scale(110));
        assert_eq!(options.ui_scale(), UI_SCALE);
        assert!(options.set_ui_scale(150));
        assert_eq!(options.ui_scale(), 150);
        options.0.set_u32("UiScale", 999);
        assert_eq!(options.ui_scale(), UI_SCALE);
        assert_eq!(scaled_dpi(96, 100), 96);
        assert_eq!(scaled_dpi(144, 100), 144);
        assert_eq!(scaled_dpi(96, 125), 120);
        assert_eq!(scaled_dpi(96, 200), 192);
        assert_eq!(scaled_dpi(120, 150), 180);
        assert_eq!(scaled_dpi(80, 130), 96);
        assert_eq!(ui_scale_to_write(100, 100), None);
        assert_eq!(ui_scale_to_write(100, 200), Some(200));
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
    fn settings_json_with_a_non_object_root_stops_translation_and_is_kept() {
        for (n, broken) in [&b"[]"[..], b"null", b"42", b"\"azure\""]
            .into_iter()
            .enumerate()
        {
            let dir =
                std::env::temp_dir().join(format!("yiime-non-object-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(SETTINGS_FILE), broken).unwrap();
            let loaded = ApiSettings::load(&dir, &PlainSealer);
            assert!(loaded.settings_broken);
            assert_eq!(loaded.check_key_access(), Err(SETTINGS_BROKEN));
            assert_eq!(loaded.validate(), Err(SETTINGS_BROKEN));
            // Even a dialog that cleared the flag cannot overwrite the damaged file.
            let draft = ApiSettings {
                engine: Engine::GoogleFree,
                ..ApiSettings::default()
            };
            assert!(loaded.save(&dir, &PlainSealer).is_err());
            assert!(draft.save(&dir, &PlainSealer).is_err());
            assert_eq!(std::fs::read(dir.join(SETTINGS_FILE)).unwrap(), broken);
            std::fs::remove_dir_all(dir).unwrap();
        }
        // First run: no file is not an error.
        let dir = std::env::temp_dir().join(format!("yiime-first-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fresh = ApiSettings::load(&dir, &PlainSealer);
        assert!(!fresh.settings_broken && !fresh.keys_unreadable);
        assert!(fresh.check_key_access().is_ok());
        fresh.save(&dir, &PlainSealer).unwrap();
        assert!(!ApiSettings::load(&dir, &PlainSealer).settings_broken);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_or_non_string_engine_is_rejected_while_missing_engine_defaults() {
        let dir = std::env::temp_dir().join(format!("yiime-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Missing `engine` field (older config): default Azure is ok.
        std::fs::write(
            dir.join(SETTINGS_FILE),
            br#"{"url":"https://example.com/v1"}"#,
        )
        .unwrap();
        let legacy = ApiSettings::load(&dir, &PlainSealer);
        assert_eq!(legacy.engine, Engine::Azure);
        assert!(!legacy.unknown_engine);
        assert!(legacy.check_key_access().is_ok());

        // Present with unknown string or wrong type: must stop and prompt to reselect.
        for bad in [
            br#"{"engine":"future_v9"}"#.as_slice(),
            br#"{"engine":42}"#.as_slice(),
            br#"{"engine":null}"#.as_slice(),
            br#"{"engine":[]}"#.as_slice(),
        ] {
            std::fs::write(dir.join(SETTINGS_FILE), bad).unwrap();
            let loaded = ApiSettings::load(&dir, &PlainSealer);
            assert!(loaded.unknown_engine);
            assert_eq!(loaded.check_key_access(), Err(UNKNOWN_ENGINE));
            assert_eq!(loaded.validate(), Err(UNKNOWN_ENGINE));
            assert!(loaded.save(&dir, &PlainSealer).is_err());
        }

        // Selecting a valid engine clears unknown_engine and saves cleanly.
        let fixed = ApiSettings {
            engine: Engine::DeepL,
            deepl_key: "k:fx".into(),
            ..ApiSettings::default()
        };
        fixed.save(&dir, &PlainSealer).unwrap();
        let reloaded = ApiSettings::load(&dir, &PlainSealer);
        assert!(!reloaded.unknown_engine);
        assert_eq!(reloaded.engine, Engine::DeepL);

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
            disable_thinking: false,
            backup_llm: Default::default(),
            style: TranslationStyle::Academic,
            custom: "x".into(),
            google_key: "gkey".into(),
            azure_key: "akey".into(),
            azure_region: "eastus".into(),
            deepl_key: "dkey:fx".into(),
            keys_unreadable: false,
            settings_broken: false,
            unknown_engine: false,
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

    #[test]
    fn plain_cell_keeps_pending_until_adopt_succeeds() {
        use super::plain_cell::{encode, read, store_after_adopt};
        let cell = encode(true, true);
        let r = read(cell, false, false);
        assert!(r.adopt);
        assert_eq!(store_after_adopt(r, false), None);
        assert_eq!(store_after_adopt(r, true), Some(encode(true, false)));
    }

    #[test]
    fn save_merged_preserves_concurrently_changed_fields_and_unknown_keys() {
        struct Xor;
        impl Sealer for Xor {
            fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
                Some(data.iter().map(|b| b ^ 0x5a).collect())
            }
            fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
                Some(data.iter().map(|b| b ^ 0x5a).collect())
            }
        }
        let dir = std::env::temp_dir().join(format!("yiime-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = ApiSettings {
            engine: Engine::Azure,
            azure_key: "az-1".into(),
            model: "gpt-4o-mini".into(),
            ..ApiSettings::default()
        };
        base.save(&dir, &Xor).unwrap();
        // Simulate newer version or concurrent edit adding an unknown key + changing model.
        let mut disk = base.clone();
        disk.model = "gpt-5".into();
        disk.save(&dir, &Xor).unwrap();
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        raw.as_object_mut()
            .unwrap()
            .insert("future_flag".into(), serde_json::json!(42));
        std::fs::write(dir.join(SETTINGS_FILE), serde_json::to_vec(&raw).unwrap()).unwrap();

        // Dialog based on `base` edits only `azure_region` and saves.
        let mut edited = base.clone();
        edited.azure_region = "eastus".into();
        let saved = edited.save_merged(Some(&base), &dir, &Xor).unwrap();
        assert_eq!(saved.model, "gpt-5");
        assert_eq!(saved.azure_region, "eastus");
        let after = ApiSettings::load(&dir, &Xor);
        assert_eq!(after.model, "gpt-5");
        assert_eq!(after.azure_region, "eastus");
        let raw_after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(raw_after["future_flag"], serde_json::json!(42));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn xor_dir(name: &str) -> (std::path::PathBuf, XorSealer) {
        let dir = std::env::temp_dir().join(format!("yiime-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (dir, XorSealer)
    }

    struct XorSealer;
    impl Sealer for XorSealer {
        fn seal(&self, data: &[u8]) -> Option<Vec<u8>> {
            Some(data.iter().map(|b| b ^ 0x5a).collect())
        }
        fn open(&self, data: &[u8]) -> Option<Vec<u8>> {
            Some(data.iter().map(|b| b ^ 0x5a).collect())
        }
    }

    #[test]
    fn llm_models_migrate_from_single_profile() {
        let (dir, sealer) = xor_dir("migrate-llm");
        let settings = ApiSettings {
            engine: Engine::Llm,
            url: "https://api.deepinfra.com/v1/openai/chat/completions".into(),
            model: "deepseek-ai/DeepSeek-V4.1-Flash-1017".into(),
            key: "primary-secret".into(),
            ..ApiSettings::default()
        };
        // Save without llm_models first: write a 0.2.5-style file (4 keys, no llm_models).
        settings.save(&dir, &sealer).unwrap();
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        raw.as_object_mut().unwrap().remove("llm_models");
        raw.as_object_mut().unwrap().remove("disable_thinking");
        // Shrink the sealed bundle back to four strings, like an older build.
        let four = sealer
            .seal(&serde_json::to_vec(&["primary-secret", "", "", ""]).unwrap())
            .unwrap();
        raw.as_object_mut()
            .unwrap()
            .insert("sealed_keys".into(), serde_json::to_value(four).unwrap());
        std::fs::write(dir.join(SETTINGS_FILE), serde_json::to_vec(&raw).unwrap()).unwrap();

        let loaded = ApiSettings::load(&dir, &sealer);
        assert_eq!(loaded.url, settings.url);
        assert_eq!(loaded.model, settings.model);
        assert_eq!(loaded.key, "primary-secret");
        assert!(
            loaded.disable_thinking,
            "DeepInfra defaults to disabling thinking"
        );
        assert!(loaded.backup_llm.iter().all(LlmProfile::is_empty));
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!json.contains("primary-secret"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn llm_models_openai_default_keeps_thinking() {
        assert!(!default_disable_thinking(DEFAULT_URL));
        assert!(default_disable_thinking(
            "https://api.deepinfra.com/v1/openai/chat/completions"
        ));
        assert!(!default_disable_thinking(
            "https://deepinfra.com.evil.example/v1"
        ));
    }

    #[test]
    fn llm_models_round_trip_preserves_order_and_keys() {
        let (dir, sealer) = xor_dir("llm-order");
        let mut settings = ApiSettings {
            engine: Engine::Llm,
            url: "https://primary.example/v1".into(),
            model: "model-a".into(),
            key: "key-a".into(),
            disable_thinking: true,
            ..ApiSettings::default()
        };
        settings.backup_llm = [
            LlmProfile {
                url: "https://backup.example/v1".into(),
                model: "model-b".into(),
                key: "key-b".into(),
                disable_thinking: false,
            },
            LlmProfile {
                url: "https://third.example/v1".into(),
                model: "model-c".into(),
                key: "key-c".into(),
                disable_thinking: true,
            },
        ];
        settings.save(&dir, &sealer).unwrap();
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!json.contains("key-a") && !json.contains("key-b") && !json.contains("key-c"));
        let loaded = ApiSettings::load(&dir, &sealer);
        assert_eq!(loaded, settings);

        // Reorder: B, A, C.
        let mut reordered = loaded;
        let mut profiles = reordered.llm_profiles();
        profiles.swap(0, 1);
        reordered.set_llm_profiles(profiles);
        reordered.save(&dir, &sealer).unwrap();
        let again = ApiSettings::load(&dir, &sealer);
        let names: Vec<_> = again.llm_profiles().into_iter().map(|p| p.model).collect();
        assert_eq!(names, ["model-b", "model-a", "model-c"]);
        assert_eq!(again.key, "key-b");
        assert_eq!(again.backup_llm[0].key, "key-a");
        assert!(!again.disable_thinking);
        assert!(again.backup_llm[0].disable_thinking);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn llm_backup_hint_names_the_saved_slot() {
        assert_eq!(llm_backup_hint(0), None);
        assert_eq!(llm_backup_hint(1).as_deref(), Some("已切换到备用模型 2"));
        assert_eq!(llm_backup_hint(2).as_deref(), Some("已切换到备用模型 3"));
    }

    /// Same check the settings window uses before writing API settings and other options.
    fn dialog_api_check(settings: &ApiSettings, plain: bool) -> Result<(), &'static str> {
        let llm_blank = settings.llm_profiles().iter().all(LlmProfile::is_empty);
        if settings.keys_unreadable || (plain && llm_blank) {
            Ok(())
        } else if settings.engine != Engine::Llm && !llm_blank {
            settings.validate().and_then(|_| settings.validate_llm())
        } else {
            settings.validate()
        }
    }

    #[test]
    fn placeholder_openai_url_is_unused_so_other_settings_can_be_saved() {
        // Fresh install: default OpenAI URL, empty model/key. Choosing free Google and changing
        // only an unrelated option (candidate page size is stored separately) must still save.
        let mut settings = ApiSettings::default();
        assert_eq!(settings.url, DEFAULT_URL);
        assert!(settings.model.is_empty() && settings.key.is_empty());
        assert!(settings.llm_profiles()[0].is_empty());
        assert!(settings.llm_profiles().iter().all(LlmProfile::is_empty));
        settings.engine = Engine::GoogleFree;
        assert!(dialog_api_check(&settings, false).is_ok());
        assert!(settings.validate().is_ok());
        assert!(settings.validate_llm().is_err());

        // A half-filled profile (URL plus model, no key) is still a configuration error.
        settings.url = DEFAULT_URL.to_owned();
        settings.model = "gpt-4o-mini".into();
        assert!(!settings.llm_profiles()[0].is_empty());
        assert!(dialog_api_check(&settings, false).is_err());

        // A custom URL with no model or key is also not an unused slot.
        let custom = LlmProfile {
            url: "https://example.com/v1".into(),
            ..LlmProfile::default()
        };
        assert!(!custom.is_empty());
        assert!(custom.validate().is_err());
    }

    fn sample_profile(url: &str, model: &str, key: &str) -> LlmProfile {
        LlmProfile {
            url: url.to_owned(),
            model: model.to_owned(),
            key: key.to_owned(),
            disable_thinking: false,
        }
    }

    #[test]
    fn save_merged_keeps_key_with_the_same_model_after_a_concurrent_reorder() {
        let (dir, sealer) = xor_dir("llm-merge-reorder");
        let mut base = ApiSettings {
            engine: Engine::Llm,
            ..ApiSettings::default()
        };
        base.set_llm_profiles([
            sample_profile("https://a.example/v1", "model-a", "test-key-a"),
            sample_profile("https://b.example/v1", "model-b", "test-key-b"),
            LlmProfile::default(),
        ]);
        base.save(&dir, &sealer).unwrap();

        // Window 1 moves B first and saves.
        let mut window1 = base.clone();
        let mut profiles = window1.llm_profiles();
        profiles.swap(0, 1);
        window1.set_llm_profiles(profiles);
        window1.save_merged(Some(&base), &dir, &sealer).unwrap();

        // Window 2 still shows A, B and only updates A's key.
        let mut window2 = base.clone();
        window2.key = "test-key-a-new".into();
        let saved = window2.save_merged(Some(&base), &dir, &sealer).unwrap();
        let loaded = ApiSettings::load(&dir, &sealer);
        assert_eq!(saved, loaded);
        let profiles = loaded.llm_profiles();
        assert_eq!(profiles[0].url, "https://b.example/v1");
        assert_eq!(profiles[0].model, "model-b");
        assert_eq!(profiles[0].key, "test-key-b");
        assert_eq!(profiles[1].url, "https://a.example/v1");
        assert_eq!(profiles[1].model, "model-a");
        assert_eq!(profiles[1].key, "test-key-a-new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct LockedSealer;
    impl Sealer for LockedSealer {
        fn seal(&self, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
        fn open(&self, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
    }

    fn sealed_keys_value(dir: &std::path::Path) -> serde_json::Value {
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        raw.get("sealed_keys")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }

    fn overwrite_sealed_bundle(
        dir: &std::path::Path,
        sealer: &dyn Sealer,
        keys: serde_json::Value,
    ) {
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        let sealed = sealer.seal(&serde_json::to_vec(&keys).unwrap()).unwrap();
        raw.as_object_mut()
            .unwrap()
            .insert("sealed_keys".into(), serde_json::to_value(sealed).unwrap());
        std::fs::write(dir.join(SETTINGS_FILE), serde_json::to_vec(&raw).unwrap()).unwrap();
    }

    fn settings_with_backups() -> ApiSettings {
        let mut settings = ApiSettings {
            engine: Engine::Llm,
            url: "https://a.example/v1".into(),
            model: "model-a".into(),
            key: "test-key-a".into(),
            custom: "old-style".into(),
            ..ApiSettings::default()
        };
        settings.backup_llm = [
            sample_profile("https://b.example/v1", "model-b", "test-key-b"),
            sample_profile("https://c.example/v1", "model-c", "test-key-c"),
        ];
        settings
    }

    #[test]
    fn decrypt_failure_with_backup_urls_still_saves_unrelated_settings() {
        let (dir, sealer) = xor_dir("decrypt-backup-urls");
        let original = settings_with_backups();
        original.save(&dir, &sealer).unwrap();
        let ciphertext = sealed_keys_value(&dir);

        let mut draft = ApiSettings::load(&dir, &LockedSealer);
        assert!(draft.keys_unreadable);
        assert_eq!(draft.backup_llm[0].url, "https://b.example/v1");
        assert_eq!(draft.backup_llm[1].url, "https://c.example/v1");
        draft.custom = "new-style".into();
        draft.save(&dir, &LockedSealer).unwrap();
        assert_eq!(sealed_keys_value(&dir), ciphertext);
        let roundtrip = ApiSettings::load(&dir, &sealer);
        assert_eq!(roundtrip.key, original.key);
        assert_eq!(roundtrip.backup_llm[0].key, original.backup_llm[0].key);
        assert_eq!(roundtrip.backup_llm[1].key, original.backup_llm[1].key);
        assert_eq!(roundtrip.custom, "new-style");

        let mut draft = ApiSettings::load(&dir, &LockedSealer);
        draft.backup_llm[0].url = "https://other.example/v1".into();
        assert!(draft.save(&dir, &LockedSealer).is_err());
        assert_eq!(sealed_keys_value(&dir), ciphertext);

        let mut draft = ApiSettings::load(&dir, &LockedSealer);
        draft.url.push_str("/changed");
        assert!(draft.save(&dir, &LockedSealer).is_err());
        assert_eq!(sealed_keys_value(&dir), ciphertext);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn four_and_six_string_key_bundles_are_readable() {
        let (dir, sealer) = xor_dir("bundle-4-6");
        settings_with_backups().save(&dir, &sealer).unwrap();

        overwrite_sealed_bundle(
            &dir,
            &sealer,
            serde_json::json!(["k-llm", "k-google", "k-azure", "k-deepl"]),
        );
        let four = ApiSettings::load(&dir, &sealer);
        assert!(!four.keys_unreadable);
        assert_eq!(four.key, "k-llm");
        assert_eq!(four.google_key, "k-google");
        assert_eq!(four.azure_key, "k-azure");
        assert_eq!(four.deepl_key, "k-deepl");
        assert!(four.backup_llm[0].key.is_empty());
        assert!(four.backup_llm[1].key.is_empty());

        overwrite_sealed_bundle(
            &dir,
            &sealer,
            serde_json::json!(["k0", "g", "a", "d", "b1", "b2"]),
        );
        let six = ApiSettings::load(&dir, &sealer);
        assert!(!six.keys_unreadable);
        assert_eq!(six.key, "k0");
        assert_eq!(six.google_key, "g");
        assert_eq!(six.azure_key, "a");
        assert_eq!(six.deepl_key, "d");
        assert_eq!(six.backup_llm[0].key, "b1");
        assert_eq!(six.backup_llm[1].key, "b2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_key_bundles_stay_unreadable_and_are_not_overwritten() {
        for (name, bundle) in [
            (
                "five",
                serde_json::json!(["k-llm", "k-google", "k-azure", "k-deepl", "k-extra"]),
            ),
            (
                "null",
                serde_json::json!(["k-llm", null, "k-azure", "k-deepl"]),
            ),
        ] {
            let (dir, sealer) = xor_dir(&format!("bundle-bad-{name}"));
            settings_with_backups().save(&dir, &sealer).unwrap();
            overwrite_sealed_bundle(&dir, &sealer, bundle);
            let ciphertext = sealed_keys_value(&dir);
            let mut draft = ApiSettings::load(&dir, &sealer);
            assert!(draft.keys_unreadable, "{name} bundle should be unreadable");
            draft.custom = "unrelated".into();
            draft.save(&dir, &sealer).unwrap();
            assert_eq!(sealed_keys_value(&dir), ciphertext);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
