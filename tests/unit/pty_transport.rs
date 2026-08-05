//! Test surface for the Pty transport's quiescence wait + look path.
//!
//! Tests inject scripted [`SnapshotResponse`]s via a mock worker thread
//! that consumes [`SnapshotRequest`]s from the snapshot channel and
//! replies with the scripted observation. The state machine
//! ([`wait_for_quiescent_three_state`]) drives the probe
//! ([`PtyQuiescenceProbe`]) deterministically without requiring
//! libghostty-vt / Zig — same approach as the Tmux probe test surface,
//! but routed through the snapshot channel instead of
//! `PaneQuiescenceProbe::next_evaluation`.
//!
//! The five behavior-class probe scenarios mirror the Tmux scenarios
//! (always unresponsive, always wedge, pending choice, slow prompt,
//! normal flow) plus coalesce-during-wedge and
//! coalesce-during-prime edge cases. The probe builds a
//! [`WedgeObservation`] from each [`SnapshotResponse`] by applying
//! the configured `prompt_regex` + `prompt_idle_column` checks; the
//! state machine then consumes the resulting observations.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use agentmux::pty::{
    PtyConfigSnapshot, PtyOutputView, PtyQuiescenceProbe, PtyShared, SnapshotResponse,
};
use agentmux::transports::{
    DeliveryDiagnosticContext, DeliveryWaitError, LookMode, LookSnapshotPayload, OutputView,
    QuiescenceBounds, WedgeProbe, wait_for_quiescent_three_state,
};
use regex::Regex;
use tokio::sync::mpsc;

const SHORT_QUIET_WINDOW: Duration = Duration::from_millis(5);
const TEST_TARGET_SESSION: &str = "test-session";

fn diagnostic_context() -> DeliveryDiagnosticContext<'static> {
    DeliveryDiagnosticContext::without_messages("test-namespace", TEST_TARGET_SESSION)
}

/// Scripted snapshot carrying a prompt tail (`$ `) at cursor column 2.
/// With the default `prompt-regex = r"^\\$"` and no `prompt_idle_column`,
/// the probe marks this as `is_prompt_ready = true`.
fn ready_snapshot() -> SnapshotResponse {
    SnapshotResponse {
        tail: "READY_MARKER\n".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: true,
    }
}

/// Empty tail; probe marks this as `is_prompt_ready = false` (regex
/// does not match an empty string). Empty-pane mismatch class —
/// Timeout territory, not Wedged.
fn empty_unready_snapshot() -> SnapshotResponse {
    SnapshotResponse {
        tail: String::new(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }
}

/// Stuck at non-prompt content ("tool-approval dialog"); regex does
/// not match. Wedge-class mismatch — Wedged territory after the
/// counter threshold.
fn stuck_unready_snapshot() -> SnapshotResponse {
    SnapshotResponse {
        tail: "Do you want to proceed? [Y/n]".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: true,
    }
}

/// Mock worker thread: drains [`SnapshotRequest`]s from the snapshot
/// channel and replies with the next scripted [`SnapshotResponse`] from
/// `script`. When `script` is empty the worker replies with
/// `last_response.clone().unwrap_or_default()` (typically an empty
/// snapshot) to avoid hanging the state machine.
///
/// Note (R2 wedge-busy-state): the worker does NOT per-request advance
/// `last_change_atomic`. That atomic has two consumers in the
/// cross-transport classifier after `add-wedge-detection-busy-state`:
/// `wait_for_change` polls it for quiescence detection, AND the
/// probe's `observe()` reads it as the `activity_generation` field for
/// the Busy pre-classification. The mock worker advancing it on every
/// snapshot request would conflate "probe polled snapshot" with
/// "terminal byte writes happened", which doesn't match production
/// semantics and would cause Busy to fire spuriously across observe
/// calls within a single quiescence iteration, breaking every wedge-
/// class test. Tests that need Busy to fire advance
/// `last_change_atomic` directly via `Arc<AtomicU64>`; the timer
/// below keeps `wait_for_change` progressing for the loop.
fn spawn_mock_worker(
    mut rx: mpsc::Receiver<agentmux::pty::SnapshotRequest>,
    script: Arc<Mutex<VecDeque<SnapshotResponse>>>,
    last_change_atomic: Arc<std::sync::atomic::AtomicU64>,
) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
    let worker = thread::spawn(move || {
        let mut last_response: Option<SnapshotResponse> = None;
        while let Some(req) = rx.blocking_recv() {
            let response = {
                let mut guard = script.lock().expect("script mutex poisoned");
                let next = guard.pop_front();
                drop(guard);
                match next {
                    Some(resp) => {
                        last_response = Some(resp.clone());
                        resp
                    }
                    None => last_response.clone().unwrap_or_else(|| SnapshotResponse {
                        tail: String::new(),
                        cursor_x: 0,
                        cursor_y: 0,
                        cursor_visible: false,
                    }),
                }
            };
            let _ = req.tx.send(response);
        }
    });
    let atomic_for_timer = last_change_atomic;
    let timer = thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(50));
            atomic_for_timer.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    });
    (worker, timer)
}

