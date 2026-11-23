//! Health rules for the shared input-box service (`YiIME.exe` in TSF mode), kept free of Win32
//! so they can be tested anywhere.
//!
//! * [`ServiceHealth`] lives in each TSF DLL instance: every request to the service has a short
//!   timeout; a timeout marks the service unhealthy, and the next activation terminates that
//!   process and starts a new one, with exponential backoff so a service that keeps hanging is
//!   not restarted in a loop (during backoff the DLL simply does not open the input box).
//! * [`Watchdog`] lives in the service: the message loop beats, a watchdog thread checks, and a
//!   loop stuck for longer than the limit ends the process so the DLL can start a fresh one.

use std::time::{Duration, Instant};

/// Timeout of one interactive request from the DLL to the service.
pub const REQUEST_TIMEOUT_MS: u32 = 300;
/// Delay before the second restart; doubles up to [`MAX_BACKOFF`].
pub const FIRST_BACKOFF: Duration = Duration::from_secs(2);
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A restart this long ago without new trouble resets the backoff.
pub const BACKOFF_RESET: Duration = Duration::from_secs(300);

/// What to do when a new input session should open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// Use the running service (or start one if none is running).
    Use,
    /// Terminate the hung service process `kill` (after checking its executable path), then
    /// start a new one.
    Restart { kill: u32 },
    /// The service hung again soon after a restart: do not open the input box until the backoff
    /// has passed (keys go to the application as if the IME were off).
    Wait,
}

#[derive(Debug, Default, Clone)]
pub struct ServiceHealth {
    unhealthy: Option<u32>,
    restarts: u32,
    last_restart: Option<Instant>,
    /// Input boxes this DLL started that exited or never reported ready, in a row.
    start_failures: u32,
    last_start_failure: Option<Instant>,
}

impl ServiceHealth {
    /// A request to service process `pid` timed out or the service never became ready.
    pub fn timed_out(&mut self, pid: u32) {
        if pid != 0 {
            self.unhealthy = Some(pid);
        }
    }

    /// Service process `pid` answered.
    pub fn responded(&mut self, pid: u32) {
        if self.unhealthy == Some(pid) {
            self.unhealthy = None;
        }
    }

    pub fn is_unhealthy(&self, pid: u32) -> bool {
        pid != 0 && self.unhealthy == Some(pid)
    }

    /// An input box this DLL started exited, or never reported ready in time. Starting another
    /// one waits [`Self::start_backoff`] (keys go to the application meanwhile), so a box that
    /// cannot start does not swallow every first letter.
    pub fn start_failed(&mut self, now: Instant) {
        if self
            .last_start_failure
            .is_some_and(|last| now.saturating_duration_since(last) >= BACKOFF_RESET)
        {
            self.start_failures = 0;
        }
        self.start_failures = self.start_failures.saturating_add(1);
        self.last_start_failure = Some(now);
    }

    /// An input box reported ready: starting works again.
    pub fn started(&mut self) {
        self.start_failures = 0;
        self.last_start_failure = None;
    }

    /// Wait after the last failed start before starting another box: 2 s, doubling to 60 s.
    pub fn start_backoff(&self) -> Duration {
        match self.start_failures {
            0 => Duration::ZERO,
            n => FIRST_BACKOFF
                .saturating_mul(1u32 << (n - 1).min(16))
                .min(MAX_BACKOFF),
        }
    }

    /// Decides at activation; `running` is the pid of the service found running (if any).
    pub fn before_activation(&mut self, running: Option<u32>, now: Instant) -> Activation {
        // Nothing running, so a box would be started: not while backing off a failed start.
        if running.is_none()
            && self
                .last_start_failure
                .is_some_and(|last| now.saturating_duration_since(last) < self.start_backoff())
        {
            return Activation::Wait;
        }
        let Some(hung) = self.unhealthy else {
            return Activation::Use;
        };
        if running != Some(hung) {
            // The hung process is gone (exited or restarted by another application).
            self.unhealthy = None;
            return Activation::Use;
        }
        if let Some(last) = self.last_restart {
            let since = now.saturating_duration_since(last);
            if since >= BACKOFF_RESET {
                self.restarts = 0;
            } else if since < self.backoff() {
                return Activation::Wait;
            }
        }
        self.restarts += 1;
        self.last_restart = Some(now);
        self.unhealthy = None;
        Activation::Restart { kill: hung }
    }

    /// Wait required after the last restart before another one.
    pub fn backoff(&self) -> Duration {
        match self.restarts {
            0 => Duration::ZERO,
            n => FIRST_BACKOFF
                .saturating_mul(1u32 << (n - 1).min(16))
                .min(MAX_BACKOFF),
        }
    }
}

/// Mandatory integrity level RIDs (SECURITY_MANDATORY_*_RID).
pub const MEDIUM_INTEGRITY: u32 = 0x2000;
pub const HIGH_INTEGRITY: u32 = 0x3000;

/// Security context of a process: from its token and session (Win32 side fills it in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSecurity {
    /// TokenElevation.
    pub elevated: bool,
    /// Integrity level RID (TokenIntegrityLevel).
    pub integrity: u32,
    /// Terminal Services session.
    pub session: u32,
    /// User SID bytes (TokenUser).
    pub user: Vec<u8>,
}

