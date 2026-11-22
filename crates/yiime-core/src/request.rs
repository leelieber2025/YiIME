//! Asynchronous requests (translations) that finish after the session that started them may be
//! gone. The macOS input method commits a translation only if [`RequestGate::finish`] says the
//! request is still the current one: switching applications, deactivating the input method or
//! starting another request makes an earlier result stale, and it is dropped instead of being
//! typed into whatever has the focus now.

use std::sync::atomic::{AtomicU64, Ordering};

/// Request ids are unique in the process, so a session that was reset (or recreated) never
/// mistakes an old request for its own.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Default)]
pub struct RequestGate {
    /// Id of the request whose result may still be used; 0 when none.
    current: u64,
}

impl RequestGate {
    /// Starts a request and returns its id (never 0). Any earlier request becomes stale.
    pub fn begin(&mut self) -> u64 {
        self.current = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.current
    }

    /// The session ended or moved to another client: no pending result may be used.
    pub fn cancel(&mut self) {
        self.current = 0;
    }

    /// Whether a request is pending.
    pub fn pending(&self) -> bool {
        self.current != 0
    }

    /// Request `id` completed: true if its result may be used (it is still the current request),
    /// at most once.
    pub fn finish(&mut self, id: u64) -> bool {
        let current = id != 0 && id == self.current;
        if current {
            self.current = 0;
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_current_request_may_commit_once() {
        let mut gate = RequestGate::default();
        assert!(!gate.pending());
        let first = gate.begin();
        assert!(gate.pending());
        assert!(gate.finish(first));
        assert!(!gate.finish(first), "a result is used at most once");
        assert!(!gate.finish(0));
    }

    #[test]
    fn cancel_or_a_newer_request_makes_results_stale() {
        let mut gate = RequestGate::default();
        let first = gate.begin();
        // App switch / input method deactivated while translating.
        gate.cancel();
        assert!(!gate.finish(first));
        let second = gate.begin();
        let third = gate.begin();
        assert!(!gate.finish(second));
        assert!(gate.finish(third));
        // A session that was reset starts from scratch but never reuses an id.
        let mut fresh = RequestGate::default();
        let fourth = fresh.begin();
        assert_ne!(fourth, first);
        assert!(!fresh.finish(first));
        assert!(fresh.finish(fourth));
    }
}
