//! The notify-only signal that a target's mailbox is worth reading.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A hint that a target's mailbox has gained content, carrying no data and no
/// custody.
///
/// Ringing transfers nothing: the entries stay in the relay's custody, and a
/// woken executor learns what arrived only by peeking. That is what keeps a lost
/// notification harmless — it costs the delay until the executor's next bounded
/// poll and nothing else, so no correctness argument anywhere may rest on a ring
/// being observed.
///
/// Cloning yields another handle on the same doorbell: the relay holds one to
/// ring, the target's delivery executor holds one to wait on. The doorbell is
/// built fresh for each consumer generation and is never persisted, because there
/// is no state in it worth carrying across one.
///
/// **The wait is blocking rather than async**, because every delivery-loop
/// executor runs on a dedicated thread — ACP, Tmux and Pty each own one for
/// reasons of their own (a blocking write seam, and in Pty's case a `!Send`
/// terminal that cannot leave its thread), and UI joined them when it became one
/// serial executor rather than a thread per delivery. A `tokio::sync::Notify`
/// cannot be waited on from such a thread without a runtime handle, and handing
/// one down would put a runtime dependency in the neutral boundary for the
/// benefit of no caller.
#[derive(Clone, Debug, Default)]
pub struct DeliveryDoorbell {
    inner: Arc<Doorbell>,
}

/// The retained signal and the condition variable waiters park on.
///
/// A flag rather than a count: a doorbell says only *that* a peek is worth
/// making, never how many entries prompted it, so several rings observed as one
/// wake lose nothing — the executor peeks and finds whatever arrived.
#[derive(Debug, Default)]
struct Doorbell {
    rung: Mutex<bool>,
    changed: Condvar,
}

impl DeliveryDoorbell {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Signals that the mailbox is worth reading.
    ///
    /// Never blocks and never fails. With no executor waiting, the signal is
    /// retained for the next wait rather than dropped, so an entry admitted just
    /// before an executor begins waiting does not sit until the poll backstop.
    pub fn ring(&self) {
        let Ok(mut rung) = self.inner.rung.lock() else {
            // A poisoned doorbell has lost nothing that matters. The wait below
            // treats the same poisoning as a wake, so the executor still peeks;
            // failing the ring loudly would turn a notification the contract
            // already permits to be lost into a delivery error.
            return;
        };
        *rung = true;
        self.inner.changed.notify_all();
    }

    /// Waits up to `timeout` for a ring, reporting whether one was observed.
    ///
    /// A caller must treat waking as a hint to peek, never as evidence that
    /// anything is there — and must pair this with its own bounded poll, since a
    /// ring that is never observed is a case the protocol permits. Passing that
    /// poll interval as `timeout` is how the two are paired: the call returns on
    /// whichever comes first, and the caller peeks either way.
    ///
    /// Consumes the retained signal when it observes one, so a single ring wakes
    /// one wait rather than every subsequent one.
    pub fn wait_for(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let Ok(mut rung) = self.inner.rung.lock() else {
            return false;
        };
        while !*rung {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let Ok((guard, _)) = self.inner.changed.wait_timeout(rung, remaining) else {
                return false;
            };
            rung = guard;
        }
        *rung = false;
        true
    }
}