/// The TSF DLL in an elevated or high-integrity application (or one whose token cannot be read)
/// never uses the shared service: a lower-integrity process must not type into it. It starts a
/// private input box of its own (inheriting the application's level) that serves only it and
/// exits with it.
pub fn needs_private_editor(host: Option<&ProcessSecurity>) -> bool {
    host.is_none_or(|host| host.elevated || host.integrity > MEDIUM_INTEGRITY)
}

/// Whether the DLL in `host` may connect to a shared service running as `service`: same user,
/// same session, same integrity level, not elevated. Unknown (`None`) is never trusted; the DLL
/// then starts its own service.
pub fn may_share_service(host: &ProcessSecurity, service: Option<&ProcessSecurity>) -> bool {
    !needs_private_editor(Some(host))
        && service.is_some_and(|service| {
            !service.elevated
                && service.integrity == host.integrity
                && service.session == host.session
                && service.user == host.user
        })
}

/// Version of the DLL ↔ input-box protocol (window messages and COPYDATA packets). It is part of
/// the receiver window class and the service window title, so a DLL and a service of different
/// protocol versions never find or accept each other: after an upgrade without a restart, an
/// application still running an old DLL keeps its own old input box instead of talking to the new
/// one. Bump it whenever a message or packet changes meaning.
pub const TSF_PROTOCOL: u32 = 3;
/// Window class of the TSF DLL's receiver window (the input box accepts sessions only from it).
pub const TSF_RECEIVER_CLASS: &str = concat!("YiIMETsfReceiver.", 3);
/// Window class of YiIME.exe's tray and input-box windows, which the TSF DLL looks up.
pub const APP_WINDOW_CLASS: &str = "YiIMEWindow";
/// Title of the tray instance's window: the TSF DLL asks it to open settings or history.
pub const TRAY_WINDOW_TITLE: &str = "YiIME 译 · 托盘";

/// Window title of the input box: the shared service has one per integrity level (FindWindow only
/// sees the caller's session and desktop, and the DLL checks the process as well); a private box
/// has its own title so no other application finds it. Every title carries [`TSF_PROTOCOL`].
pub fn service_window_title(integrity: u32, private: bool) -> String {
    if private {
        format!("YiIME 译 · TSF（私有）· p{TSF_PROTOCOL}")
    } else if integrity == MEDIUM_INTEGRITY {
        format!("YiIME 译 · TSF · p{TSF_PROTOCOL}")
    } else {
        format!("YiIME 译 · TSF · IL{integrity:x} · p{TSF_PROTOCOL}")
    }
}

/// Main loop of the service considered stuck after this long without a beat.
pub const WATCHDOG_LIMIT: Duration = Duration::from_secs(5);
/// No checks during start-up (dictionary loading, first window).
pub const WATCHDOG_GRACE: Duration = Duration::from_secs(20);

/// Heartbeat check with an injectable clock: times are milliseconds since the watchdog started.
#[derive(Debug, Clone, Copy)]
pub struct Watchdog {
    pub limit_ms: u64,
    pub grace_ms: u64,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self {
            limit_ms: WATCHDOG_LIMIT.as_millis() as u64,
            grace_ms: WATCHDOG_GRACE.as_millis() as u64,
        }
    }
}

impl Watchdog {
    /// Whether the loop counts as stuck at `now_ms`, given its last beat at `last_beat_ms` (0 =
    /// none yet). Before the grace period ends nothing counts as stuck; afterwards a loop that
    /// never beat is measured from the end of the grace period.
    pub fn stalled(&self, last_beat_ms: u64, now_ms: u64) -> bool {
        now_ms >= self.grace_ms
            && now_ms.saturating_sub(last_beat_ms.max(self.grace_ms)) > self.limit_ms
    }
}

/// Owner of a TSF receiver window: process and thread id from `GetWindowThreadProcessId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostOwner {
    pub pid: u32,
    pub thread: u32,
}

/// Receivers whose host process the service watches for exit. Windows reuses window handles, so a
/// receiver is keyed by its handle *and* its owner: a reused handle from another process is a new
/// host (watched again), and an exit notice from the old owner no longer ends its session.
#[derive(Debug, Default)]
pub struct WatchedHosts {
    watched: std::collections::HashMap<usize, HostOwner>,
}

impl WatchedHosts {
    /// A session starts for `receiver`, whose current owner is `owner` (None when the window is
    /// gone). Returns true when a new exit watch must be started for it.
    pub fn session_started(&mut self, receiver: usize, owner: Option<HostOwner>) -> bool {
        match owner.filter(|owner| owner.pid != 0) {
            None => {
                self.watched.remove(&receiver);
                false
            }
            Some(owner) => self.watched.insert(receiver, owner) != Some(owner),
        }
    }

    /// The watched process `pid` of `receiver` exited. Returns true when that is the receiver's
    /// current host (end its session); false for a stale notice about an earlier owner of a
    /// reused handle.
    pub fn host_exited(&mut self, receiver: usize, pid: u32) -> bool {
        if self
            .watched
            .get(&receiver)
            .is_some_and(|owner| owner.pid == pid)
        {
            self.watched.remove(&receiver);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.watched.len()
    }

    pub fn is_empty(&self) -> bool {
        self.watched.is_empty()
    }
}

/// What the shared input box looks like right now, for [`hide_empty_box`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BoxView {
    pub visible: bool,
    /// No code, no draft, no status line.
    pub empty: bool,
    /// The box is the foreground window (typing goes into it).
    pub foreground: bool,
    /// A translation or commit is in flight.
    pub busy: bool,
    /// Time since the box was shown.
    pub shown_for_ms: u64,
}

