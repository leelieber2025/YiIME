//! Platform-independent core of YiIME.
//!
//! Dictionaries and indexes, the candidate engine, input state and settings live here without OS
//! dependencies; the Windows TSF DLL and input-box EXE only handle system interfaces (registry,
//! DPAPI, windows, key injection).
//!
//! Storage goes through traits: [`OptionStore`] for switches (HKCU registry on Windows) and
//! [`Sealer`] for encrypting keys and history (DPAPI on Windows).
//!
//! Features: `engine` (input engine and Simplified→Traditional tables), `bundled` (built-in
//! dictionaries), `translate` (translation client), all on by default. The TSF DLL uses only the
//! settings model, shortcuts and punctuation mapping, with every feature off to stay small.

#[cfg(feature = "engine")]
mod dict;
#[cfg(feature = "engine")]
mod draft;
#[cfg(feature = "engine")]
mod engine;
mod history;
mod hotkey;
mod inline;
#[cfg(feature = "engine")]
mod learn;
mod settings;
mod style;
mod text;
#[cfg(feature = "translate")]
mod translate;

#[cfg(feature = "engine")]
pub use dict::{Candidate, MixedInput, Source};
#[cfg(feature = "engine")]
pub use draft::Draft;
#[cfg(feature = "engine")]
pub use engine::{
    CANDIDATE_PAGES, ComposeOptions, Composer, LetterAction, LetterRules, MAX_CODE_LEN, Pick,
    auto_commits_four, char_of_word, classify_letter, paging_direction, pushes_fifth,
    selection_index,
};
pub use history::{HISTORY_CAP, HISTORY_FILE, History, HistoryEntry, history_label};
pub use hotkey::{DEFAULT_PLAIN_TOGGLE, Hotkey, parse_hotkey};
pub use inline::{
    SHIFT_TAP_MAX, ShiftTap, is_literal_in_english, is_numpad_digit, is_shift_key, literal_code,
};
#[cfg(feature = "engine")]
pub use learn::{LEARNED_FILE, Learned};
pub use settings::{
    AZURE_KEY_FILE, ApiSettings, DEFAULT_HISTORY_KEY, DEFAULT_URL, Engine, Flag, GOOGLE_KEY_FILE,
    KEY_FILE, MemoryStore, OptionStore, Options, PAGE_SIZE, PAGE_SIZE_RANGE, PlainSealer,
    SETTINGS_FILE, Sealer, USER_WORDS_FILE, parse_page_size,
};
pub use style::{TranslationStyle, system_prompt};
#[cfg(feature = "engine")]
pub use text::to_traditional;
pub use text::{Quotes, closing_pair, full_width, is_closing, single_line, us_key_char};
#[cfg(feature = "translate")]
pub use translate::{
    Attempt, Outcome, prewarm, translate, translate_openai, translate_openai_with,
};
