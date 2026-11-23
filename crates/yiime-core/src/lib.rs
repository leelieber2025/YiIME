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
mod import;
mod inline;
#[cfg(feature = "engine")]
mod keys;
#[cfg(feature = "engine")]
mod learn;
mod reload;
mod request;
mod service;
mod settings;
mod shuangpin;
mod style;
#[cfg(feature = "engine")]
mod table;
mod text;
#[cfg(feature = "translate")]
mod translate;

#[cfg(feature = "engine")]
pub use dict::{
    COMMON_FREQUENCY_FLOOR, Candidate, MixedInput, Source, is_common_text, is_gb2312_hanzi,
};
#[cfg(feature = "engine")]
pub use draft::Draft;
#[cfg(feature = "engine")]
pub use engine::{
    CANDIDATE_PAGES, ComposeOptions, Composer, Flushed, LetterAction, LetterRules, MAX_CODE_LEN,
    Pick, auto_commits_four, char_of_word, classify_letter, paging_direction, pushes_fifth,
    selection_index,
};
pub use history::{HISTORY_CAP, HISTORY_FILE, History, HistoryEntry, history_label};
pub use hotkey::{DEFAULT_PLAIN_TOGGLE, Hotkey, parse_hotkey};
pub use import::{
    ImportedWord, decode_text, import_words, merge_user_words, parse_scel, parse_text_export,
};
pub use inline::{
    SHIFT_TAP_MAX, ShiftTap, is_literal_in_english, is_numpad_digit, is_shift_key, literal_code,
};
#[cfg(feature = "engine")]
pub use keys::{
    KeyAction, KeyContext, classify_char, semicolon_is_code, shuangpin_selection, with_separator,
};
#[cfg(feature = "engine")]
pub use learn::{
    HALF_LIFE_DAYS, LEARNED_FILE, LEGACY_LEARNED_FILE, Learned, MAX_ENTRIES, PROMOTE_SCORE,
    write_atomically,
};
pub use reload::{FileStamp, UserWordsReloader};
pub use request::RequestGate;
pub use service::{
    APP_WINDOW_CLASS, Activation, BACKOFF_RESET, BoxView, CommitTarget, DUPLICATE_COMMIT_MS,
    EMPTY_BOX_GRACE_MS, FIRST_BACKOFF, FieldSwitches, HIGH_INTEGRITY, HostOwner, KeyRoute,
    LeftField, MAX_BACKOFF, MEDIUM_INTEGRITY, PRIVATE_SCOPES, PassReason, ProcessSecurity,
    REQUEST_TIMEOUT_MS, SegmentAnchor, ServiceHealth, ShiftTapAction, Startup, TRAY_WINDOW_TITLE,
    TSF_PROTOCOL, TSF_RECEIVER_CLASS, UnconfirmedCommit, WATCHDOG_GRACE, WATCHDOG_LIMIT, Watchdog,
    WatchedHosts, caret_anchor, commit_digest, commit_targets, duplicate_commit,
    english_ends_session, focused_first, handle_from_wire, hide_empty_box, key_route,
    keys_to_give_back, late_commit_landed, left_field, may_share_service, needs_private_editor,
    next_held_id, pass_reason, private_scope, redirect_to_focused, service_window_title,
    shift_tap_action, startup_state,
};
pub use settings::{
    AZURE_KEY_FILE, ApiSettings, DEFAULT_HISTORY_KEY, DEFAULT_URL, Engine, Flag, GOOGLE_KEY_FILE,
    KEY_FILE, MemoryStore, OptionStore, Options, PAGE_SIZE, PAGE_SIZE_RANGE, PlainSealer,
    SETTINGS_FILE, Sealer, USER_WORDS_FILE, llm_url_allowed, parse_page_size,
};
pub use shuangpin::Shuangpin;
pub use style::{TranslationStyle, system_prompt};
#[cfg(feature = "engine")]
pub use text::to_traditional;
pub use text::{Quotes, closing_pair, full_width, is_closing, single_line, us_key_char};
#[cfg(feature = "translate")]
pub use translate::{
    Attempt, Outcome, prewarm, translate, translate_openai, translate_openai_with,
};
