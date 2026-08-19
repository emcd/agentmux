//! The watch loop's wake policy: which batches reconcile, which are inert, and
//! that a silent channel reconciles anyway.
//!
//! Filesystem notification is best-effort, and one of its failure modes is
//! silent by construction. `notify`'s FSEvents backend renders a single
//! coalesced record as `[Create, Remove]` for one path, and
//! `notify-debouncer-full` cancels that pair — a file created and later removed
//! reads as a file that never existed, so the debouncer emits nothing at all.
//! The relay never learns the definition is gone. Reconciliation re-scans every
//! layer from disk, so the loss is entirely in the trigger, which is why the
//! sweep is the fix and why it is worth a test that no filesystem can stage: the
//! backend that drops the event is macOS-only, and the loop must reconcile
//! without one on every platform.
//!
//! These drive the loop through its own channel rather than a real debouncer.
//! The wake policy is the whole subject, and a fixture that had to provoke a
//! genuine dropped event could only do so on the platform where the drop
//! happens.

use agentmux::relay::{WatchWake, run_bundle_watch_loop};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent,
    notify::{
        Event, EventKind,
        event::{AccessKind, DataChange, ModifyKind},
    },
};
use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// A running loop plus the two ends the test drives it by. Dropping the event
/// sender disconnects the loop's channel, which is the only thing that ends it.
struct LoopHarness {
    events: Option<mpsc::Sender<DebounceEventResult>>,
    wakes: mpsc::Receiver<WatchWake>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LoopHarness {
    fn start(sweep_interval: Duration) -> Self {
        let (events, receiver) = mpsc::channel::<DebounceEventResult>();
        let (wake_sender, wakes) = mpsc::channel::<WatchWake>();
        let thread = thread::Builder::new()
            .name("watch-loop-under-test".to_string())
            .spawn(move || {
                run_bundle_watch_loop(&receiver, sweep_interval, |wake| {
                    // A test that has stopped listening is not a reason to
                    // panic inside the loop under test.
                    let _ = wake_sender.send(wake);
                })
            })
            .expect("spawn watch loop thread");
        Self {
            events: Some(events),
            wakes,
            thread: Some(thread),
        }
    }

    fn send(&self, batch: DebounceEventResult) {
        self.events
            .as_ref()
            .expect("event sender still held")
            .send(batch)
            .expect("watch loop still receiving");
    }

    /// Ends the loop and joins it, so a loop that fails to notice its channel
    /// closing fails the test rather than leaking a thread.
    fn stop(mut self) {
        self.events.take();
        self.thread
            .take()
            .expect("loop thread")
            .join()
            .expect("watch loop thread");
    }
}

fn access_batch() -> DebounceEventResult {
    batch(EventKind::Access(AccessKind::Read))
}

fn batch(kind: EventKind) -> DebounceEventResult {
    Ok(vec![DebouncedEvent::new(
        Event::new(kind).add_path(PathBuf::from("/nonexistent/bundles/alpha.toml")),
        Instant::now(),
    )])
}

// The failure this exists for reports nothing: the debouncer cancels a create
// against a later remove and emits no event at all, so a loop that only ever
// wakes on an event waits forever. Reconciliation is driven entirely by what is
// on disk, so waking on nothing is a complete substitute for the lost event.
#[test]
fn a_silent_channel_still_reconciles_on_the_sweep_interval() {
    let harness = LoopHarness::start(Duration::from_millis(150));

    // Generous against a loaded machine: the claim is that a wake arrives at
    // all without an event, not that it lands on a particular millisecond.
    let wake = harness
        .wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the sweep must reconcile without any filesystem event");
    assert_eq!(wake, WatchWake::Reconcile);

    harness.stop();
}

// Access (open/close/read) events cannot change bundle content, and the watcher
// generates them itself against the very files it watches: reconciliation reads
// every definition. Reconciling on them would re-trigger from its own reads,
// once per debounce interval, forever.
#[test]
fn an_access_only_batch_never_reconciles() {
    // Long enough that the sweep cannot be what the absence below is measuring.
    let harness = LoopHarness::start(Duration::from_secs(30));

    harness.send(access_batch());
    harness.send(access_batch());

    let wake = harness.wakes.recv_timeout(Duration::from_millis(500));
    assert!(
        wake.is_err(),
        "an access-only batch must not reconcile, got {wake:?}"
    );

    harness.stop();
}

// A batch carrying anything other than access is a reason to look, without the
// loop inspecting which file or which kind: the re-scan derives that from disk.
#[test]
fn a_batch_carrying_a_content_change_reconciles() {
    let harness = LoopHarness::start(Duration::from_secs(30));

    harness.send(batch(EventKind::Modify(ModifyKind::Data(DataChange::Any))));

    let wake = harness
        .wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("a content change must reconcile");
    assert_eq!(wake, WatchWake::Reconcile);

    harness.stop();
}

// The inert batches the watcher provokes from its own reads arrive continuously
// on a busy relay. Were an inert batch to restart the sweep timer, that stream
// would postpone the sweep indefinitely and the dropped-event backstop would
// never run on exactly the relay that needs it most.
#[test]
fn a_stream_of_access_batches_does_not_postpone_the_sweep() {
    let sweep = Duration::from_millis(200);
    let harness = LoopHarness::start(sweep);

    // Arrive well inside the sweep interval, so a timer that restarts per batch
    // would never expire.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut sweeps = 0;
    while Instant::now() < deadline {
        harness.send(access_batch());
        if harness
            .wakes
            .recv_timeout(Duration::from_millis(50))
            .is_ok()
        {
            sweeps += 1;
        }
    }

    // A lower bound: the interval implies about ten over two seconds, and the
    // distinction being drawn is against zero.
    assert!(
        sweeps >= 2,
        "the sweep must keep firing under a stream of inert batches, saw {sweeps}"
    );

    harness.stop();
}

// The loop owns no shutdown flag: dropping the debouncer closes the channel, and
// that is what has to end it. A loop that missed the disconnect would keep the
// relay's watcher thread alive past teardown.
#[test]
fn a_closed_channel_ends_the_loop() {
    let harness = LoopHarness::start(Duration::from_secs(30));
    // `stop` joins, so a loop that ignores the disconnect hangs here.
    harness.stop();
}
