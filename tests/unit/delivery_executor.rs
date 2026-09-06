//! What the delivery-loop executor does when the doorbell never arrives.
//!
//! The doorbell is a hint and nothing more: it carries no data, takes no
//! custody, and the protocol permits a ring to be lost. Every correctness
//! argument in the delivery model therefore rests on the *bounded poll* the
//! executor pairs it with, and nothing that reads a ring may depend on having
//! read one.
//!
//! Two existing tests bracket that claim without making it. `doorbell.rs`'s
//! inline test pins when the relay decides to ring, which is a question about
//! the ledger. `delivery_protocol.rs` pins that the `DeliveryDoorbell` type
//! retains a ring and reports a timeout, which is a question about the type.
//! Neither drives an executor, so neither can say what happens to an entry whose
//! ring was never made.
//!
//! Driven through `agentmux::transports`, the same seam a real transport is
//! constructed against.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agentmux::envelope::AddressIdentity;
use agentmux::protocol::mailbox::{EntrySequence, MailboxEntry, MailboxPayload};
use agentmux::protocol::{DeliveryDoorbell, DeliveryEnvelope, DeliveryMessage};
use agentmux::transports::{
    DeliveryExecutorContext, DeliveryWriter, PeekDimensions, PlannedWrite, SubmissionEvidence,
    TransportHealth, run_delivery_executor,
};

use super::stub_mailbox::StubMailbox;

/// How long the harness will wait for an entry the executor is expected to
/// deliver.
///
/// Deliberately far larger than the poll interval below, because this test is
/// not about how promptly the backstop fires. A bound tight enough to measure
/// the poll would fail under load for a reason that says nothing about the
/// property under test; a generous one still fails, and only fails, if the
/// executor never wakes at all.
const DELIVERY_WINDOW: Duration = Duration::from_secs(10);

/// The backstop being exercised. Short so the test is quick, and never asserted
/// against.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A writer that submits whatever it is shown and records nothing else.
///
/// Deliberately featureless: readiness, health and planning are other tests'
/// subjects, and a stub that could decline to write would let a failure here be
/// explained by something other than the wake it was waiting for.
struct AlwaysWrites {
    stop: Arc<AtomicBool>,
    waits: Arc<AtomicUsize>,
    rings_observed: Arc<AtomicUsize>,
}

impl DeliveryWriter for AlwaysWrites {
    type Plan = usize;

    fn peek_dimensions(&self) -> PeekDimensions {
        PeekDimensions {
            envelopes_max: 8,
            canonical_bytes_max: 1_000_000,
        }
    }

    fn health(&self) -> TransportHealth {
        TransportHealth::Healthy
    }

