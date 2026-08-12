//! Relay-side batch formation against a transport's maximum handover dimensions.
//!
//! The relay decides how much pending work a target's transport may be holding at
//! once; the transport decides how to pack what it receives. That division is why
//! the bounds here are envelope count and canonical payload bytes and never
//! tokens: only the transport can render and count tokens, so a relay-side budget
//! expressed in them would be circular.
//!
//! A *window* is one handover's worth of work: what the target's transport is
//! currently holding, submitted before the relay waits, closed when the last of
//! it resolves. Members that do not fit stay `Pending` with their quota still
//! reserved, and are offered again once the window closes — a window is always a
//! prefix of the target's FIFO, so bounding one can delay a member but can never
//! reorder two.
//!
//! # A window is not a batch
//!
//! A window bounds *how much* is handed over. A **batch** is the unit of
//! authorization, and the contract requires its membership be fixed before any
//! member is authorized: no envelope may be absorbed into a batch after that
//! batch is authorized, because mutable batch membership is precisely what let
//! one outcome be reported for members that were written and members that were
//! not. A window accumulates as members arrive, so it is *not* a set authorized
//! together and its extent must not be used as a batch.
//!
//! The two stay distinct now that batches are formed rather than minted per
//! member. A batch is the prefix drained at one instant, fixed and authorized
//! whole; a window stays open until nothing is in flight, so it can span several
//! batches authorized at different instants. That is exactly the difference the
//! contract cares about, and it is why the window's counters bound the drain and
//! never name the set.

use crate::transports::{HandoverDimensions, TransportImpl};

/// How much of a target's pending work is currently handed over to its transport.
///
/// `Copy` so a batch can be proposed against a scratch copy and the real window
/// advanced only once that batch is authorized. Membership is fixed before
/// authorization, so a proposal that authorization then refuses must leave the
/// window exactly as it found it — otherwise the window would be reserving room
/// for work no transport is holding.
#[derive(Clone, Copy)]
pub(super) struct HandoverWindow {
    /// `None` for a transport that accepts no handover at all (`Pubsub`), which
    /// is refused before it reaches here. Treated as unbounded rather than
    /// closed, because bounding a transport the relay never submits to would be
    /// a rule with no subject.
    dimensions: Option<HandoverDimensions>,
    envelopes: usize,
    bytes: u64,
}

impl HandoverWindow {
    pub(super) fn for_transport(transport: &TransportImpl) -> Self {
        Self {
            dimensions: transport.maximum_handover_dimensions(),
            envelopes: 0,
            bytes: 0,
        }
    }

    /// Whether the window could accept any further member at all.
    ///
    /// Gates whether the loop takes another task off the channel. Deliberately
    /// asked *before* a candidate is known — a full window should stop the worker
    /// receiving, rather than receive a member it would then have to hold.
    pub(super) fn has_room(&self) -> bool {
        let Some(dimensions) = self.dimensions else {
            return true;
        };
        self.envelopes < dimensions.envelopes_max && self.bytes < dimensions.canonical_bytes_max
    }

    /// Whether this specific candidate fits the room that remains.
    ///
    /// # An empty window always admits
    ///
    /// Even a candidate larger than the whole byte ceiling is admitted into an
    /// empty window. Admission already rejects an envelope whose canonical
    /// payload exceeds the target transport's ceiling, so an admitted candidate
    /// provably fits alone; this is a stall guard rather than a tolerance.
    /// Refusing a head-of-queue member no window could ever hold would park that
    /// target's queue permanently, turning a bound that exists to *split* work
    /// into one that silently stops it.
    pub(super) fn admits(&self, candidate_bytes: u64) -> bool {
        let Some(dimensions) = self.dimensions else {
            return true;
        };
        if self.envelopes == 0 {
            return true;
        }
        if self.envelopes >= dimensions.envelopes_max {
            return false;
        }
        // Saturating because the sum is bounded by neither the queue nor the
        // ceiling: a wrap here would silently admit an unbounded batch, which is
        // the one outcome this check exists to prevent.
        self.bytes.saturating_add(candidate_bytes) <= dimensions.canonical_bytes_max
    }

    /// Records an admitted member against the window's budget.
    ///
    /// Deliberately returns nothing. An earlier revision returned a `BatchId`
    /// minted on the first member and shared with every later one, which absorbed
    /// envelopes into a batch that was already authorized — the mutable batch
    /// membership the contract forbids by name.
    pub(super) fn record(&mut self, candidate_bytes: u64) {
        self.envelopes += 1;
        self.bytes = self.bytes.saturating_add(candidate_bytes);
    }

    /// Closes the window once nothing is in flight, so the next member is offered
    /// a full handover's worth of room again.
    pub(super) fn close(&mut self) {
        self.envelopes = 0;
        self.bytes = 0;
    }
}

/// A window bounds by both components, floors at one member, and reopens on close.
///
/// Inline because `HandoverWindow` is crate-private by design: it is the relay's
/// half of a division of labour with the transports, and publishing it to reach
/// it from an integration test would expose a scheduling decision that has no
/// business being callable from outside `dispatch`. No public interface isolates
/// it either — a send exercises it only through a live transport, where the
/// window's arithmetic is invisible behind whatever the target does with the
/// envelopes.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_stops_at_whichever_component_binds_first() {
        // Stand-in dimensions rather than a real transport's: the arithmetic is
        // what is under test, and small numbers make which component bound
        // legible in the assertion rather than arithmetic the reader must redo.
        let dimensions = HandoverDimensions {
            envelopes_max: 3,
            canonical_bytes_max: 100,
        };
        let window = || HandoverWindow {
            dimensions: Some(dimensions),
            envelopes: 0,
            bytes: 0,
        };

        // THE FLOOR. An empty window admits anything, including a candidate over
        // the whole ceiling. Admission proves that cannot happen; refusing it
        // here would park the target's queue forever.
        assert!(window().admits(9_999));
        assert!(window().has_room());

        // THE COUNT BINDS. Three one-byte members are nowhere near the byte
        // ceiling, so only the envelope cap can close this window.
        let mut counting = window();
        for _ in 0..3 {
            assert!(counting.has_room());
            counting.record(1);
        }
        assert!(!counting.has_room(), "the envelope cap closed the window");
        assert!(!counting.admits(1));

        // THE BYTES BIND, well under the envelope cap.
        let mut weighing = window();
        weighing.record(60);
        assert!(weighing.has_room(), "one member of three, 60 of 100 bytes");
        assert!(!weighing.admits(60), "60 + 60 overruns the byte ceiling");
        assert!(weighing.admits(40), "but exactly filling it is allowed");

        // The ceiling is inclusive: a sum landing exactly on it fits, and then
        // closes the window.
        weighing.record(40);
        assert!(!weighing.has_room(), "100 of 100 bytes leaves no room");

        // SATURATING, NOT WRAPPING. The fixture is chosen so the two differ:
        // `u64::MAX + 101` wraps to exactly 100, which is under the ceiling and
        // would be admitted. Two `u64::MAX` values would NOT discriminate — they
        // wrap to `MAX - 1`, still over the ceiling, so that pair passes either
        // way and proves nothing.
        let mut overflowing = window();
        overflowing.record(u64::MAX);
        assert!(!overflowing.admits(101));

        // CLOSING RESTORES A WHOLE HANDOVER'S ROOM. A window that had filled
        // admits again once nothing is in flight, which is what keeps a bound
        // from becoming a permanent stop.
        let mut reopening = window();
        reopening.record(100);
        assert!(!reopening.has_room());
        reopening.close();
        assert!(reopening.has_room());
        assert!(reopening.admits(100));
    }
}