/// Time an empty, unfocused box may stay up after being shown (bringing it to the foreground can
/// take a moment).
pub const EMPTY_BOX_GRACE_MS: u64 = 500;

/// Whether to hide the box now. An empty box that does not have the keyboard is useless and only
/// covers the page: it goes away at once when the host reports that its focus moved (another
/// text field, the page, another window), and otherwise after [`EMPTY_BOX_GRACE_MS`]. A box with
/// content, a busy box or one the user is typing in stays.
pub fn hide_empty_box(view: BoxView, host_focus_moved: bool) -> bool {
    view.visible
        && view.empty
        && !view.busy
        && !view.foreground
        && (host_focus_moved || view.shown_for_ms >= EMPTY_BOX_GRACE_MS)
}

/// The host application switched to English (Shift tap, Ctrl+Space, the mode button, the
/// taskbar): the box ends its session and hides, empty or not (a draft is kept for next time, as
/// when clicking away), unless a translation is in flight (its result still lands). Before
/// 0.2.0-rc.2 the box was not told, and an empty translation box stayed up until it timed out.
pub fn english_ends_session(view: BoxView) -> bool {
    view.visible && !view.busy
}

/// What to do with the draft when the host leaves this text field for another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftField {
    /// A translation is in flight: commit the Chinese where it was typed and drop the result.
    CommitSource,
    /// Nothing was submitted: drop the draft. It must not be inserted into the other field.
    Discard,
}

/// `translating`: Enter was pressed and the result has not come back.
pub fn left_field(translating: bool) -> LeftField {
    if translating {
        LeftField::CommitSource
    } else {
        LeftField::Discard
    }
}

/// Where a letter typed in the host goes, in the TSF DLL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// A session is open in this text field: buffer the letter for the box.
    Buffer,
    /// A session is open, but for another text field (the user clicked elsewhere while the box
    /// lingered): end that session at once and start a fresh one with this letter.
    StaleSession,
    /// No session: the letter starts one.
    NewSession,
}

/// `session_open`: the DLL has an input session; `same_context`: the key comes from the text
/// field (TSF context) that session belongs to. A session for another field never swallows
/// input: before 0.2.0-rc.2 those letters were refused until the old box ended.
pub fn key_route(session_open: bool, same_context: bool) -> KeyRoute {
    match (session_open, same_context) {
        (true, true) => KeyRoute::Buffer,
        (true, false) => KeyRoute::StaleSession,
        (false, _) => KeyRoute::NewSession,
    }
}

/// What decides whether a text field takes the IME's letters. Only the field's own switches
/// count, as TSF and Microsoft's SampleIME prescribe: GUID_COMPARTMENT_KEYBOARD_DISABLED,
/// GUID_COMPARTMENT_EMPTYCONTEXT, TF_SD_READONLY, and ES_PASSWORD on a Win32 edit control. A
/// compartment that cannot be read (None) does not disable. InputScope never decides this: it is
/// read asynchronously and late in some hosts (a freshly opened Word passed every letter to the
/// document in 0.2.0), so it only drives [`private_scope`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FieldSwitches {
    pub keyboard_disabled: Option<bool>,
    pub empty_context: Option<bool>,
    pub read_only: bool,
    pub es_password: bool,
}

impl FieldSwitches {
    /// The field refuses IME input (read-only, disabled or empty context).
    pub fn blocked(self) -> bool {
        self.read_only
            || self.keyboard_disabled.unwrap_or(false)
            || self.empty_context.unwrap_or(false)
    }
}

/// InputScope values that mark a private field: password and PIN scopes, and IS_PRIVATE, which
/// Chromium reports for password fields and for every field of a private window. Values from
/// InputScope.h.
pub const PRIVATE_SCOPES: [i32; 6] = [31, 61, 63, 64, 65, 66];

/// Whether a field's InputScope list marks it private. Such a field is still typed in through
/// the box, but nothing is learned, kept in the history or sent to a translation service.
pub fn private_scope(scopes: &[i32]) -> bool {
    scopes.iter().any(|scope| PRIVATE_SCOPES.contains(scope))
}

/// A caret rectangle (left, top, right, bottom) worth placing the box at: not empty (some hosts
/// answer zeros before layout) and overlapping the virtual screen `screen`.
pub fn caret_usable(rect: [i32; 4], screen: [i32; 4]) -> bool {
    let [left, top, right, bottom] = rect;
    let [screen_left, screen_top, screen_right, screen_bottom] = screen;
    bottom > top
        && right >= left
        && !(left == 0 && top == 0 && right == 0 && bottom == 0)
        && right >= screen_left
        && left < screen_right
        && bottom > screen_top
        && top < screen_bottom
}

/// Why a plain letter went to the application instead of the input box (logged when it changes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassReason {
    /// The field is read-only, keyboard-disabled or an empty context.
    ReadOnly,
    /// The IME is in English mode.
    English,
    /// A Win32 edit control with ES_PASSWORD.
    Password,
    /// The input box could not be opened (service unavailable or backing off).
    BoxUnavailable,
}

