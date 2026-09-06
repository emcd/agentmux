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

/// A writer that takes one entry per write and refuses to be ready again until
/// the test says so.
///
/// This is the shape of an ACP worker mid-turn. `AcpDeliveryWriter::is_ready`
/// admits only `Available`, so a worker that accepted a turn and published `Busy`
/// fails the same gate this stub fails — and the gate, not anything ACP-specific,
/// is what decides the fate of the entries behind the one it took.
struct BusyAfterOneTurn {
    stop: Arc<AtomicBool>,
    available: Arc<AtomicBool>,
    writes: Arc<AtomicUsize>,
}

impl DeliveryWriter for BusyAfterOneTurn {
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

    /// The Busy gate. Nothing else about the transport changes while it is mid
    /// turn: it is reachable, its channel is open, and it will be ready again.
    fn is_ready(&mut self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    /// One entry per write, which is what makes members two onward the subjects
    /// of this test rather than groupmates of member one.
    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>> {
        entries.first().map(|_| PlannedWrite {
            entry_count: 1,
            rendered: 1,
        })
    }

    /// Accepting the turn is what makes the worker busy, so the flip happens
    /// here rather than on a timer — the same ordering ACP has, where the
    /// transport publishes `Busy` on accepting a prompt.
    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.available.store(false, Ordering::SeqCst);
        vec![SubmissionEvidence::Submitted; planned.rendered]
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// A target that goes busy after its first envelope leaves the rest queued and
/// undeclared, and delivers them when it is ready again.
///
/// This is the scenario that motivated the pull model. Under the push model the
/// relay sized a batch, handed the whole of it to the transport, and a transport
/// that could take only the first envelope left the relay holding members it had
/// already committed — which surfaced to their senders as `not_submitted`, a
/// positive claim of non-delivery for messages that were simply never attempted.
///
/// The pull model's answer is not a better spelling for those members. It is that
/// nothing resolves them at all: the executor peeks, writes what it planned, and
/// the entries behind it stay exactly where they were, unbound and unread, until
/// a later pass finds the target ready. So the assertion worth making is an
/// absence — no outcome of any kind for members two and three while the target is
/// busy — with delivery afterwards as the positive control that the absence is
/// suspension rather than loss.
#[test]
fn a_target_that_goes_busy_leaves_the_rest_queued_rather_than_resolving_them() {
    let mailbox = Arc::new(StubMailbox::with_entries(vec![
        entry(1, "busy-1"),
        entry(2, "busy-2"),
        entry(3, "busy-3"),
    ]));
    let stop = Arc::new(AtomicBool::new(false));
    let available = Arc::new(AtomicBool::new(true));
    let writes = Arc::new(AtomicUsize::new(0));

    let writer = BusyAfterOneTurn {
        stop: Arc::clone(&stop),
        available: Arc::clone(&available),
        writes: Arc::clone(&writes),
    };
    let context = DeliveryExecutorContext {
        consumer: Arc::clone(&mailbox) as Arc<_>,
        doorbell: DeliveryDoorbell::new(),
        poll_interval: POLL_INTERVAL,
        unreachable_dwell: Duration::from_secs(3_600),
    };
    let executor = std::thread::spawn(move || run_delivery_executor(writer, context));

    within_the_window(
        || !mailbox.acked().is_empty(),
        "the first envelope was never written",
    );

    // Let the executor take several further passes while the target is busy. Each
    // one is an opportunity to resolve the remaining members wrongly, and the
    // whole claim is that none of them does. Without this the test would assert
    // only that nothing had happened *yet*.
    let passes_before = writes.load(Ordering::SeqCst);
    std::thread::sleep(POLL_INTERVAL * 5);
    assert_eq!(
        writes.load(Ordering::SeqCst),
        passes_before,
        "a busy target is not written to, however many passes the executor makes"
    );

    let during_busy = mailbox.acked();
    assert_eq!(
        during_busy.len(),
        1,
        "only the envelope that was actually written has an outcome: {during_busy:?}"
    );
    assert_eq!(during_busy[0].message_id, "busy-1");

    // The regression itself. Members two and three carry no outcome at all, and
    // in particular not the `not_submitted` the push model produced for them.
    assert!(
        !during_busy
            .iter()
            .any(|member| member.evidence == SubmissionEvidence::NotSubmitted),
        "no member is reported not_submitted for a write that was never attempted: {during_busy:?}"
    );
    assert_eq!(
        mailbox.outstanding_range(),
        None,
        "and nothing behind the busy target is left declared, so no guard holds them"
    );

    // Ready again, and then ready again after that. The stub goes busy on every
    // write because an ACP worker publishes `Busy` on accepting every turn, so
    // becoming available once releases exactly one more envelope; what stands in
    // here for the turn-completion observer is a loop that re-arms it. The
    // absence above has to be suspension rather than loss, or it is a worse
    // defect than the one being fixed.
    let started = Instant::now();
    while mailbox.acked().len() < 3 {
        assert!(
            started.elapsed() < DELIVERY_WINDOW,
            "the queued envelopes were never delivered once the target was ready again"
        );
        available.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(2));
    }

    stop.store(true, Ordering::SeqCst);
    executor.join().expect("the executor thread ends");

    let delivered = mailbox.acked();
    assert_eq!(
        delivered
            .iter()
            .map(|member| member.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["busy-1", "busy-2", "busy-3"],
        "every envelope is delivered exactly once, in mailbox order"
    );
    assert!(
        delivered
            .iter()
            .all(|member| member.evidence == SubmissionEvidence::Submitted),
        "and each reports the write that actually carried it: {delivered:?}"
    );
}