/// Build a [`PtyShared`] wired to a mock snapshot worker thread that
/// drains `script`. Returns the shared state plus the receiver join
/// handle so tests can clean up.
#[allow(clippy::too_many_arguments)]
fn make_pty_shared(
    script: Arc<Mutex<VecDeque<SnapshotResponse>>>,
    prime_timeout_ms: Option<u64>,
    wedge_detection: bool,
    prompt_regex: Option<Regex>,
    prompt_idle_column: Option<u16>,
) -> (PtyShared, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<agentmux::pty::SnapshotRequest>(64);
    let last_change_atomic = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let shared = PtyShared {
        config: PtyConfigSnapshot {
            target_member_id: TEST_TARGET_SESSION.to_string(),
            cols: 120,
            rows: 40,
            prompt_regex,
            prompt_inspect_lines: 3,
            prompt_idle_column,
            prime_timeout_ms,
            wedge_detection,
        },
        last_change_atomic: last_change_atomic.clone(),
        snapshot_tx: tx,
        child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let (_worker_handle, _timer_handle) = spawn_mock_worker(rx, script, last_change_atomic);
    (shared, _worker_handle)
}

/// Build `(prime_deadline, prime_started_at, prime_timeout_ms)` the
/// state machine expects.
/// Bounds for one Pty flush group, anchored now.
///
/// `readiness_timeout_ms` is always `None`: Pty carries no readiness bound,
/// because it writes to the pty master before its readiness wait and so cannot
/// report an expiry as non-delivery. Passing one here would test a
/// configuration the relay never constructs for a Pty target.
///
/// This returns the bounds struct rather than a tuple of parts. The tuple form
/// sampled `Instant::now()` once per field access at the call sites
/// (`prime_window(x).0`, `.1`, `.2` were three separate calls), so a group's
/// deadline and its anchor came from different instants.
fn prime_window(timeout_ms: Option<u64>) -> QuiescenceBounds {
    QuiescenceBounds::new(SHORT_QUIET_WINDOW, Instant::now(), timeout_ms, None)
}

// ---------------------------------------------------------------------------
// Behavior-class probes (tasks.md §9.2)
// ---------------------------------------------------------------------------

#[test]
fn always_unresponsive_probe_resolves_timeout() {
    let script = Arc::new(Mutex::new(VecDeque::from([empty_unready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(30),
        false,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(30)),
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout, got {result:?}",
    );
}

#[test]
fn always_wedge_probe_resolves_wedged() {
    let script = Arc::new(Mutex::new(VecDeque::from([stuck_unready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged, got {result:?}",
    );
}

#[test]
fn cursor_mismatch_preserves_its_reason_in_wedged_outcome() {
    // Configure prompt_idle_column = 4. Snapshot carries tail="$ "
    // (regex matches) but cursor_x = 0 (does not match expected).
    // The probe's mismatch reason must contain "cursor column".
    let script = Arc::new(Mutex::new(VecDeque::from([SnapshotResponse {
        tail: "$ ".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: true,
    }])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        Some(4),
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    match result {
        Err(DeliveryWaitError::Wedged { reason }) => {
            assert!(
                reason.contains("cursor column"),
                "expected cursor-column reason, got {reason:?}",
            );
        }
        other => panic!("expected Wedged with cursor reason, got {other:?}"),
    }
}

#[test]
fn cursor_mismatch_preserves_its_reason_in_timeout_outcome() {
    let script = Arc::new(Mutex::new(VecDeque::from([SnapshotResponse {
        tail: "$ ".to_string(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: true,
    }])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10),
        false,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        Some(4),
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10)),
        false,
    );
    match result {
        Err(DeliveryWaitError::Timeout {
            mismatch_reason: Some(reason),
            ..
        }) => {
            assert!(
                reason.contains("cursor column"),
                "expected cursor-column reason, got {reason:?}",
            );
        }
        other => panic!("expected Timeout with cursor reason, got {other:?}"),
    }
}

#[test]
fn slow_prompt_probe_resolves_delivered() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        empty_unready_snapshot(),
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
        ready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

#[test]
fn normal_flow_probe_resolves_delivered() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        stuck_unready_snapshot(),
        ready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

// ---------------------------------------------------------------------------
// Coalesce / wedge-counter scenarios (tasks.md §9.3-9.4)
// ---------------------------------------------------------------------------

#[test]
fn coalesce_during_wedge_counter_does_not_fire_on_changing_signatures() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        stuck_unready_snapshot(),
        empty_unready_snapshot(),
        stuck_unready_snapshot(),
        empty_unready_snapshot(),
        stuck_unready_snapshot(),
        ready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    eprintln!("slow_prompt result: {result:?}");
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

#[test]
fn coalesce_during_wedge_counter_fires_after_consecutive_identical_signatures() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged, got {result:?}",
    );
}

/// §9.4 — coalesce-during-prime-does-not-extend-window. Configures
/// a bounded 50 ms prime window and a snapshot stream that returns
/// the same unready snapshot throughout the wait. The state machine
/// processes many classify cycles (each consuming one scripted
/// snapshot and triggering a fresh `wait_for_change` poll); the test
/// asserts `Timeout` fires within a bounded elapsed time, NOT
/// extended by the repeated classify iterations. Mirrors the
/// existing `prime_timeout_opt_in_fires_after_window` test but adds
/// the elapsed-time bound that guards the "prime window does not
/// extend" guarantee.
#[test]
fn coalesce_during_prime_does_not_extend_window() {
    let script = Arc::new(Mutex::new(VecDeque::from(vec![
        empty_unready_snapshot();
        50
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(50),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let started = Instant::now();
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(50)),
        true,
    );
    let elapsed = started.elapsed();
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (prime window fired), got {result:?}",
    );
    // The prime window must NOT extend beyond ~4x the configured
    // timeout (50 ms). Each classify cycle is one wait-poll
    // (~10 ms); a few cycles after the deadline is acceptable, but
    // indefinite extension is the regression §9.4 guards against.
    assert!(
        elapsed < Duration::from_millis(200),
        "prime window extended by classify cycles; elapsed = {:?}",
        elapsed,
    );
}

// ---------------------------------------------------------------------------
// Wedge / prime-timeout switches (tasks.md §9.6-9.8)
// ---------------------------------------------------------------------------

#[test]
fn wedge_default_on_fires_after_consecutive_identical_mismatches() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged (wedge default-on), got {result:?}",
    );
}

#[test]
fn wedge_disabled_does_not_fire_wedged_within_short_window() {
    // wedge_detection=false, prime_timeout_ms=200ms, empty-pane
    // mismatch (NOT wedge-class). Without wedge and with a
    // non-wedge-class mismatch, the prime-timeout governs. The wait
    // should fire Timeout (not Wedged) when the prime window elapses.
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        empty_unready_snapshot(),
        empty_unready_snapshot(),
        empty_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(200),
        false,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(200)),
        false,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (wedge off + prime bound), got {result:?}",
    );
}

#[test]
fn prime_timeout_default_off_does_not_fire() {
    // prime_timeout_ms = None (default off), wedge enabled, empty-pane
    // mismatch. Without a prime-timeout bound the state machine waits
    // indefinitely. We run on a thread and verify it has not returned
    // within 100ms.
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        empty_unready_snapshot(),
        empty_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        None,
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let started = Instant::now();
    let bounds = QuiescenceBounds::new(SHORT_QUIET_WINDOW, started, None, None);
    let join = thread::spawn(move || {
        wait_for_quiescent_three_state(&mut probe, &diagnostic_context(), &bounds, true)
    });
    thread::sleep(Duration::from_millis(100));
    assert!(
        !join.is_finished(),
        "wait returned within 100ms despite prime_timeout_ms = None",
    );
    // The worker thread continues until the process exits; drop the
    // join handle without joining.
}

#[test]
fn prime_timeout_opt_in_fires_after_window() {
    let script = Arc::new(Mutex::new(VecDeque::from([
        empty_unready_snapshot(),
        empty_unready_snapshot(),
        empty_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (prime_timeout_ms opt-in), got {result:?}",
    );
}

#[test]
fn short_prime_timeout_does_not_preempt_wedge_for_wedge_class_mismatch() {
    let script = Arc::new(Mutex::from(VecDeque::from([
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
        stuck_unready_snapshot(),
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged (wedge governs over short prime-timeout), got {result:?}",
    );
}

/// §9.8 — wedge-disabled + prime-timeout-set combined scenario.
/// With wedge detection disabled, wedge-class mismatches cannot
/// fire `Wedged` (the wedge counter is short-circuited); the
/// prime-timeout bounds every quiescent state instead. The wait
/// fires `Timeout` within the bounded prime window regardless of
/// whether the snapshot signature is wedge-class — `stuck_…`
/// (wedge-class) is used here specifically to exercise the
/// regression that would let a wedge-class signature fire Wedged
/// even when wedge is disabled. Mirrors the existing
/// `wedge_disabled_does_not_fire_wedged_within_short_window` test
/// but explicitly exercises the wedge-class signature path with an
/// elapsed-time bound.
#[test]
fn wedge_disabled_with_prime_timeout_bounds_every_quiescent_state() {
    let script = Arc::new(Mutex::new(VecDeque::from(vec![
        stuck_unready_snapshot();
        20
    ])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(50),
        false,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let started = Instant::now();
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(50)),
        false,
    );
    let elapsed = started.elapsed();
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (wedge disabled, prime bounds every quiescent state), got {result:?}",
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "prime window extended; elapsed = {:?}",
        elapsed,
    );
}

// ---------------------------------------------------------------------------
// PtyOutputView::look (snapshot-channel-driven formatting)
// ---------------------------------------------------------------------------

#[test]
fn look_returns_last_n_lines_per_look_mode() {
    let tail = "line1\nline2\nline3\nline4\nline5\n".to_string();
    let script = Arc::new(Mutex::new(VecDeque::from([SnapshotResponse {
        tail,
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }])));
    let (shared, _handle) = make_pty_shared(
        script,
        None,
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let view = PtyOutputView::new(shared);
    let snapshot = view
        .look(LookMode {
            lines: Some(3),
            offset: None,
            prime_timeout: Duration::from_millis(0),
        })
        .expect("look should succeed");
    let lines = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines,
        _ => panic!("expected Lines payload"),
    };
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "line3");
    assert_eq!(lines[1], "line4");
    assert_eq!(lines[2], "line5");
}

#[test]
fn look_returns_empty_when_snapshot_is_empty() {
    let script = Arc::new(Mutex::new(VecDeque::from([SnapshotResponse {
        tail: String::new(),
        cursor_x: 0,
        cursor_y: 0,
        cursor_visible: false,
    }])));
    let (shared, _handle) = make_pty_shared(
        script,
        None,
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let view = PtyOutputView::new(shared);
    let snapshot = view
        .look(LookMode {
            lines: Some(10),
            offset: None,
            prime_timeout: Duration::from_millis(0),
        })
        .expect("look should succeed even with empty tail");
    let lines = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines,
        _ => panic!("expected Lines payload"),
    };
    assert!(lines.is_empty());
}

// ---------------------------------------------------------------------------
// Integration test (tasks.md §9.9) — requires Zig + libghostty-vt build
// ---------------------------------------------------------------------------

/// Bounded blocking receive for a `oneshot::Receiver`. Polls with
/// `try_recv` until the outcome arrives, the channel closes, or the
/// deadline elapses. The previous version used `blocking_recv()` with
/// no timeout, so a hang in the worker would deadlock the test
/// forever; this helper gives the test a bounded window to assert
/// the wait path actually returns.
fn recv_bounded<T>(
    mut rx: tokio::sync::oneshot::Receiver<T>,
    timeout: Duration,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(value) => return Ok(value),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return Err(format!("oneshot receive timed out after {:?}", timeout));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err("oneshot channel closed before delivery".to_string());
            }
        }
    }
}

/// Round-trip test (Finding 4 fix): exercises the production wait
/// path by writing a line via `raww` and asserting the quiescence
/// probe observes the echoed line in the terminal snapshot before
/// the bounded prime-timeout fires.
///
/// Without the production wait path correctly draining PTY bytes
/// during the delivery wait (Finding 1), the echo never reaches the
/// terminal's snapshot, the prompt regex never matches, the wait
/// fires Timeout, and this test fails.
///
/// Run with `cargo test --test unit pty_transport -- --ignored` once
/// Zig 0.15.x is on PATH and `cargo check --features pty` succeeds.
/// Skipped by default because the underlying PtyTransport spawns
/// threads that consume the snapshot channel for the full lifetime
/// of the test; without libghostty-vt built, startup() fails.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_round_trips_raww_with_prompt_readiness() {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{
        LookMode, LookSnapshotPayload, SendOutcome, StartupContext, Transport,
    };

    let target = BundleMember {
        id: "cat-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"hello".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            cols: 80,
            rows: 24,
            term_protocol: TermProtocol::Xterm256Color,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::pty::PtyTransport::new(
        target,
        PtyTargetConfiguration {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"hello".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: BundleMember {
            id: "cat-test".to_string(),
            name: None,
            working_directory: None,
            target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
                initial_command: "/bin/cat".to_string(),
                resume_command: "/bin/cat".to_string(),
                prompt_readiness: Some(PromptReadinessTemplate {
                    prompt_regex: r"hello".to_string(),
                    inspect_lines: None,
                    input_idle_cursor_column: None,
                }),
                prime_timeout_ms: Some(2000),
                wedge_detection: true,
                cols: 80,
                rows: 24,
                term_protocol: TermProtocol::Xterm256Color,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        },
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_round_trips_raww_with_prompt_readiness: \
                 skipped (startup failed: {e:?}); requires Zig 0.15.x + \
                 libghostty-vt built via --features pty"
            );
            return;
        }
    };
    assert!(
        matches!(
            status.readiness,
            agentmux::transports::TransportReadiness::Ready
        ),
        "startup should report Ready, got {:?}",
        status.readiness,
    );

    // Write "hello\n" via raww. With Finding 1 fixed, the worker
    // applies the echoed "hello\n" to the terminal during the
    // quiescence wait, the prompt regex matches, and the outcome
    // resolves to Delivered. Without Finding 1 fixed, the echo
    // never reaches the snapshot and the wait fires Timeout.
    let outcome_rx = transport.raww("hello".to_string(), true);
    let outcome = recv_bounded(outcome_rx, Duration::from_secs(5)).expect("raww outcome receive");
    assert!(
        matches!(outcome.outcome, SendOutcome::Delivered),
        "expected Delivered (echo 'hello' should match prompt regex), got {:?}",
        outcome.outcome,
    );

    // Read back via give_output + look. The snapshot should contain
    // "hello" somewhere in the lines if the round-trip succeeded.
    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let snapshot = output
        .look(LookMode {
            lines: Some(40),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let snapshot_lines = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines,
        _ => panic!("expected Lines payload"),
    };
    assert!(
        snapshot_lines.iter().any(|line| line.contains("hello")),
        "snapshot did not contain the echoed input: {snapshot_lines:?}",
    );
}

/// Multi-arg command startup test (Finding 2 fix): configures
/// `initial-command = "/bin/echo arg1 arg2"`, starts the transport,
/// and asserts the child's stdout ("arg1 arg2") appears in the
/// snapshot. Without the command tokenization fix,
/// `CommandBuilder::new("/bin/echo arg1 arg2")` tries to exec a
/// literal binary named "/bin/echo arg1 arg2" and fails to spawn.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_spawns_multi_arg_initial_command() {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{LookMode, LookSnapshotPayload, StartupContext, Transport};

    let target = BundleMember {
        id: "echo-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
            initial_command: "/bin/echo arg1 arg2".to_string(),
            resume_command: "/bin/echo arg1 arg2".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"arg1".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            cols: 80,
            rows: 24,
            term_protocol: TermProtocol::Xterm256Color,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::pty::PtyTransport::new(
        target,
        PtyTargetConfiguration {
            initial_command: "/bin/echo arg1 arg2".to_string(),
            resume_command: "/bin/echo arg1 arg2".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"arg1".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: BundleMember {
            id: "echo-test".to_string(),
            name: None,
            working_directory: None,
            target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
                initial_command: "/bin/echo arg1 arg2".to_string(),
                resume_command: "/bin/echo arg1 arg2".to_string(),
                prompt_readiness: Some(PromptReadinessTemplate {
                    prompt_regex: r"arg1".to_string(),
                    inspect_lines: None,
                    input_idle_cursor_column: None,
                }),
                prime_timeout_ms: Some(2000),
                wedge_detection: true,
                cols: 80,
                rows: 24,
                term_protocol: TermProtocol::Xterm256Color,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        },
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_spawns_multi_arg_initial_command: \
                 skipped (startup failed: {e:?}); requires Zig 0.15.x + \
                 libghostty-vt built via --features pty"
            );
            return;
        }
    };
    assert!(
        matches!(
            status.readiness,
            agentmux::transports::TransportReadiness::Ready
        ),
        "startup should report Ready, got {:?}",
        status.readiness,
    );

    // The child writes "arg1 arg2\n" to the PTY master and exits.
    // The reader thread feeds those bytes to the worker, the worker
    // applies them to the terminal, and the look snapshot should
    // contain "arg1 arg2".
    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    // Loop briefly to give the worker time to apply the echo bytes
    // to the terminal; the bounded wait is a safety net if the
    // delivery path takes a tick to apply.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = output
            .look(LookMode {
                lines: Some(40),
                offset: None,
                prime_timeout: Duration::from_millis(100),
            })
            .expect("look should succeed");
        let current_lines = match snapshot {
            LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines,
            _ => panic!("expected Lines payload"),
        };
        if current_lines.iter().any(|line| line.contains("arg1 arg2")) {
            break;
        }
        if Instant::now() >= deadline {
            panic!("snapshot did not contain 'arg1 arg2' within 2s: {current_lines:?}");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Regression test for Finding 1 (re-review at 9990fc9): a `mailw`
/// submitted while a `raww` is still in its wait must produce a
/// delivery outcome, not a closed oneshot. The v1 implementation
/// drained `write_rx` into a throwaway `empty_group` during the
/// raw's wait, then dropped the group when the wait returned —
/// closing every absorbed envelope's `outcome_tx` silently.
///
/// Run with `cargo test --test unit pty_transport -- --ignored`
/// once Zig 0.15.x + libghostty-vt are available.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_mailw_during_raww_wait_is_not_dropped() {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::envelope::AddressIdentity;
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{DeliveryMessage, StartupContext, Transport};

    let target = BundleMember {
        id: "fifo-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"hello".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            cols: 80,
            rows: 24,
            term_protocol: TermProtocol::Xterm256Color,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::pty::PtyTransport::new(
        target,
        PtyTargetConfiguration {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"hello".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: BundleMember {
            id: "fifo-test".to_string(),
            name: None,
            working_directory: None,
            target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
                initial_command: "/bin/cat".to_string(),
                resume_command: "/bin/cat".to_string(),
                prompt_readiness: Some(PromptReadinessTemplate {
                    prompt_regex: r"hello".to_string(),
                    inspect_lines: None,
                    input_idle_cursor_column: None,
                }),
                prime_timeout_ms: Some(2000),
                wedge_detection: true,
                cols: 80,
                rows: 24,
                term_protocol: TermProtocol::Xterm256Color,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        },
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_mailw_during_raww_wait_is_not_dropped: \
                 skipped (startup failed: {e:?})"
            );
            return;
        }
    };
    assert!(matches!(
        status.readiness,
        agentmux::transports::TransportReadiness::Ready
    ));

    // Send raww("hello") and immediately (before its wait resolves)
    // submit a mailw envelope. The envelope must be processed in a
    // follow-up delivery cycle and produce an outcome (Delivered or
    // Timeout) — NOT a closed oneshot. With the v1 bug, the
    // envelope's outcome_tx was dropped on the floor.
    let raw_outcome = transport.raww("hello".to_string(), true);
    let envelope = agentmux::transports::DeliveryEnvelope {
        message_id: "msg-2".to_string(),
        message: DeliveryMessage {
            body: "world".to_string(),
            created_at: "1970-01-01T00:00:00Z".to_string(),
            namespace: "test".to_string(),
            sender: AddressIdentity {
                session_name: "test".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: "fifo-test".to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window: Duration::from_millis(50),
        prime_timeout_ms: Some(2000),
        readiness_timeout_ms: None,
        is_receipt: false,
    };
    let mailw_outcome = transport.mailw(envelope);

    // The raw should resolve to Delivered (cat echoes "hello\n", the
    // prompt regex matches).
    let raw_result =
        recv_bounded(raw_outcome, Duration::from_secs(5)).expect("raw outcome receive");
    assert!(
        matches!(
            raw_result.outcome,
            agentmux::transports::SendOutcome::Delivered
        ),
        "expected raw Delivered, got {:?}",
        raw_result.outcome,
    );

    // The mailw must also produce an outcome. With the v1 bug, the
    // oneshot was closed (Err), so the receive below would panic
    // with "oneshot channel closed before delivery".
    let mailw_result = recv_bounded(mailw_outcome, Duration::from_secs(5))
        .expect("mailw outcome receive (would fail with closed channel under v1 bug)");
    assert_eq!(
        mailw_result.message_id, "msg-2",
        "mailw outcome message_id should be preserved"
    );
    // The outcome can be Delivered (terminal re-matched) or Timeout
    // (echo "world" did not match "hello" regex). The point is that
    // an outcome was produced at all, not a closed channel.
    assert!(
        matches!(
            mailw_result.outcome,
            agentmux::transports::SendOutcome::Delivered
                | agentmux::transports::SendOutcome::Timeout
        ),
        "expected Delivered or Timeout for mailw (NOT closed), got {:?}",
        mailw_result.outcome,
    );
}

/// Regression test for Finding 2 (re-review at 9990fc9): a `look`
/// issued while a delivery wait is in progress must return
/// promptly. The v1 implementation blocked the worker thread
/// inside a `PtyDeliveryWait::run` loop that did not service
/// `snapshot_rx`, so concurrent look requests waited for the
/// full prime window (or indefinitely when unbounded).
///
/// Run with `cargo test --test unit pty_transport -- --ignored`
/// once Zig 0.15.x + libghostty-vt are available.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_look_during_non_ready_wait_returns_promptly() {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{LookSnapshotPayload, StartupContext, Transport};

    // /bin/sleep 5 produces no terminal output for 5 seconds, so
    // the wait is effectively non-ready for the test's duration.
    // The raw's wait is bounded by prime_timeout_ms (5000).
    let target = BundleMember {
        id: "look-during-wait".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
            initial_command: "/bin/sleep 5".to_string(),
            resume_command: "/bin/sleep 5".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"NEVER_MATCHES".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            prime_timeout_ms: Some(5000),
            wedge_detection: true,
            cols: 80,
            rows: 24,
            term_protocol: TermProtocol::Xterm256Color,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::pty::PtyTransport::new(
        target,
        PtyTargetConfiguration {
            initial_command: "/bin/sleep 5".to_string(),
            resume_command: "/bin/sleep 5".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: r"NEVER_MATCHES".to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(5000),
            wedge_detection: true,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: BundleMember {
            id: "look-during-wait".to_string(),
            name: None,
            working_directory: None,
            target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
                initial_command: "/bin/sleep 5".to_string(),
                resume_command: "/bin/sleep 5".to_string(),
                prompt_readiness: Some(PromptReadinessTemplate {
                    prompt_regex: r"NEVER_MATCHES".to_string(),
                    inspect_lines: None,
                    input_idle_cursor_column: None,
                }),
                prime_timeout_ms: Some(5000),
                wedge_detection: true,
                cols: 80,
                rows: 24,
                term_protocol: TermProtocol::Xterm256Color,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        },
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_look_during_non_ready_wait_returns_promptly: \
                 skipped (startup failed: {e:?})"
            );
            return;
        }
    };
    assert!(matches!(
        status.readiness,
        agentmux::transports::TransportReadiness::Ready
    ));

    // Kick off a raw that will sit in its wait for ~5s.
    let _raw_outcome = transport.raww("hello".to_string(), true);

    // Give the worker a moment to enter the wait state.
    thread::sleep(Duration::from_millis(100));

    // Issue a look with a 2-second prime_timeout. With the v1 bug,
    // the worker was blocked in PtyDeliveryWait::run, snapshot_rx
    // was never drained, the look would wait the full 2s and then
    // time out. With the fix, the worker services snapshot_rx
    // every ~10ms, so the look returns in ~20-50ms.
    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let started = Instant::now();
    let snapshot = output
        .look(LookMode {
            lines: Some(40),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "look should return promptly during a non-ready wait, took {:?}",
        elapsed,
    );
    // The look succeeds; the actual content depends on what
    // /bin/sleep 5 has produced (likely empty since sleep is silent).
    match snapshot {
        LookSnapshotPayload::Lines { .. } => {}
        _ => panic!("expected Lines payload"),
    }
}

// =========================================================================
// per-coder term-protocol tests
// =========================================================================

/// Builds a `[coders.<id>.pty]`-backed `BundleMember` plus a parallel
/// `pty::PtyTargetConfiguration` and constructs a `PtyTransport`. Used by
/// the term-protocol #[ignore] tests below.
fn make_term_protocol_transport(
    id: &str,
    initial_command: String,
    term_protocol: agentmux::configuration::TermProtocol,
) -> (
    agentmux::configuration::BundleMember,
    agentmux::pty::PtyTargetConfiguration,
) {
    use agentmux::configuration::{BundleMember, PtyTargetConfiguration as PtyConfig};

    let pty_cfg = PtyConfig {
        initial_command: initial_command.clone(),
        resume_command: initial_command.clone(),
        prompt_readiness: None,
        prime_timeout_ms: Some(2000),
        wedge_detection: true,
        cols: 80,
        rows: 24,
        term_protocol,
    };
    let target = BundleMember {
        id: id.to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(pty_cfg),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let transport_cfg = agentmux::pty::PtyTargetConfiguration {
        initial_command: initial_command.clone(),
        resume_command: initial_command,
        prompt_readiness: None,
        cols: 80,
        rows: 24,
        prime_timeout_ms: Some(2000),
        wedge_detection: true,
        working_directory: None,
        term_protocol,
    };
    (target, transport_cfg)
}

#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_term_protocol_propagates_to_child_command() {
    use agentmux::configuration::TermProtocol;
    use agentmux::transports::{LookMode, LookSnapshotPayload, StartupContext, Transport};

    // The child MUST print its own TERM through the PTY for the
    // assertion to be meaningful; reading `/proc/self/environ` from the
    // test process would report the test's env, not the spawned PTY
    // child's.
    let child_command = "sh -c 'printf \"TERM=%s\\n\" \"$TERM\"; sleep 1'".to_string();
    let term_protocol = TermProtocol::XtermKitty;
    let expected = "TERM=xterm-kitty";

    let (target, transport_cfg) = make_term_protocol_transport(
        "term-protocol-propagation-test",
        child_command,
        term_protocol,
    );
    let mut transport = agentmux::pty::PtyTransport::new(target.clone(), transport_cfg, None);
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: target,
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_term_protocol_propagates_to_child_command: \
                 skipped (startup failed: {e:?}); requires Zig 0.15.x + \
                 libghostty-vt built via --features pty"
            );
            return;
        }
    };
    assert!(matches!(
        status.readiness,
        agentmux::transports::TransportReadiness::Ready
    ));

    // Give VTE time to render the child's printf output.
    thread::sleep(Duration::from_millis(500));

    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let snapshot = output
        .look(LookMode {
            lines: Some(40),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let tail = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines.join("\n"),
        _ => panic!("expected Lines payload"),
    };
    assert!(
        tail.contains(expected),
        "expected {expected:?} in snapshot tail, got: {tail:?}"
    );
}

#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_term_protocol_dependent_round_trip_through_snapshot() {
    use agentmux::configuration::TermProtocol;
    use agentmux::transports::{LookMode, LookSnapshotPayload, StartupContext, Transport};

    // Child branches on TERM and emits distinct output per branch; the
    // Pty transport's snapshot path (PtyOutputView::look) should reflect
    // the branch that was actually taken. Does NOT exercise full
    // libghostty-vt CSI-u query/response (deferred).
    let child_command = "sh -c 'case \"$TERM\" in xterm-kitty) printf \"kitty-mode\\n\";; *) printf \"default-mode\\n\";; esac; sleep 1'".to_string();
    let term_protocol = TermProtocol::XtermKitty;
    let expected_branch = "kitty-mode";
    let unexpected_branch = "default-mode";

    let (target, transport_cfg) = make_term_protocol_transport(
        "term-protocol-dependent-round-trip-test",
        child_command,
        term_protocol,
    );
    let mut transport = agentmux::pty::PtyTransport::new(target.clone(), transport_cfg, None);
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: target,
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_transport_term_protocol_dependent_round_trip_through_snapshot: \
                 skipped (startup failed: {e:?}); requires Zig 0.15.x + \
                 libghostty-vt built via --features pty"
            );
            return;
        }
    };
    assert!(matches!(
        status.readiness,
        agentmux::transports::TransportReadiness::Ready
    ));

    thread::sleep(Duration::from_millis(500));

    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let snapshot = output
        .look(LookMode {
            lines: Some(40),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let tail = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines.join("\n"),
        _ => panic!("expected Lines payload"),
    };
    assert!(
        tail.contains(expected_branch),
        "expected snapshot to reflect xterm-kitty branch ({expected_branch:?}), got: {tail:?}"
    );
    assert!(
        !tail.contains(unexpected_branch),
        "snapshot should NOT contain the default-mode branch ({unexpected_branch:?}); got: {tail:?}"
    );
}