impl PassReason {
    pub fn describe(self) -> &'static str {
        match self {
            PassReason::ReadOnly => "输入框只读、禁用或为空上下文",
            PassReason::English => "英文模式",
            PassReason::Password => "密码输入框（ES_PASSWORD）",
            PassReason::BoxUnavailable => "输入框暂时打不开",
        }
    }
}

/// The reason to log for a plain letter that is not taken, or None when taking it is expected
/// to be left to the application anyway (Caps Lock, Shift, Ctrl, Alt, Win).
pub fn pass_reason(
    read_only: bool,
    english: bool,
    modifier_or_caps: bool,
    password: bool,
) -> Option<PassReason> {
    if read_only {
        Some(PassReason::ReadOnly)
    } else if english {
        Some(PassReason::English)
    } else if modifier_or_caps {
        None
    } else if password {
        Some(PassReason::Password)
    } else {
        None
    }
}

/// Where the box's commit (COPYDATA 0, text for its current session) goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitTarget {
    /// The text field of the open session.
    Session,
    /// The field parked when the session ended for another field (the box had not been told
    /// yet, e.g. a translation that finished in between).
    Held,
    /// No field is known any more: the text field that has the focus now, rather than refusing
    /// the commit and leaving the user unable to type.
    Focused,
}

/// Commit targets in the order to try them. An open session wins, so text never lands in a field
/// that was left; the focused field is the last resort. `moved_in_window`: the session's
/// document no longer has the focus, but the focus is still inside the same top-level window
/// (Word replaces its first document right after opening, Electron gives a refocused field a new
/// one, Explorer's search box moves between child windows). The focused context is then tried
/// first, as qingjian commits into the context the host handed over last rather than a
/// remembered one: the old context may accept the edit and show nothing.
pub fn commit_targets(
    session: bool,
    held: bool,
    focused: bool,
    moved_in_window: bool,
) -> Vec<CommitTarget> {
    let first_focused = session && focused && moved_in_window;
    [
        (first_focused, CommitTarget::Focused),
        (session, CommitTarget::Session),
        (held, CommitTarget::Held),
        (focused && !first_focused, CommitTarget::Focused),
    ]
    .into_iter()
    .filter_map(|(available, target)| available.then_some(target))
    .collect()
}

/// Digest of a committed text, sent back with the DLL's confirmation (WM_APP + 10) so the box can
/// match it to the draft it sent. FNV-1a over the UTF-16 units the box puts in the packet.
pub fn commit_digest(text: &str) -> u32 {
    text.encode_utf16().fold(0x811c_9dc5u32, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193)
    })
}

/// How long after a commit the same text, arriving with no session and no parked field, is the
/// box re-sending a commit whose reply it lost rather than new input (new input opens a session).
pub const DUPLICATE_COMMIT_MS: u64 = 3000;

/// Whether a commit is the box re-sending one that already landed. `last` is the digest and age
/// in ms of the last successful commit.
pub fn duplicate_commit(session: bool, held: bool, last: Option<(u32, u64)>, digest: u32) -> bool {
    !session
        && !held
        && last.is_some_and(|(previous, age)| previous == digest && age < DUPLICATE_COMMIT_MS)
}

/// A commit the box gave up on (no reply): the receiver it went to, the digest of the text sent,
/// and the digest of the draft the box kept (the text sent may be a translation of it).
pub type UnconfirmedCommit = (usize, u32, u32);

/// Whether a confirmation (`receiver`, `digest`) that arrives after the box gave up on a commit
/// means the kept draft was already inserted: it matches that commit and the user has not edited
/// the draft since. The box then clears it instead of the user sending it twice.
pub fn late_commit_landed(
    unconfirmed: Option<UnconfirmedCommit>,
    receiver: usize,
    digest: u32,
    draft: &str,
) -> bool {
    unconfirmed.is_some_and(|(to, sent, kept)| {
        to == receiver && sent == digest && commit_digest(draft) == kept
    })
}

/// What a Shift tap in the input box does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftTapAction {
    /// Toggle the inline English segment ("Shift 夹英文"): the box has something in it.
    ToggleSegment,
    /// The box is empty: switch the application's input mode to English (as a Shift tap in the
    /// application does) and close the box at once, instead of leaving an empty box up.
    SwitchToEnglish,
}

/// `tsf`: the box serves a TSF session (not the standalone tray box). `english_segment`: the
/// inline English segment is on (a tap turns it off and keeps the box).
pub fn shift_tap_action(
    tsf: bool,
    code_empty: bool,
    draft_empty: bool,
    english_segment: bool,
) -> ShiftTapAction {
    if tsf && code_empty && draft_empty && !english_segment {
        ShiftTapAction::SwitchToEnglish
    } else {
        ShiftTapAction::ToggleSegment
    }
}

/// State of an input box the DLL is waiting for after sending it the first letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// No session waiting for a box (none open, or the box already reported ready).
    Idle,
    /// Still starting: keep buffering letters.
    Waiting,
    /// The box process exited (or its window disappeared) before it reported ready.
    Exited,
    /// The box is alive but did not report ready in time.
    TimedOut,
}

