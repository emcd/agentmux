//! The notify-only signal that a target's mailbox is worth reading.

use std::sync::Arc;

use tokio::sync::Notify;

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
#[derive(Clone, Debug, Default)]
pub struct DeliveryDoorbell {
    notify: Arc<Notify>,
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
        self.notify.notify_one();
    }

    /// Waits for the next ring.
    ///
    /// A caller must treat waking as a hint to peek, never as evidence that
    /// anything is there — and must pair this with its own bounded poll, since a
    /// ring that is never observed is a case the protocol permits.
    pub async fn rung(&self) {
        self.notify.notified().await;
    }
}
