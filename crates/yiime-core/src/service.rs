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

    /// Decides at activation; `running` is the pid of the service found running (if any).
    pub fn before_activation(&mut self, running: Option<u32>, now: Instant) -> Activation {
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

/// Window title of the input box: the shared service has one per integrity level (FindWindow only
/// sees the caller's session and desktop, and the DLL checks the process as well); a private box
/// has its own title so no other application finds it.
pub fn service_window_title(integrity: u32, private: bool) -> String {
    if private {
        "YiIME 译 · TSF（私有）".to_owned()
    } else if integrity == MEDIUM_INTEGRITY {
        // Unchanged name, so a 0.1.x DLL still finds it.
        "YiIME 译 · TSF".to_owned()
    } else {
        format!("YiIME 译 · TSF · IL{integrity:x}")
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "YiIME 译 · TSF"
        );
        assert_eq!(
            service_window_title(0x1000, false),
            "YiIME 译 · TSF · IL1000"
        );
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
}