/// Checked on every key and on a short timer while a session waits for its box, so a box that
/// cannot start is noticed at once instead of on the next key, and one that hangs is given up.
pub fn startup_state(
    session_open: bool,
    ready: bool,
    process_gone: bool,
    overdue: bool,
) -> Startup {
    if !session_open || ready {
        Startup::Idle
    } else if process_gone {
        Startup::Exited
    } else if overdue {
        Startup::TimedOut
    } else {
        Startup::Waiting
    }
}

/// Letters to type into the application when a session ends without its box: before the box was
/// ready it saw none of them, so the first letter and everything buffered go back in order;
/// after it was ready it already had the first letter and the sent ones, so only the letters
/// still waiting to be sent go back. Nothing a user typed is silently dropped.
pub fn keys_to_give_back(first: Option<char>, pending: &str, box_ready: bool) -> String {
    let mut keys = String::with_capacity(pending.len() + 1);
    if let Some(first) = first.filter(|_| !box_ready) {
        keys.push(first);
    }
    keys.push_str(pending);
    keys
}

#[cfg(test)]
mod tests {
    #[test]
    fn commit_digest_tells_texts_apart() {
        assert_eq!(commit_digest("我想试一试"), commit_digest("我想试一试"));
        assert_ne!(commit_digest("我想试一试"), commit_digest("我想试一下"));
        assert_ne!(commit_digest(""), commit_digest("a"));
    }

    #[test]
    fn resent_commit_after_fallback_is_not_inserted_twice() {
        let digest = commit_digest("如果");
        // The box lost the reply to a commit that landed through the focused-field fallback and
        // sends it again: no session, no parked field, same text, moments later.
        assert!(duplicate_commit(false, false, Some((digest, 50)), digest));
        // New input opens a session, so the same word typed again is inserted.
        assert!(!duplicate_commit(true, false, Some((digest, 50)), digest));
        assert!(!duplicate_commit(false, true, Some((digest, 50)), digest));
        assert!(!duplicate_commit(
            false,
            false,
            Some((digest, DUPLICATE_COMMIT_MS)),
            digest
        ));
        assert!(!duplicate_commit(
            false,
            false,
            Some((commit_digest("别的"), 50)),
            digest
        ));
        assert!(!duplicate_commit(false, false, None, digest));
    }

    #[test]
    fn late_confirmation_clears_only_the_matching_draft() {
        let sent = commit_digest("I want to try");
        let kept = commit_digest("我想试一试");
        let pending = Some((7, sent, kept));
        assert!(late_commit_landed(pending, 7, sent, "我想试一试"));
        // Another application's receiver, another text, or no commit waiting.
        assert!(!late_commit_landed(pending, 8, sent, "我想试一试"));
        assert!(!late_commit_landed(pending, 7, kept, "我想试一试"));
        assert!(!late_commit_landed(None, 7, sent, "我想试一试"));
        // The user already changed the kept draft: keep it.
        assert!(!late_commit_landed(pending, 7, sent, "我想试一试吧"));
    }

    use super::*;

    #[test]
    fn focus_events_never_end_a_session_only_keys_from_another_field_do() {
        // Explorer's search box, Chromium, Electron and Word move the focused document around
        // (other child windows, suggestion lists, new documents) while the box has the keyboard.
        // Only a letter typed into another field in the application ends the session.
        assert_eq!(key_route(true, true), KeyRoute::Buffer);
        assert_eq!(key_route(true, false), KeyRoute::StaleSession);
        assert_eq!(key_route(false, false), KeyRoute::NewSession);
    }

    #[test]
    fn only_the_fields_own_switches_block_letters() {
        // Fresh Word: nothing known about the field yet, compartments unreadable: letters go to
        // the box.
        assert!(!FieldSwitches::default().blocked());
        assert!(
            FieldSwitches {
                keyboard_disabled: Some(true),
                ..Default::default()
            }
            .blocked()
        );
        assert!(
            FieldSwitches {
                empty_context: Some(true),
                ..Default::default()
            }
            .blocked()
        );
        assert!(
            FieldSwitches {
                read_only: true,
                ..Default::default()
            }
            .blocked()
        );
        assert!(
            !FieldSwitches {
                keyboard_disabled: Some(false),
                empty_context: Some(false),
                ..Default::default()
            }
            .blocked()
        );
    }

    #[test]
    fn private_scopes_only_mark_privacy() {
        assert!(private_scope(&[31])); // IS_PASSWORD
        assert!(private_scope(&[1, 61])); // IS_PRIVATE (Chromium)
        assert!(private_scope(&[63])); // IS_NUMERIC_PASSWORD
        assert!(private_scope(&[64])); // IS_NUMERIC_PIN
        assert!(private_scope(&[65])); // IS_ALPHANUMERIC_PIN
        assert!(private_scope(&[66])); // IS_ALPHANUMERIC_PIN_SET
        assert!(!private_scope(&[]));
        assert!(!private_scope(&[0, 1, 5])); // default, URL, e-mail
    }

    #[test]
    fn empty_or_off_screen_caret_is_ignored() {
        let screen = [-1920, 0, 1920, 1080];
        assert!(caret_usable([100, 200, 101, 220], screen));
        assert!(caret_usable([-500, 300, -499, 320], screen));
        assert!(!caret_usable([0, 0, 0, 0], screen));
        assert!(!caret_usable([100, 200, 100, 200], screen));
        assert!(!caret_usable([5000, 200, 5001, 220], screen));
        assert!(!caret_usable([100, -3000, 101, -2980], screen));
    }