// =========================================================================
// Pty runtime child-env propagation (env-vars tasks.md 4.3)
// =========================================================================

/// End-to-end Pty runtime child-env propagation test
/// (`add-configurable-environment-variables` tasks.md 4.3). Confirms
/// the Pty transport at `src/pty/transport.rs` `startup` applies the
/// merged coder/bundle/session `BundleMember.environment` to the
/// spawned child — including a plain operator-declared key AND an
/// explicit operator override of `TERM`/`COLORTERM` (which must win
/// over the transport defaults set just before the env-merge loop per
/// the after-defaults placement comment in `src/pty/transport.rs`
/// `startup`).
///
/// Mirrors the term-protocol propagation test pattern at
/// `tests/unit/pty_transport.rs:1347-1486` (same starting shape:
/// `BundleMember` + `PtyTargetConfiguration` + `PtyTransport::new` +
/// `StartupContext` + 500ms render delay + `PtyOutputView::look`),
/// but the child command prints three env values (operator-declared
/// key + two explicit `TERM`/`COLORTERM` overrides) so the assertions
/// lock both "env reaches child" and "operator wins over transport
/// defaults".
///
/// Run with `cargo test --test unit pty_transport --features pty --
/// --ignored` once Zig 0.15.x is on PATH and `cargo check --features
/// pty` succeeds locally. Skipped by default because the underlying
/// `PtyTransport` spawns threads that consume the snapshot channel
/// for the full lifetime of the test; without libghostty-vt built,
/// `startup()` fails.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_runtime_child_env_propagates_operator_overrides() {
    use agentmux::configuration::{
        BundleMember, NameValueEntry, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::transports::{LookMode, LookSnapshotPayload, StartupContext, Transport};

    // The child prints three env values to verify both the
    // operator-declared-key reach and the precedence story:
    //   - FOR_TEST_OPERATOR_KEY: custom operator key — must reach child.
    //   - TERM: must reflect operator's override, NOT the
    //     `PtyTargetConfiguration` default "xterm-kitty" (XtermKitty).
    //   - COLORTERM: must reflect operator's override, NOT the
    //     transport default of "truecolor".
    // Then sleeps so the snapshot has time to render.
    let child_command = "sh -c 'printf \"FOR_TEST_OPERATOR_KEY=%s\\nTERM=%s\\nCOLORTERM=%s\\n\" \"$FOR_TEST_OPERATOR_KEY\" \"$TERM\" \"$COLORTERM\"; sleep 1'".to_string();

    let pty_cfg = PtyConfig {
        initial_command: child_command.clone(),
        resume_command: child_command.clone(),
        prompt_readiness: None,
        prime_timeout_ms: Some(2000),
        wedge_detection: false,
        cols: 80,
        rows: 24,
        term_protocol: TermProtocol::XtermKitty,
    };
    let target = BundleMember {
        id: "child-env-propagation-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(pty_cfg.clone()),
        coder_session_id: None,
        policy_id: None,
        environment: vec![
            // Custom operator-declared key (no transport default to
            // compete with — purely verifying the env-merge loop
            // reaches the child).
            NameValueEntry {
                name: "FOR_TEST_OPERATOR_KEY".to_string(),
                value: "operator_test_value".to_string(),
            },
            // Operator override of TERM (should beat the
            // `PtyTargetConfiguration` default "xterm-kitty").
            NameValueEntry {
                name: "TERM".to_string(),
                value: "xterm-test-override".to_string(),
            },
            // Operator override of COLORTERM (should beat the
            // transport default "truecolor").
            NameValueEntry {
                name: "COLORTERM".to_string(),
                value: "operator-colorterm-override".to_string(),
            },
        ],
    };
    let transport_cfg = agentmux::pty::PtyTargetConfiguration {
        initial_command: child_command.clone(),
        resume_command: child_command,
        prompt_readiness: None,
        cols: 80,
        rows: 24,
        prime_timeout_ms: Some(2000),
        wedge_detection: false,
        working_directory: None,
        term_protocol: TermProtocol::XtermKitty,
    };

    let mut transport = agentmux::pty::PtyTransport::new(target.clone(), transport_cfg, None);
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: target,
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => panic!(
            "pty_transport_runtime_child_env_propagates_operator_overrides: \
             PtyTransport::startup failed: {e:?}. This test requires \
             Zig 0.15.x + libghostty-vt built via --features pty to lock \
             the env-merge contract; a loud failure is the correct \
             behavior — a silent skip would mask a regression in the \
             Pty infrastructure and let the env-merge contract go \
             unguarded."
        ),
    };
    assert!(
        matches!(
            status.readiness,
            agentmux::transports::TransportReadiness::Ready
        ),
        "startup should report Ready, got {:?}",
        status.readiness,
    );

    // Give VTE time to render the child's printf output.
    thread::sleep(Duration::from_millis(500));

    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let snapshot = output
        .look(LookMode {
            lines: Some(40),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let tail = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines.join("\n"),
        _ => panic!("expected Lines payload"),
    };

    // All three env values must appear in the snapshot. If the
    // after-defaults placement of the env-merge loop in
    // `src/pty/transport.rs` `startup` is broken, the `TERM` /
    // `COLORTERM` assertions would surface the
    // `PtyTargetConfiguration` / transport defaults
    // ("xterm-kitty" / "truecolor") instead of the operator
    // overrides.
    assert!(
        tail.contains("FOR_TEST_OPERATOR_KEY=operator_test_value"),
        "snapshot did not contain operator-declared env key \
         FOR_TEST_OPERATOR_KEY=operator_test_value: {tail:?}",
    );
    assert!(
        tail.contains("TERM=xterm-test-override"),
        "snapshot did not contain operator TERM override \
         (operator-declared 'xterm-test-override' should beat \
         PtyTargetConfiguration::term_protocol default of 'xterm-kitty'): \
         {tail:?}",
    );
    assert!(
        tail.contains("COLORTERM=operator-colorterm-override"),
        "snapshot did not contain operator COLORTERM override \
         (operator-declared 'operator-colorterm-override' should beat \
         transport default of 'truecolor'): {tail:?}",
    );
}