    fn is_ready(&mut self) -> bool {
        true
    }

    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>> {
        Some(PlannedWrite {
            entry_count: entries.len(),
            rendered: entries.len(),
        })
    }

    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence> {
        vec![SubmissionEvidence::Submitted; planned.rendered]
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Counts the waits, and separately counts those a ring actually ended.
    ///
    /// The default implementation is what production uses; this overrides it only
    /// to observe, and calls straight through so the timing is the real one. The
    /// two counters are what let the assertions below distinguish "woken by the
    /// backstop" from "woken by a ring", which is the whole distinction the test
    /// rests on.
    fn wait_for_work(&mut self, doorbell: &DeliveryDoorbell, timeout: Duration) {
        self.waits.fetch_add(1, Ordering::SeqCst);
        if doorbell.wait_for(timeout) {
            self.rings_observed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn identity(name: &str) -> AddressIdentity {
    AddressIdentity {
        session_name: name.to_string(),
        display_name: None,
    }
}

fn entry(sequence: u64, message_id: &str) -> MailboxEntry {
    MailboxEntry {
        sequence: EntrySequence::new(sequence).expect("a mailbox position is one-based"),
        message_id: message_id.to_string(),
        canonical_bytes: 4,
        payload: MailboxPayload::Mail(Arc::new(DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: "ship it".to_string(),
                created_at: "2026-08-29T12:00:00Z".to_string(),
                namespace: "party".to_string(),
                sender: identity("alice@party"),
                target: identity("bob@party"),
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
        })),
    }
}

/// Spins until `condition` holds, or fails with `whats_missing` at the window.
fn within_the_window(condition: impl Fn() -> bool, whats_missing: &str) {
    let started = Instant::now();
    while started.elapsed() < DELIVERY_WINDOW {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("{whats_missing} within {DELIVERY_WINDOW:?}");
}

/// An entry that arrives while the executor is already idle is delivered even
/// though nothing rings for it.
///
/// The arrival has to happen *after* the executor is waiting, which is why the
/// mailbox starts empty and is filled by the test rather than seeded. A seeded
/// entry is drained on the loop's first pass, before any wait, so it would prove
/// only that the executor runs once — the reading that would let this test pass
/// with the backstop removed entirely.
///
/// Nothing here asserts how *quickly* the entry arrives. The claim is that
/// correctness does not depend on a ring, not that the timer is punctual, and an
/// assertion on elapsed time would fail under load for an unrelated reason.
#[test]
fn an_entry_whose_ring_is_never_made_is_still_delivered_by_the_poll() {
    let mailbox = Arc::new(StubMailbox::empty());
    let stop = Arc::new(AtomicBool::new(false));
    let waits = Arc::new(AtomicUsize::new(0));
    let rings_observed = Arc::new(AtomicUsize::new(0));

    // The relay's end of the doorbell, held and never rung. Holding it rather
    // than omitting it is the point: the executor is waiting on a live doorbell
    // that simply never receives a ring, which is the lost-notification case, not
    // a degenerate one where no doorbell exists.
    let doorbell = DeliveryDoorbell::new();
    let writer = AlwaysWrites {
        stop: Arc::clone(&stop),
        waits: Arc::clone(&waits),
        rings_observed: Arc::clone(&rings_observed),
    };
    let context = DeliveryExecutorContext {
        consumer: Arc::clone(&mailbox) as Arc<_>,
        doorbell: doorbell.clone(),
        poll_interval: POLL_INTERVAL,
        unreachable_dwell: Duration::from_secs(3_600),
    };
    let executor = std::thread::spawn(move || run_delivery_executor(writer, context));

    // Wait until the executor has actually parked. Placing the entry before this
    // would let the loop's first pass find it, and the test would pass without
    // the backstop ever running.
    within_the_window(
        || waits.load(Ordering::SeqCst) > 0,
        "the executor never reached its first wait",
    );

    mailbox.place(entry(1, "doorbell-miss-1"));
    within_the_window(
        || !mailbox.acked().is_empty(),
        "an entry placed with no ring was never delivered",
    );

    // The load-bearing negative, read here rather than after the shutdown below.
    // Nothing has rung at this point, so this is provably zero and the delivery
    // above cannot be explained by a ring — which is the reading that would let
    // this test pass against an executor that ignored its poll entirely.
    //
    // Asserting it after the shutdown ring instead would make the test depend on
    // the executor being parked when that ring lands: one that observes the stop
    // flag at the top of the loop returns without waiting again, so the ring is
    // never consumed and a count of one never arrives. That is a race in the
    // assertion rather than in the code, and it fails intermittently under a
    // loaded suite.
    assert_eq!(
        rings_observed.load(Ordering::SeqCst),
        0,
        "the entry was delivered without any ring having been observed"
    );

    stop.store(true, Ordering::SeqCst);
    doorbell.ring();
    executor.join().expect("the executor thread ends");

    let acked = mailbox.acked();
    assert_eq!(
        acked.len(),
        1,
        "exactly the placed entry was acknowledged, once"
    );
    assert_eq!(acked[0].message_id, "doorbell-miss-1");
    assert_eq!(
        acked[0].evidence,
        SubmissionEvidence::Submitted,
        "and it was reported as written rather than resolved some other way"
    );
}