    #[test]
    fn letters_passed_to_the_application_carry_a_reason() {
        assert_eq!(
            pass_reason(true, false, false, false),
            Some(PassReason::ReadOnly)
        );
        assert_eq!(
            pass_reason(false, true, false, false),
            Some(PassReason::English)
        );
        assert_eq!(
            pass_reason(false, false, false, true),
            Some(PassReason::Password)
        );
        // Capitals and shortcuts are expected to reach the application: not logged.
        assert_eq!(pass_reason(false, false, true, true), None);
        assert_eq!(pass_reason(false, false, false, false), None);
        assert!(!PassReason::BoxUnavailable.describe().is_empty());
    }

    #[test]
    fn a_commit_is_never_refused_while_a_field_is_known() {
        use CommitTarget::*;
        assert_eq!(
            commit_targets(true, true, true, false),
            [Session, Held, Focused]
        );
        // Session ended by a focus event before the box committed: the parked field, then the
        // focused one; never nothing while the user is in a text field.
        assert_eq!(commit_targets(false, true, true, false), [Held, Focused]);
        assert_eq!(commit_targets(false, false, true, false), [Focused]);
        assert!(commit_targets(false, false, false, false).is_empty());
    }

    #[test]
    fn a_document_replaced_in_the_same_window_gets_the_commit() {
        use CommitTarget::*;
        // Fresh Word: the first key opened the session on the start-up document, Word then
        // focused its real one in the same window. The commit goes to the focused context, the
        // remembered one only if that fails.
        assert_eq!(commit_targets(true, false, true, true), [Focused, Session]);
        assert_eq!(
            commit_targets(true, true, true, true),
            [Focused, Session, Held]
        );
        // Nothing focused: the session's own context.
        assert_eq!(commit_targets(true, false, false, true), [Session]);
    }

    #[test]
    fn letters_typed_while_the_box_starts_are_given_back() {
        // The box process exited before it was ready: "n" (first key) and "i" (buffered) both
        // go to the application, in order.
        assert_eq!(keys_to_give_back(Some('n'), "i", false), "ni");
        assert_eq!(keys_to_give_back(Some('n'), "", false), "n");
        // After ready the box had the first letter; only unsent letters come back.
        assert_eq!(keys_to_give_back(Some('n'), "ih", true), "ih");
        assert_eq!(keys_to_give_back(None, "", false), "");
    }

    #[test]
    fn startup_failures_are_noticed_and_classified() {
        assert_eq!(startup_state(false, false, true, true), Startup::Idle);
        assert_eq!(startup_state(true, true, true, true), Startup::Idle);
        assert_eq!(startup_state(true, false, false, false), Startup::Waiting);
        assert_eq!(startup_state(true, false, true, false), Startup::Exited);
        assert_eq!(startup_state(true, false, true, true), Startup::Exited);
        assert_eq!(startup_state(true, false, false, true), Startup::TimedOut);
    }

    #[test]
    fn a_box_that_cannot_start_backs_off_then_recovers() {
        let mut health = ServiceHealth::default();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(health.before_activation(None, at(0)), Activation::Use);
        // The started box crashed: for 2 s keys go to the application instead.
        health.start_failed(at(0));
        assert_eq!(health.before_activation(None, at(1000)), Activation::Wait);
        // A running shared service is still used meanwhile.
        assert_eq!(health.before_activation(Some(5), at(1000)), Activation::Use);
        assert_eq!(health.before_activation(None, at(2000)), Activation::Use);
        // Failing again doubles the wait, capped at one minute.
        health.start_failed(at(2000));
        assert_eq!(health.start_backoff(), Duration::from_secs(4));
        assert_eq!(health.before_activation(None, at(5999)), Activation::Wait);
        assert_eq!(health.before_activation(None, at(6000)), Activation::Use);
        for n in 0..10 {
            health.start_failed(at(6000 + n));
        }
        assert_eq!(health.start_backoff(), MAX_BACKOFF);
        // A box that reports ready clears it.
        health.started();
        assert_eq!(health.before_activation(None, at(6100)), Activation::Use);
        // A failure long after the last one starts over at 2 s.
        health.start_failed(at(0));
        health.start_failed(at(400_000));
        assert_eq!(health.start_backoff(), FIRST_BACKOFF);
    }

    #[test]
    fn healthy_service_is_used() {
        let mut health = ServiceHealth::default();
        let now = Instant::now();
        assert_eq!(health.before_activation(Some(7), now), Activation::Use);
        assert_eq!(health.before_activation(None, now), Activation::Use);
        health.timed_out(7);
        health.responded(7);
        assert_eq!(health.before_activation(Some(7), now), Activation::Use);
    }

    #[test]
    fn timeout_restarts_the_hung_process_on_next_activation() {
        let mut health = ServiceHealth::default();
        let now = Instant::now();
        health.timed_out(7);
        assert!(health.is_unhealthy(7));
        assert_eq!(
            health.before_activation(Some(7), now),
            Activation::Restart { kill: 7 }
        );
        // The new process is healthy.
        assert_eq!(health.before_activation(Some(8), now), Activation::Use);
        // A hung process that is already gone needs no kill.
        health.timed_out(8);
        assert_eq!(health.before_activation(Some(9), now), Activation::Use);
        assert!(!health.is_unhealthy(8));
    }