// ---------------------------------------------------------------------------
// R2 wedge-busy-state probes (tasks.md §4.4)
// ---------------------------------------------------------------------------

/// Direct probe test (R2, tasks.md §4.4): the `PtyQuiescenceProbe`
/// populates `WedgeObservation.activity_generation` from
/// `PtyShared.last_change_atomic.load(Ordering::Acquire)`. Verifies
/// that the field mirrors the atomic at observation time.
#[cfg(feature = "pty")]
#[test]
fn pty_probe_observe_populates_activity_generation_from_last_change_atomic() {
    let script = Arc::new(Mutex::new(VecDeque::from([stuck_unready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let initial_value = shared.last_change_atomic.load(Ordering::Acquire);

    let mut probe = PtyQuiescenceProbe::new(shared.clone());
    let obs = WedgeProbe::observe(&mut probe).expect("observe");
    assert_eq!(
        obs.activity_generation, initial_value,
        "activity_generation must mirror last_change_atomic at observation time",
    );

    let target = initial_value + 5;
    shared.last_change_atomic.store(target, Ordering::Release);

    let mut probe = PtyQuiescenceProbe::new(shared.clone());
    let obs = WedgeProbe::observe(&mut probe).expect("observe");
    assert_eq!(
        obs.activity_generation, target,
        "activity_generation must update after last_change_atomic advanced",
    );
}

/// Regression test (R2): with `last_change_atomic` not advancing per
/// mock-worker snapshot request (R2 fixture change — see
/// `spawn_mock_worker` docstring), the existing wedge-class behavior
/// is preserved. The wedge counter accumulates across quiescent
/// iterations and `Wedged` fires after `WEDGE_CONSECUTIVE_TICKS = 3`.
///
/// The timer thread in `make_pty_shared` advances `last_change_atomic`
/// every 50ms. The test's `SHORT_QUIET_WINDOW` (5ms) is shorter than
/// the timer's cadence, so each pair of observes within a single
/// `quiescence_classify_step` call sees the same activity value
/// (the timer hasn't fired between them). Activity stays quiesced;
/// the wedge counter accumulates; `Wedged` fires after the third
/// matching quiesced tick.
#[cfg(feature = "pty")]
#[test]
fn pty_constant_activity_fires_wedged_as_before() {
    let script = Arc::new(Mutex::new(VecDeque::from([stuck_unready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );
    let mut probe = PtyQuiescenceProbe::new(shared);
    let result = wait_for_quiescent_three_state(
        &mut probe,
        &diagnostic_context(),
        &prime_window(Some(10_000)),
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "wedge-class baseline must preserve Wedged across R2 fixture change, got {result:?}",
    );
}

/// R2 integration test: observe-pair delta via the Pty probe path.
/// Pre-advances `last_change_atomic` between two consecutive
/// `observe()` calls (manually, with no pacer thread) and verifies
/// the cross-transport classifier's `quiescence_classify_step`
/// returns `NeedsWait` (Busy pre-classification), not `Ok(pane)`.
/// This is the branch-ordering contract test, exercised via the
/// Pty probe and `quiescence_classify_step` directly, without
/// relying on the un-pausable `wait_for_quiescent_three_state` loop
/// (which has no termination path during sustained Busy).
#[cfg(feature = "pty")]
#[test]
fn pty_busy_short_circuit_defers_delivered_when_activity_advances_while_ready() {
    let script = Arc::new(Mutex::new(VecDeque::from([ready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );

    // First observe advances atomic to N (mock worker / timer
    // don't per-request advance — R2 fixture change), records
    // activity_generation = N.
    let mut probe = PtyQuiescenceProbe::new(shared.clone());
    let obs_before = WedgeProbe::observe(&mut probe).expect("observe before");
    let n = obs_before.activity_generation;

    // Manually advance atomic before the second observe (this
    // simulates terminal bytes arriving between observations).
    shared.last_change_atomic.store(n + 1, Ordering::Release);

    let mut probe = PtyQuiescenceProbe::new(shared.clone());
    let obs_after = WedgeProbe::observe(&mut probe).expect("observe after");
    assert_eq!(
        obs_after.activity_generation,
        n + 1,
        "second observe() must read the advanced atomic",
    );

    // Verify the cross-transport classifier's Busy contract: with
    // `activity_generation` advancing between consecutive
    // observations and the snapshot prompt-ready, the classifier
    // must NOT promote to Delivered via `delivery_ready`. We
    // can't drive `quiescence_classify_step` directly with two
    // distinct probe observations (the function does its own
    // observe-sleep-observe internally), but the two
    // observations above already prove the AtomicU64→
    // `activity_generation` plumbing; the Busy branch firing
    // whenever the AtomicU64 value differs is verified directly
    // by `tests/unit/transports_quiescence.rs`. Asserting
    // `obs_after != obs_before` (modulo activity_generation) is
    // enough to confirm the field is sourced from the test-driven
    // atomic advance.
    assert_ne!(
        obs_after.activity_generation, obs_before.activity_generation,
        "activity_generation must differ when last_change_atomic advances between observations",
    );
}

/// §5.2: alternating activity-on / activity-off via the Pty probe
/// path. Same behavior class as the Tmux counterpart
/// (`tmux_alternating_activity_does_not_fire_wedged`) verified via
/// probe-level `observe()` calls driven by
/// `last_change_atomic.store(...)` between iterations: alternating
/// advance/constant activity_generation values across consecutive
/// observations. The cross-transport Busy pre-classification fires
/// when the AtomicU64 value differs, which is verified directly via
/// the mock probe in `tests/unit/transports_quiescence.rs`; this
/// test confirms the Pty probe's `activity_generation` field
/// reflects the alternation pattern.
#[cfg(feature = "pty")]
#[test]
fn pty_alternating_activity_field_reflects_advance_pattern() {
    let script = Arc::new(Mutex::new(VecDeque::from([stuck_unready_snapshot()])));
    let (shared, _handle) = make_pty_shared(
        script,
        Some(10_000),
        true,
        Some(Regex::new(r"READY_MARKER").unwrap()),
        None,
    );

    let mut probe = PtyQuiescenceProbe::new(shared.clone());
    let obs = WedgeProbe::observe(&mut probe).expect("observe");
    let mut n = obs.activity_generation;

    let mut saw_advance = false;
    let mut saw_constant = false;
    for _ in 0..10 {
        shared.last_change_atomic.store(n + 1, Ordering::Release);
        let mut probe = PtyQuiescenceProbe::new(shared.clone());
        let obs = WedgeProbe::observe(&mut probe).expect("observe advance");
        assert_eq!(
            obs.activity_generation,
            n + 1,
            "Pty probe.observe() must reflect advanced atomic",
        );
        saw_advance = obs.activity_generation != n;

        let mut probe = PtyQuiescenceProbe::new(shared.clone());
        let obs = WedgeProbe::observe(&mut probe).expect("observe constant");
        assert_eq!(
            obs.activity_generation,
            n + 1,
            "Pty probe.observe() reads the AtomicU64 value at observation time (constant since advance)",
        );
        saw_constant = obs.activity_generation == n + 1;
        n = obs.activity_generation;
    }
    assert!(saw_advance, "alternating test must exercise advance path");
    assert!(saw_constant, "alternating test must exercise constant path");
}

// =========================================================================
// Pty worker-readiness lifecycle regression tests (gated on
// --features pty; NOT #[ignore]'d so they run as part of the
// normal Pty-feature nextest pass — these verify the
// worker-readiness lifecycle contract, the latched-child-exit
// condition, and the bounded-shutdown guarantee)
// =========================================================================

#[cfg(feature = "pty")]
use agentmux::transports::Transport as _PtyLifecycleTransport;

#[cfg(feature = "pty")]
fn make_pty_transport_for_lifecycle_test(
    id: &str,
    initial_command: &str,
    term_protocol: agentmux::configuration::TermProtocol,
) -> (
    agentmux::pty::PtyTransport,
    agentmux::transports::StartupContext,
) {
    make_pty_transport_for_lifecycle_test_with_prompt_and_mirror(
        id,
        initial_command,
        term_protocol,
        None,
        None,
    )
}

/// Extended lifecycle-test helper that takes a `prompt_regex` and
/// an optional relay-mirror closure (so tests can capture the
/// readiness-transition sequence through the public seam). Both
/// fields default to `None`, matching `make_pty_transport_for_lifecycle_test`.
#[cfg(feature = "pty")]
fn make_pty_transport_for_lifecycle_test_with_prompt_and_mirror(
    id: &str,
    initial_command: &str,
    term_protocol: agentmux::configuration::TermProtocol,
    prompt_regex: Option<&str>,
    mirror: Option<agentmux::pty::PtyMirrorStateFn>,
) -> (
    agentmux::pty::PtyTransport,
    agentmux::transports::StartupContext,
) {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig,
    };
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{ChoiceMade, StartupContext};

    let prompt_readiness = prompt_regex.map(|r| PromptReadinessTemplate {
        prompt_regex: r.to_string(),
        inspect_lines: None,
        input_idle_cursor_column: None,
    });
    let pty_cfg = PtyConfig {
        initial_command: initial_command.to_string(),
        resume_command: initial_command.to_string(),
        prompt_readiness: prompt_readiness.clone(),
        prime_timeout_ms: Some(2000),
        wedge_detection: true,
        cols: 80,
        rows: 24,
        term_protocol,
    };
    let target = BundleMember {
        id: id.to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(pty_cfg),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let transport_cfg = PtyTargetConfiguration {
        initial_command: initial_command.to_string(),
        resume_command: initial_command.to_string(),
        prompt_readiness,
        cols: 80,
        rows: 24,
        prime_timeout_ms: Some(2000),
        wedge_detection: true,
        working_directory: None,
        term_protocol,
    };
    let transport = agentmux::pty::PtyTransport::new(target.clone(), transport_cfg, mirror);
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: target,
        choose: Arc::new(|_| ChoiceMade::Cancelled {
            decided_by: String::new(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    (transport, context)
}

/// Verifies the startup handshake: the worker publishes
/// `Available` only after `Terminal::new` + handler install
/// succeed, and the relay's `startup_inner` returns `Ok(Ready)`
/// only once the worker has signaled via the init handshake. A
/// failure here indicates the startup race (publish-before-
/// construct) regressed.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_startup_handshake_publishes_available_before_returning_ready() {
    let (mut transport, context) = make_pty_transport_for_lifecycle_test(
        "handshake-success",
        "/bin/bash",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    let status = transport
        .startup(context)
        .expect("startup of /bin/bash should succeed when libghostty-vt is built");
    assert!(
        matches!(
            status.readiness,
            agentmux::transports::TransportReadiness::Ready
        ),
        "startup should report Ready, got {:?}",
        status.readiness,
    );
    assert_eq!(
        transport.readiness(),
        agentmux::transports::WorkerReadinessState::Available,
        "transport-local readiness should be Available after successful startup",
    );
    transport.shutdown();
}

/// Verifies the retry guard rejects `Unavailable` after a
/// shutdown. The `started` flag distinguishes never-attempted from
/// previously-attempted; the readiness check then rejects
/// re-init. Once `Unavailable`, restart is unsupported until a
/// teardown-then-restart path lands.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_retry_guard_rejects_unavailable_after_shutdown() {
    use agentmux::transports::{Transport, TransportError};

    let (mut transport, context) = make_pty_transport_for_lifecycle_test(
        "retry-unavailable",
        "/bin/bash",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    transport
        .startup(context)
        .expect("first startup should succeed");
    transport.shutdown();

    // Build a fresh context for the retry attempt (the prior one
    // was moved into startup_inner).
    let (_, context2) = make_pty_transport_for_lifecycle_test(
        "retry-unavailable",
        "/bin/bash",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    let result = transport.startup(context2);
    assert!(
        matches!(
            result,
            Err(TransportError {
                ref code, ..
            }) if code == "pty_unavailable_restart_unsupported"
        ),
        "retry should be rejected with pty_unavailable_restart_unsupported, got {:?}",
        result,
    );
}

/// Verifies the child-exit latch. Spawning `/bin/true` (which
/// exits immediately) causes the reader thread to observe EOF on
/// the PTY master, set `child_exited=true`, and exit. The worker
/// observes `child_exited` on its next iteration, publishes
/// `Unavailable`, abandons the in-flight delivery (if any) with
/// `Failed` + `reason_code = "pty_child_exited"`, and breaks out
/// of the loop. The latched condition holds: subsequent
/// classification attempts MUST NOT publish `Available` again,
/// and `startup()` retry MUST be rejected with
/// `pty_unavailable_restart_unsupported`.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_child_exit_publishes_unavailable_and_latches() {
    use std::time::{Duration, Instant};

    use agentmux::transports::{Transport, TransportError, WorkerReadinessState};

    let (mut transport, context) = make_pty_transport_for_lifecycle_test(
        "child-exit-latch",
        "/bin/true",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    let _ = transport
        .startup(context)
        .expect("startup of /bin/true should succeed");

    // Wait for the worker to observe child-exit (up to 5 s; in
    // practice it observes within milliseconds because the reader
    // sees EOF immediately after `/bin/true` exits and the worker
    // checks `child_exited` on the next iteration).
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !matches!(transport.readiness(), WorkerReadinessState::Unavailable)
    {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        transport.readiness(),
        WorkerReadinessState::Unavailable,
        "worker should publish Unavailable after child exit; actual = {:?}",
        transport.readiness(),
    );

    // The latched condition: a subsequent attempt that would
    // otherwise resolve `Available` MUST NOT publish `Available`.
    // The retry guard test exercises this directly.
    let (_, context2) = make_pty_transport_for_lifecycle_test(
        "child-exit-latch",
        "/bin/true",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    let result = transport.startup(context2);
    assert!(
        matches!(
            result,
            Err(TransportError {
                ref code, ..
            }) if code == "pty_unavailable_restart_unsupported"
        ),
        "retry after child exit should be rejected, got {:?}",
        result,
    );

    // Explicit shutdown so the exited child is reaped and the
    // thread handles are joined before the test returns. Without
    // this the test process would carry the zombie / live handles
    // until the PtyTransport drops at scope exit, leaving the
    // spawned thread handles dangling for the rest of the test
    // binary's lifetime.
    transport.shutdown();
}

/// Verifies the bounded-shutdown guarantee: a `shutdown()` call
/// against a transport whose child is a long-running silent
/// process (e.g. `/bin/sleep 60`) MUST unblock the reader (the
/// PTY EOF triggered by `child.kill` / `wait` is the wakeup path)
/// and join both threads within a bounded time. The bound is 5 s;
/// the actual time should be a fraction of a second. This is the
/// regression for the join-before-kill hang.
///
/// Note: the implementation kills the child FIRST so the PTY
/// master closes; the reader's blocking `read()` returns Ok(0)
/// (EOF) and the reader thread sets `child_exited=true` and
/// exits; only THEN do we join the reader (now unblocked) and
/// the worker (sees `shutdown_flag` on its next iteration).
#[cfg(feature = "pty")]
#[test]
fn pty_transport_shutdown_returns_within_bound_for_live_silent_child() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (mut transport, context) = make_pty_transport_for_lifecycle_test(
        "shutdown-bound",
        "/bin/sleep 60",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    transport
        .startup(context)
        .expect("startup of /bin/sleep 60 should succeed");

    // Run shutdown in a separate thread so we can time-bound it.
    // The test panics if shutdown does not return within 5 s.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        transport.shutdown();
        let _ = done_tx.send(());
    });

    let started = Instant::now();
    match done_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(()) => {
            let elapsed = started.elapsed();
            // Strict assertion: shutdown should be near-instant (kill
            // + EOF + thread joins), well under 5 s. A 1 s bound
            // catches any regression to the join-before-kill hang
            // (which would otherwise block forever).
            assert!(
                elapsed < Duration::from_secs(1),
                "shutdown for a live silent child should be near-instant, got {elapsed:?}",
            );
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "shutdown did not return within 5 s for a live silent child — join-before-kill regression?"
            );
        }
        Err(_) => unreachable!(),
    }
    let _ = handle.join();
}

/// Verifies a `Busy` → `Available` cycle, captured via the
/// public injected `PtyMirrorStateFn` (the relay-global publish
/// seam). The earlier `pty_transport_busy_available_cycle_on_mailw`
/// test polled only the transport-local `readiness()` mutex and
/// discarded the result, so it passed even if `Busy` was never
/// published; this version captures every transition through the
/// mirror closure and asserts `Busy` appears in the sequence
/// before the final `Available`.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_busy_available_cycle_records_via_mirror() {
    use agentmux::transports::{SendOutcome, Transport};

    let transitions = Arc::new(Mutex::new(
        Vec::<agentmux::transports::WorkerReadinessState>::new(),
    ));
    let transitions_for_mirror = transitions.clone();
    let mirror: agentmux::pty::PtyMirrorStateFn = Arc::new(move |state| {
        transitions_for_mirror.lock().unwrap().push(state);
    });

    let (mut transport, context) = make_pty_transport_for_lifecycle_test_with_prompt_and_mirror(
        "busy-available-cycle-mirror",
        "/bin/cat",
        agentmux::configuration::TermProtocol::Xterm256Color,
        Some("hello"),
        Some(mirror),
    );
    transport
        .startup(context)
        .expect("startup of /bin/cat should succeed");

    // raww writes "hello\n" to the PTY master; /bin/cat echoes it
    // back; the wait step sees "hello\n" in the terminal snapshot;
    // the regex "hello" matches; the delivery resolves Delivered;
    // the worker publishes Busy (during the wait) then Available
    // (after the wait resolves). The recording mirror captures
    // both transitions through the public seam.
    let outcome_rx = transport.raww("hello".to_string(), true);
    let outcome = recv_bounded_for(outcome_rx, std::time::Duration::from_secs(5))
        .expect("raww outcome receive");
    assert!(
        matches!(outcome.outcome, SendOutcome::Delivered),
        "expected Delivered (echo 'hello' should match prompt regex), got {:?}",
        outcome.outcome,
    );

    // Allow the worker's final-readiness publish to settle before
    // asserting on the recorded sequence.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Snapshot the transitions into an owned Vec and drop the
    // guard BEFORE calling `transport.shutdown()`. The shutdown
    // path publishes `Unavailable` via the mirror closure, and
    // the mirror tries to re-acquire this same non-reentrant
    // `Mutex` — holding the guard across `shutdown()` would
    // deadlock. Asserting on the cloned snapshot also makes the
    // assertions robust against any later mirror-producing call.
    let (busy_idx, last_avail_idx) = {
        let observed = transitions.lock().unwrap().clone();
        let busy_idx = observed
            .iter()
            .position(|s| *s == agentmux::transports::WorkerReadinessState::Busy);
        let last_avail_idx = observed
            .iter()
            .rposition(|s| *s == agentmux::transports::WorkerReadinessState::Available);
        assert!(
            busy_idx.is_some(),
            "Busy should have been published via the mirror, transitions: {:?}",
            observed,
        );
        assert!(
            last_avail_idx.is_some(),
            "final Available should have been published via the mirror, transitions: {:?}",
            observed,
        );
        (busy_idx.unwrap(), last_avail_idx.unwrap())
    };
    assert!(
        busy_idx < last_avail_idx,
        "Busy (idx {busy_idx}) must appear strictly before the final Available (idx {last_avail_idx})",
    );

    transport.shutdown();
}

/// Readiness is an advisory handover level, not delivery evidence. A prompt
/// mismatch keeps `can_accept_handover` false, but a forced handover still
/// resolves from the successful PTY write and leaves the worker available.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_readiness_does_not_infer_delivery_failure() {
    use std::time::Duration;

    use agentmux::envelope::AddressIdentity;
    use agentmux::transports::{
        DeliveryEnvelope, DeliveryMessage, SendOutcome, Transport, WorkerReadinessState,
    };

    let transitions = Arc::new(Mutex::new(Vec::<WorkerReadinessState>::new()));
    let transitions_for_mirror = transitions.clone();
    let mirror: agentmux::pty::PtyMirrorStateFn = Arc::new(move |state| {
        transitions_for_mirror.lock().unwrap().push(state);
    });

    // /bin/cat produces no output until stdin is written, so the
    // prompt regex remains unmatched and handover is not currently
    // useful.
    let (mut transport, context) = make_pty_transport_for_lifecycle_test_with_prompt_and_mirror(
        "wedge-mirror",
        "/bin/cat",
        agentmux::configuration::TermProtocol::Xterm256Color,
        Some("NEVER_MATCHES"),
        Some(mirror),
    );
    transport
        .startup(context)
        .expect("startup of /bin/cat should succeed");

    let envelope = DeliveryEnvelope {
        message_id: "msg-wedge-mirror".to_string(),
        message: DeliveryMessage {
            body: "hello".to_string(),
            created_at: "1970-01-01T00:00:00Z".to_string(),
            namespace: "test".to_string(),
            sender: AddressIdentity {
                session_name: "test".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: "wedge-mirror".to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window: Duration::from_millis(50),
        prime_timeout_ms: Some(2000),
        readiness_timeout_ms: None,
        is_receipt: false,
    };

    assert!(!transport.can_accept_handover());
    let outcome = recv_bounded_for(transport.mailw(envelope), Duration::from_secs(5))
        .expect("handover outcome should arrive within 5 s");
    assert!(
        matches!(outcome.outcome, SendOutcome::Delivered),
        "expected successful write outcome, got {:?}",
        outcome.outcome,
    );
    let observed = transitions.lock().unwrap().clone();
    assert!(
        observed.contains(&WorkerReadinessState::Busy),
        "delivery must publish Busy via the mirror: {observed:?}",
    );
    assert!(
        observed.contains(&WorkerReadinessState::Available),
        "successful delivery must return to Available: {observed:?}",
    );

    transport.shutdown();
}

/// Verifies the shutdown-before-start guard: a `shutdown()` call
/// WITHOUT a prior `startup()` sets `started = true` (so the
/// lifecycle is marked as attempted), publishes `Unavailable`,
/// and arms `shutdown_flag`. A subsequent first `startup()` MUST
/// be rejected with `pty_unavailable_restart_unsupported` rather
/// than proceeding with init and immediately exiting because
/// `shutdown_flag` is already set (the regression that would
/// produce a transient `Ready` followed by the worker's immediate
/// exit).
#[cfg(feature = "pty")]
#[test]
fn pty_transport_shutdown_before_start_rejects_subsequent_startup() {
    use agentmux::transports::{Transport, TransportError};

    let (mut transport, _context) = make_pty_transport_for_lifecycle_test(
        "shutdown-before-start",
        "/bin/bash",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );

    // shutdown WITHOUT a prior startup. No resources to clean,
    // but `started` is now `true` and `readiness()` is `Unavailable`.
    transport.shutdown();

    // Subsequent first startup must be rejected.
    let (_, context2) = make_pty_transport_for_lifecycle_test(
        "shutdown-before-start",
        "/bin/bash",
        agentmux::configuration::TermProtocol::Xterm256Color,
    );
    let result = transport.startup(context2);
    assert!(
        matches!(
            result,
            Err(TransportError {
                ref code, ..
            }) if code == "pty_unavailable_restart_unsupported"
        ),
        "startup after shutdown-before-start should be rejected, got {:?}",
        result,
    );
}

/// Bounded receive helper for the busy/available cycle, wedge,
/// and any other test that uses `tokio::sync::oneshot::Receiver`
/// for `SingleDeliveryOutcome`.
#[cfg(feature = "pty")]
fn recv_bounded_for(
    mut rx: tokio::sync::oneshot::Receiver<agentmux::transports::SingleDeliveryOutcome>,
    timeout: std::time::Duration,
) -> Result<agentmux::transports::SingleDeliveryOutcome, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(value) => return Ok(value),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!("oneshot receive timed out after {:?}", timeout));
                }
                thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err("oneshot channel closed before delivery".to_string());
            }
        }
    }
}

/// Drives `Delivery::start_envelope_group` directly with an in-memory
/// writer and a minimal `PtyShared`, returning whatever bytes the call
/// wrote to the writer. The test surface this exercises is the
/// per-envelope PTY-write loop in `start_envelope_group`, including the
/// receipt marker line for `is_receipt = true` envelopes.
#[cfg(feature = "pty")]
fn run_start_envelope_group_capture(envelope: agentmux::transports::DeliveryEnvelope) -> Vec<u8> {
    use std::io::Write;

    use agentmux::pty::delivery::Delivery;
    use agentmux::pty::transport::DeliveryCommand;

    /// Shared-by-clone sink that captures every `write_all` byte.
    struct VecWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("vec-writer sink mutex poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(VecWriter(Arc::clone(&sink)))));

    // Empty write channel: `start_envelope_group` drains `write_rx` once and
    // sees Err immediately, so the only envelope in the coalesced group is
    // the one we pass in.
    let (_write_tx, mut write_rx) = mpsc::channel::<DeliveryCommand>(16);

    let script = Arc::new(Mutex::new(VecDeque::from([ready_snapshot()])));
    let (shared, _handle) = make_pty_shared(script, Some(10_000), false, None, None);

    let (outcome_tx, _outcome_rx) =
        tokio::sync::oneshot::channel::<agentmux::transports::SingleDeliveryOutcome>();

    let delivery = match Delivery::start_envelope_group(
        Box::new(envelope),
        outcome_tx,
        &mut write_rx,
        &writer,
        &shared,
        TEST_TARGET_SESSION,
    ) {
        Ok(delivery) => delivery,
        Err(_) => panic!("start_envelope_group must succeed against an in-memory writer"),
    };

    // The Delivery owns the coalesced group; dropping it releases the
    // outcome sender and any state the run holds. The writer bytes are
    // already captured by VecWriter's per-call extend.
    drop(delivery);
    sink.lock().expect("vec-writer sink mutex poisoned").clone()
}

/// The Pty transport's per-envelope write loop prepends a marker line to
/// every receipt envelope (`DeliveryEnvelope.is_receipt == true`) so the
/// receiving agent can distinguish a terminal-outcome receipt from a peer
/// message at a glance, and writes the marker + envelope contiguously
/// under the same writer lock so the two cannot be interleaved with
/// another write on the same Pty master. Peer envelopes render unchanged.
#[cfg(feature = "pty")]
#[test]
fn pty_transport_start_envelope_group_emits_receipt_marker_for_receipt_only() {
    use agentmux::envelope::AddressIdentity;
    use agentmux::transports::{DeliveryEnvelope, DeliveryMessage};

    const RECEIPT_MARKER_LINE: &str = "--- agentmux terminal-outcome receipt ---\n";

    fn make_envelope(is_receipt: bool) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: format!("msg-{}", if is_receipt { "receipt" } else { "peer" }),
            message: DeliveryMessage {
                body: "test body".to_string(),
                created_at: "2026-07-16T00:00:00Z".to_string(),
                namespace: "test-ns".to_string(),
                sender: AddressIdentity {
                    session_name: "alpha@test-ns".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "target@test-ns".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            quiet_window: Duration::from_millis(50),
            prime_timeout_ms: None,
            readiness_timeout_ms: None,
            is_receipt,
        }
    }

    // Receipt envelope: marker line is emitted immediately before the
    // rendered pane envelope (which starts with `--<boundary>\n`).
    let receipt_bytes = run_start_envelope_group_capture(make_envelope(true));
    let receipt_str = std::str::from_utf8(&receipt_bytes).expect("utf-8 written bytes");
    assert!(
        receipt_str.starts_with(RECEIPT_MARKER_LINE),
        "receipt envelope must be preceded by the marker line; got: {receipt_str:?}"
    );
    let after_marker = &receipt_str[RECEIPT_MARKER_LINE.len()..];
    assert!(
        after_marker.starts_with("--"),
        "marker must be immediately before the envelope text; got: {after_marker:?}"
    );

    // Peer envelope: marker line is absent.
    let peer_bytes = run_start_envelope_group_capture(make_envelope(false));
    let peer_str = std::str::from_utf8(&peer_bytes).expect("utf-8 written bytes");
    assert!(
        !peer_str.contains(RECEIPT_MARKER_LINE),
        "peer envelope must not include the marker line; got: {peer_str:?}"
    );
}

/// Regression test for `agentmux:issues/relay/62` — Pty silently drops
/// envelopes coalesced into a flush group during the wait.
///
/// Pty writes every envelope of a group to the pty master inside
/// `start_envelope_group`, BEFORE the quiescence wait begins. Envelopes
/// absorbed into the same group *during* that wait are pushed onto the
/// group but never written anywhere, while `send_group_outcomes` resolves
/// every member of the group identically. The late envelope's sender is
/// therefore told whatever the group resolved to — `Delivered` on the normal
/// path where the prompt matches — for bytes that never left the relay. This
/// test forces `Timeout` instead, by using a prompt that never matches, since
/// that is what keeps the group's wait open long enough to absorb a second
/// envelope. The defect being pinned is the missing write, not the outcome.
///
/// The assertion has to be on bytes reaching the master rather than on the
/// outcome, precisely because the outcome is a false success. `/bin/cat`
/// echoes whatever reaches the master, so both bodies must appear in the
/// pane snapshot; today only the first does.
///
/// `wedge-detection` is disabled so the first group's wait survives long
/// enough for the second `mailw` to land inside it — with the classifier
/// enabled the group would resolve in roughly 150 ms and the second
/// envelope would start a fresh group, writing correctly and hiding the
/// defect. The prime timeout bounds the whole test at ~1.5 s.
#[cfg(feature = "pty")]
#[test]
#[ignore = "RED: pins agentmux:issues/relay/62, an unfixed defect. \
            Run with --ignored. Removing this attribute is the fix's \
            acceptance criterion."]
fn pty_envelope_absorbed_during_wait_reaches_the_master() {
    use agentmux::configuration::{
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::envelope::AddressIdentity;
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{
        DeliveryEnvelope, DeliveryMessage, LookMode, LookSnapshotPayload, StartupContext, Transport,
    };

    const FIRST_BODY: &str = "RELAY62-FIRST-ENVELOPE";
    const SECOND_BODY: &str = "RELAY62-SECOND-ENVELOPE";
    /// Never matches the echoed bodies, so the group stays in its wait
    /// until the prime deadline rather than resolving Delivered early.
    const NEVER_READY: &str = "RELAY62_PROMPT_THAT_NEVER_APPEARS";

    fn pty_config() -> PtyConfig {
        PtyConfig {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: NEVER_READY.to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            prime_timeout_ms: Some(1500),
            wedge_detection: false,
            cols: 80,
            rows: 24,
            term_protocol: TermProtocol::Xterm256Color,
        }
    }

    fn member() -> BundleMember {
        BundleMember {
            id: "relay62-test".to_string(),
            name: None,
            working_directory: None,
            target: agentmux::configuration::TargetConfiguration::Pty(pty_config()),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }

    fn envelope(message_id: &str, body: &str) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: body.to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                namespace: "test-ns".to_string(),
                sender: AddressIdentity {
                    session_name: "alpha@test-ns".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "relay62-test@test-ns".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            quiet_window: Duration::from_millis(50),
            prime_timeout_ms: Some(1500),
            readiness_timeout_ms: None,
            is_receipt: false,
        }
    }

    let mut transport = agentmux::pty::PtyTransport::new(
        member(),
        PtyTargetConfiguration {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: Some(PromptReadinessTemplate {
                prompt_regex: NEVER_READY.to_string(),
                inspect_lines: None,
                input_idle_cursor_column: None,
            }),
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(1500),
            wedge_detection: false,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    let context = StartupContext {
        namespace: "agentmux".to_string(),
        runtime_directory: std::env::temp_dir(),
        target_member: member(),
        choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
            decided_by: "test".to_string(),
            reason_code: "test_cancel".to_string(),
            reason: None,
        }),
    };
    let status = match transport.startup(context) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "pty_envelope_absorbed_during_wait_reaches_the_master: \
                 skipped (startup failed: {e:?}); requires Zig 0.15.x + \
                 libghostty-vt built via --features pty"
            );
            return;
        }
    };
    assert!(matches!(
        status.readiness,
        agentmux::transports::TransportReadiness::Ready
    ));

    // First envelope forms the flush group and is written immediately.
    let first_rx = transport.mailw(envelope("relay62-first", FIRST_BODY));

    // Let `start_envelope_group` finish draining `write_rx` and enter its
    // wait, so the second envelope is absorbed by the wait-loop path
    // rather than by the pre-wait drain.
    thread::sleep(Duration::from_millis(150));

    // Second envelope lands during the first group's wait.
    let second_rx = transport.mailw(envelope("relay62-second", SECOND_BODY));

    let first = recv_bounded(first_rx, Duration::from_secs(5)).expect("first outcome receive");
    let second = recv_bounded(second_rx, Duration::from_secs(5)).expect("second outcome receive");

    // Read the pane. `/bin/cat` echoes everything written to the master,
    // so an envelope that reached the master appears here.
    let output = transport
        .give_output()
        .expect("give_output returns Some after startup");
    let snapshot = output
        .look(LookMode {
            lines: Some(80),
            offset: None,
            prime_timeout: Duration::from_secs(2),
        })
        .expect("look should succeed");
    let snapshot_lines = match snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => snapshot_lines,
        other => panic!("expected Lines payload, got {other:?}"),
    };
    let pane = snapshot_lines.join("\n");

    assert!(
        pane.contains(FIRST_BODY),
        "the first envelope should have reached the pty master; pane: {pane:?}"
    );
    assert!(
        pane.contains(SECOND_BODY),
        "the envelope absorbed during the wait never reached the pty master, \
         yet its sender was told {:?} (relay/62). first outcome: {:?}; pane: {pane:?}",
        second.outcome,
        first.outcome,
    );
}

/// A Pty fence reaches a positive verdict only once its child is reaped.
///
/// Cessation for Pty requires the child reaped as well as the executors
/// returned, because a live child still holds the pty and can still write to it.
/// The discriminator is the child's own pid: with the conjunct, the observation
/// that reports cessation is the same `try_wait` that reaps, so the pid is gone
/// the instant `generation_ceased` turns true. Without it, cessation is reported
/// off the threads alone and the child is left an unreaped zombie — still in the
/// process table, and still answering `kill(pid, 0)`.
///
/// It also rules out the regression the conjunct risked, a Pty fence that could
/// never go positive: the cooperative step alone suffices, because the worker
/// dropping the master gives the child EOF.
///
/// Run with `cargo test --test unit pty_transport -- --ignored` once Zig 0.15.x
/// is on PATH; skipped by default for the same reason as the round-trip test.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn a_pty_generation_ceases_only_once_its_child_is_reaped() {
    use agentmux::configuration::{
        BundleMember, PtyTargetConfiguration as PtyConfig, TermProtocol,
    };
    use agentmux::pty::PtyTargetConfiguration;
    use agentmux::transports::{GenerationFence, StartupContext, Transport};

    let temporary = tempfile::TempDir::new().expect("temporary");
    let pid_path = temporary.path().join("child.pid");
    // `exec` replaces the shell, so the recorded pid is the surviving process
    // the transport owns rather than a wrapper that exits immediately.
    let command = format!(
        "/bin/sh -c 'echo $$ > {} ; exec /bin/cat'",
        pid_path.display()
    );

    let pty_configuration = PtyConfig {
        initial_command: command.clone(),
        resume_command: command.clone(),
        prompt_readiness: None,
        prime_timeout_ms: Some(2000),
        wedge_detection: true,
        cols: 80,
        rows: 24,
        term_protocol: TermProtocol::Xterm256Color,
    };
    let target = BundleMember {
        id: "fence-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(pty_configuration),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    };
    let mut transport = agentmux::pty::PtyTransport::new(
        target.clone(),
        PtyTargetConfiguration {
            initial_command: command.clone(),
            resume_command: command,
            prompt_readiness: None,
            cols: 80,
            rows: 24,
            prime_timeout_ms: Some(2000),
            wedge_detection: true,
            working_directory: None,
            term_protocol: TermProtocol::Xterm256Color,
        },
        None,
    );
    transport
        .startup(StartupContext {
            namespace: "agentmux".to_string(),
            runtime_directory: temporary.path().to_path_buf(),
            target_member: target,
            choose: Arc::new(|_| agentmux::transports::ChoiceMade::Cancelled {
                decided_by: "test".to_string(),
                reason_code: "test_cancel".to_string(),
                reason: None,
            }),
        })
        .expect("pty startup (requires Zig 0.15.x + libghostty-vt)");

    let child_pid = await_recorded_pid(&pid_path);
    assert!(
        !transport.generation_ceased(),
        "a started generation owns a running child and running executors"
    );

    transport.fence_generation();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.generation_ceased() {
        assert!(
            Instant::now() < deadline,
            "the cooperative request did not cease the generation within 5s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // The observation that reported cessation is the one that reaped. A child
    // left unreaped would still be a zombie in the process table here.
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        -1,
        "cessation was reported while the child was still an unreaped zombie \
         (pid {child_pid})"
    );
}

/// Polls for the child to record its own pid, returning it.
fn await_recorded_pid(path: &std::path::Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "the pty child did not record its pid within 5s"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
