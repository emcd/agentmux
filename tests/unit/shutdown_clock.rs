//! The shutdown deadline, and the nesting rule every bounded shutdown step obeys.
//!
//! Shutdown steps nest: the watchdog grace contains the cleanup wait, which
//! contains the per-worker drain, which contains the generation fence. The rule
//! is that an inner step bounds itself by what remains of the outer budget
//! rather than by a duration configured in isolation — because the two have no
//! relationship, and the innermost of them here is operator-configurable up to
//! twelve times the outermost.
//!
//! These drive the arithmetic directly. What they deliberately do NOT claim is
//! that a slow fence is now survivable end to end: forcing a fence to be slow
//! needs a generation that outlives forced termination, which no real transport
//! provides, so covering it waits on an injectable transport seam.

use std::time::Duration;

use agentmux::runtime::signals::{
    arm_shutdown_deadline, budget_within_shutdown, install_shutdown_signal_handlers,
    register_shutdown_grace, shutdown_requested, shutdown_time_remaining,
};

/// With nothing armed, a step takes the bound it was configured with.
///
/// This is the CLI and test-harness case, and it must not be confused with an
/// expired budget: there is no deadline to fit inside, so taking the full
/// configured bound violates nothing.
#[test]
fn an_unarmed_process_leaves_every_configured_bound_alone() {
    assert!(shutdown_time_remaining().is_none());
    let configured = Duration::from_millis(5_000);
    assert_eq!(
        budget_within_shutdown(configured, Duration::from_millis(300)),
        configured,
    );
}

/// The armed case: a configured bound larger than the grace is cut down to fit,
/// and the reserve is withheld for whatever runs after the step.
///
/// The real numbers are the point. `fence-observation-timeout-ms` defaults to
/// 5s against a 5s watchdog grace, so the default configuration alone is enough
/// for an inner step to consume the entire outer budget and leave nothing for
/// the work behind it.
#[test]
fn an_armed_deadline_cuts_a_configured_bound_down_and_holds_back_the_reserve() {
    arm_shutdown_deadline(Duration::from_millis(1_000));
    let remaining = shutdown_time_remaining().expect("a deadline was armed");
    assert!(
        remaining <= Duration::from_millis(1_000),
        "remaining must never exceed the grace it was armed with: {remaining:?}"
    );

    let reserve = Duration::from_millis(300);
    let fitted = budget_within_shutdown(Duration::from_millis(5_000), reserve);
    assert!(
        fitted < Duration::from_millis(1_000),
        "a 5s bound inside a 1s grace must be cut down, got {fitted:?}"
    );
    assert!(
        fitted + reserve <= remaining + Duration::from_millis(50),
        "the step plus its reserve must fit inside what remained: {fitted:?} + {reserve:?}"
    );

    // A bound already smaller than the grace is left alone: the deadline is a
    // ceiling, not a target, and stretching a short bound to fill the budget
    // would make every shutdown as slow as its worst case.
    let small = Duration::from_millis(10);
    assert_eq!(budget_within_shutdown(small, reserve), small);
}

/// The dangerous third state: shutdown requested, deadline not yet armed.
///
/// There are three states, not two. Unarmed-with-no-watchdog means take the
/// configured bound. Armed means fit inside it. Between them sits
/// `shutdown_requested() && deadline == None` — the window between the signal
/// handler setting the flag and the watchdog observing it a poll interval later
/// — and treating that like the first is how a step took its full configured
/// bound and overran the grace exactly as it had before any deadline existed.
/// Arming later does not revise a budget already snapshotted inside the window.
///
/// A registered grace is what makes the states distinguishable: it is the
/// positive fact that this process *will* have a deadline.
#[test]
fn a_shutdown_already_requested_arms_the_deadline_rather_than_reporting_none() {
    let _handlers = install_shutdown_signal_handlers().expect("install handlers");
    register_shutdown_grace(Duration::from_millis(400));

    // Raise the signal rather than simulating it, so the interleaving under test
    // is the real one: the handler sets the flag, and nothing has armed a
    // deadline because no watchdog is running in this test at all.
    assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0, "raise SIGINT");
    assert!(
        shutdown_requested(),
        "the handler must have observed the signal"
    );

    let fitted = budget_within_shutdown(Duration::from_secs(60), Duration::from_millis(100));
    assert!(
        fitted <= Duration::from_millis(400),
        "a budget taken before the watchdog arms must still fit the grace, got {fitted:?}"
    );
    assert!(
        shutdown_time_remaining().is_some(),
        "the deadline must exist once shutdown is underway and a grace is registered"
    );
}

/// Arming twice cannot extend the deadline.
///
/// The watchdog that will actually force the exit is already counting down from
/// the first arming, so a later, longer deadline would be a promise nothing
/// keeps — and a step that believed it would hand its successors a budget that
/// has already been spent.
#[test]
fn a_second_arming_cannot_extend_the_deadline() {
    let first = arm_shutdown_deadline(Duration::from_millis(500));
    let second = arm_shutdown_deadline(Duration::from_secs(60));
    assert_eq!(
        first, second,
        "the first arming owns the deadline; a later one must not move it"
    );
}

/// An exhausted budget yields zero, not the configured bound.
///
/// The distinction from the unarmed case is the whole point. Both could be
/// spelled "no constraint available", and if they were, a budget that had run
/// out would silently restore the full configured wait — reintroducing exactly
/// the overrun the deadline exists to prevent, at the moment there is least time
/// to absorb it.
#[test]
fn an_exhausted_budget_yields_zero_rather_than_the_configured_bound() {
    arm_shutdown_deadline(Duration::ZERO);
    std::thread::sleep(Duration::from_millis(5));

    assert_eq!(shutdown_time_remaining(), Some(Duration::ZERO));
    assert_eq!(
        budget_within_shutdown(Duration::from_secs(60), Duration::from_millis(300)),
        Duration::ZERO,
        "an expired grace must not hand back the configured bound"
    );
}