    #[test]
    fn repeated_hangs_back_off_exponentially_and_reset_later() {
        let mut health = ServiceHealth::default();
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        health.timed_out(1);
        assert_eq!(
            health.before_activation(Some(1), at(0)),
            Activation::Restart { kill: 1 }
        );
        // The restarted service hangs again at once: wait 2 s, then restart.
        health.timed_out(2);
        assert_eq!(health.before_activation(Some(2), at(1)), Activation::Wait);
        assert_eq!(
            health.before_activation(Some(2), at(2)),
            Activation::Restart { kill: 2 }
        );
        health.timed_out(3);
        assert_eq!(health.backoff(), Duration::from_secs(4));
        assert_eq!(health.before_activation(Some(3), at(5)), Activation::Wait);
        assert_eq!(
            health.before_activation(Some(3), at(6)),
            Activation::Restart { kill: 3 }
        );
        // Capped at one minute.
        for _ in 0..10 {
            health.restarts += 1;
        }
        assert_eq!(health.backoff(), MAX_BACKOFF);
        // Long quiet period: the next hang restarts immediately again.
        health.timed_out(4);
        assert_eq!(
            health.before_activation(Some(4), at(6 + 300)),
            Activation::Restart { kill: 4 }
        );
        assert_eq!(health.backoff(), FIRST_BACKOFF);
    }

    fn medium(session: u32, user: &[u8]) -> ProcessSecurity {
        ProcessSecurity {
            elevated: false,
            integrity: MEDIUM_INTEGRITY,
            session,
            user: user.to_vec(),
        }
    }

    #[test]
    fn elevated_hosts_use_a_private_editor() {
        let normal = medium(1, b"alice");
        assert!(!needs_private_editor(Some(&normal)));
        let elevated = ProcessSecurity {
            elevated: true,
            integrity: HIGH_INTEGRITY,
            ..normal.clone()
        };
        assert!(needs_private_editor(Some(&elevated)));
        // High integrity without the elevation flag (e.g. UIAccess, services) too.
        let high = ProcessSecurity {
            integrity: HIGH_INTEGRITY,
            ..normal.clone()
        };
        assert!(needs_private_editor(Some(&high)));
        // Unknown token: the safe choice.
        assert!(needs_private_editor(None));
        // Low integrity (sandboxed browser tab) may share its own level's service.
        let low = ProcessSecurity {
            integrity: 0x1000,
            ..normal.clone()
        };
        assert!(!needs_private_editor(Some(&low)));
        // A private host never connects to any shared service.
        assert!(!may_share_service(&elevated, Some(&elevated)));
    }

    #[test]
    fn shared_service_must_match_user_session_and_level() {
        let host = medium(1, b"alice");
        assert!(may_share_service(&host, Some(&medium(1, b"alice"))));
        assert!(!may_share_service(&host, None), "unknown service");
        assert!(
            !may_share_service(&host, Some(&medium(2, b"alice"))),
            "other session"
        );
        assert!(
            !may_share_service(&host, Some(&medium(1, b"bob"))),
            "other user"
        );
        let elevated = ProcessSecurity {
            elevated: true,
            integrity: HIGH_INTEGRITY,
            ..medium(1, b"alice")
        };
        assert!(
            !may_share_service(&host, Some(&elevated)),
            "elevated service"
        );
        let low_service = ProcessSecurity {
            integrity: 0x1000,
            ..medium(1, b"alice")
        };
        assert!(!may_share_service(&host, Some(&low_service)), "other level");
    }

    #[test]
    fn window_titles_per_level() {
        assert_eq!(
            service_window_title(MEDIUM_INTEGRITY, false),
            "YiIME 译 · TSF · p3"
        );
        assert_eq!(
            service_window_title(0x1000, false),
            "YiIME 译 · TSF · IL1000 · p3"
        );
        // 0.2.0-rc.1 and earlier used unversioned names; they must not match the new ones.
        assert_ne!(
            service_window_title(MEDIUM_INTEGRITY, false),
            "YiIME 译 · TSF"
        );
        assert_eq!(
            TSF_RECEIVER_CLASS,
            format!("YiIMETsfReceiver.{TSF_PROTOCOL}")
        );
        assert_ne!(TSF_RECEIVER_CLASS, "InputTranslationTsfReceiver");
        assert_ne!(
            service_window_title(HIGH_INTEGRITY, true),
            service_window_title(HIGH_INTEGRITY, false)
        );
    }

    #[test]
    fn watchdog_tolerates_start_up_and_detects_a_stuck_loop() {
        let watchdog = Watchdog {
            limit_ms: 5_000,
            grace_ms: 20_000,
        };
        // Start-up (dictionary load) never counts, even without beats.
        assert!(!watchdog.stalled(0, 0));
        assert!(!watchdog.stalled(0, 19_999));
        // No beat at all: measured from the end of the grace period.
        assert!(!watchdog.stalled(0, 25_000));
        assert!(watchdog.stalled(0, 25_001));
        // Regular beats keep it alive.
        let mut beat = 0;
        for now in (20_000..60_000).step_by(1_000) {
            beat = now;
            assert!(!watchdog.stalled(beat, now + 999));
        }
        // The loop stops beating (stalled heartbeat).
        assert!(!watchdog.stalled(beat, beat + 5_000));
        assert!(watchdog.stalled(beat, beat + 5_001));
        // A beat during the grace period counts from the end of the grace period.
        assert!(!watchdog.stalled(10_000, 24_000));
        assert!(watchdog.stalled(10_000, 25_001));
    }

    #[test]
    fn watched_hosts_follow_reused_window_handles() {
        let first = HostOwner {
            pid: 100,
            thread: 7,
        };
        let second = HostOwner {
            pid: 200,
            thread: 9,
        };
        let mut hosts = WatchedHosts::default();
        assert!(hosts.session_started(0x10, Some(first)));
        // The same host again (another session, same process): already watched.
        assert!(!hosts.session_started(0x10, Some(first)));
        assert_eq!(hosts.len(), 1);
        // The handle is reused by another process: watch the new owner.
        assert!(hosts.session_started(0x10, Some(second)));
        assert_eq!(hosts.len(), 1);
        // The old owner's exit notice arrives late: it must not end the new host's session.
        assert!(!hosts.host_exited(0x10, 100));
        assert!(hosts.host_exited(0x10, 200));
        assert!(hosts.is_empty());
        // After the exit the entry is gone, so a reused handle (even with a reused pid) is watched.
        assert!(hosts.session_started(0x10, Some(second)));
    }

    #[test]
    fn watched_hosts_drop_receivers_that_are_gone() {
        let owner = HostOwner { pid: 5, thread: 6 };
        let mut hosts = WatchedHosts::default();
        assert!(hosts.session_started(1, Some(owner)));
        assert!(!hosts.session_started(1, None));
        assert!(!hosts.session_started(2, Some(HostOwner { pid: 0, thread: 0 })));
        assert!(hosts.is_empty());
        assert!(!hosts.host_exited(1, 5));
    }

    #[test]
    fn empty_unfocused_boxes_are_hidden() {
        let empty = BoxView {
            visible: true,
            empty: true,
            foreground: false,
            busy: false,
            shown_for_ms: 0,
        };
        // Focus moved in the host (e.g. from a text field to the page): hide at once.
        assert!(hide_empty_box(empty, true));
        // Without a focus notice (lost or never sent): after the grace period.
        assert!(!hide_empty_box(empty, false));
        assert!(hide_empty_box(
            BoxView {
                shown_for_ms: EMPTY_BOX_GRACE_MS,
                ..empty
            },
            false
        ));
        // Kept: content, a translation in flight, the user typing in it, or already hidden.
        for keep in [
            BoxView {
                empty: false,
                ..empty
            },
            BoxView {
                busy: true,
                ..empty
            },
            BoxView {
                foreground: true,
                ..empty
            },
            BoxView {
                visible: false,
                ..empty
            },
        ] {
            assert!(!hide_empty_box(keep, true));
            assert!(!hide_empty_box(
                BoxView {
                    shown_for_ms: 60_000,
                    ..keep
                },
                false
            ));
        }
    }

    #[test]
    fn shift_tap_in_an_empty_box_switches_to_english_and_closes() {
        use ShiftTapAction::*;
        assert_eq!(shift_tap_action(true, true, true, false), SwitchToEnglish);
        // Something to keep: the tap toggles the English segment, the box stays.
        assert_eq!(shift_tap_action(true, false, true, false), ToggleSegment);
        assert_eq!(shift_tap_action(true, true, false, false), ToggleSegment);
        // Leaving an English segment that is still empty: back to Chinese in the box.
        assert_eq!(shift_tap_action(true, true, true, true), ToggleSegment);
        // The standalone box has no application to switch.
        assert_eq!(shift_tap_action(false, true, true, false), ToggleSegment);
    }

    #[test]
    fn switching_the_host_to_english_closes_the_box() {
        let shown = BoxView {
            visible: true,
            empty: true,
            ..BoxView::default()
        };
        assert!(english_ends_session(shown));
        // Empty or not, and even when it still has the foreground.
        assert!(english_ends_session(BoxView {
            empty: false,
            foreground: true,
            ..shown
        }));
        // A translation in flight stays until its result is committed; a hidden box is left.
        assert!(!english_ends_session(BoxView {
            busy: true,
            ..shown
        }));
        assert!(!english_ends_session(BoxView {
            visible: false,
            ..shown
        }));
    }

    #[test]
    fn a_session_for_another_field_never_blocks_input() {
        assert_eq!(key_route(true, true), KeyRoute::Buffer);
        assert_eq!(key_route(true, false), KeyRoute::StaleSession);
        assert_eq!(key_route(false, false), KeyRoute::NewSession);
        assert_eq!(key_route(false, true), KeyRoute::NewSession);
        // After the DLL drops the stale session it tells the box. The box closes even with a
        // draft, unless a translation is in flight (that Chinese is committed to the old field).
        let lingering = BoxView {
            visible: true,
            empty: false,
            ..BoxView::default()
        };
        assert!(english_ends_session(lingering));
        assert_eq!(left_field(false), LeftField::Discard);
        assert_eq!(left_field(true), LeftField::CommitSource);
    }
}
