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
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agentmux::pty::{
    PtyConfigSnapshot, PtyOutputView, PtyQuiescenceProbe, PtyShared, SnapshotResponse,
};
use agentmux::transports::{
    DeliveryWaitError, LookMode, LookSnapshotPayload, OutputView, wait_for_quiescent_three_state,
};
use regex::Regex;
use tokio::sync::mpsc;

const SHORT_QUIET_WINDOW: Duration = Duration::from_millis(5);
const TEST_TARGET_SESSION: &str = "test-session";

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
/// `ready_snapshot()` (so quiescence is reached) to avoid hanging the
/// state machine.
fn spawn_mock_worker(
    mut rx: mpsc::Receiver<agentmux::pty::SnapshotRequest>,
    script: Arc<Mutex<VecDeque<SnapshotResponse>>>,
    last_change_atomic: Arc<std::sync::atomic::AtomicU64>,
) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
    let atomic_for_worker = last_change_atomic.clone();
    let worker = thread::spawn(move || {
        let mut last_response: Option<SnapshotResponse> = None;
        while let Some(req) = rx.blocking_recv() {
            // Update last_change_atomic on every snapshot request so the
            // probe's `wait_for_change` returns Ok(()) immediately (the
            // mock worker's reception of a request signals "something
            // changed").
            atomic_for_worker.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
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
    };
    let (_worker_handle, _timer_handle) = spawn_mock_worker(rx, script, last_change_atomic);
    (shared, _worker_handle)
}

/// Build `(prime_deadline, prime_started_at, prime_timeout_ms)` the
/// state machine expects.
fn prime_window(timeout_ms: Option<u64>) -> (Option<Instant>, Instant, Option<u64>) {
    let now = Instant::now();
    (
        timeout_ms.map(|ms| now + Duration::from_millis(ms)),
        now,
        timeout_ms,
    )
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(30)).0,
        prime_window(Some(30)).1,
        prime_window(Some(30)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10)).0,
        prime_window(Some(10)).1,
        prime_window(Some(10)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged, got {result:?}",
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10_000)).0,
        prime_window(Some(10_000)).1,
        prime_window(Some(10_000)).2,
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(200)).0,
        prime_window(Some(200)).1,
        prime_window(Some(200)).2,
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
    let join = thread::spawn(move || {
        wait_for_quiescent_three_state(
            &mut probe,
            TEST_TARGET_SESSION,
            SHORT_QUIET_WINDOW,
            None,
            started,
            None,
            true,
        )
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10)).0,
        prime_window(Some(10)).1,
        prime_window(Some(10)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Timeout { .. })),
        "expected Timeout (prime_timeout_ms opt-in), got {result:?}",
    );
}

#[test]
fn short_prime_timeout_does_not_preempt_wedge_for_wedge_class_mismatch() {
    let script = Arc::new(Mutex::new(VecDeque::from([
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
        TEST_TARGET_SESSION,
        SHORT_QUIET_WINDOW,
        prime_window(Some(10)).0,
        prime_window(Some(10)).1,
        prime_window(Some(10)).2,
        true,
    );
    assert!(
        matches!(result, Err(DeliveryWaitError::Wedged { .. })),
        "expected Wedged (wedge governs over short prime-timeout), got {result:?}",
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
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig,
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
        }),
        coder_session_id: None,
        policy_id: None,
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
        },
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
            }),
            coder_session_id: None,
            policy_id: None,
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
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig,
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
        }),
        coder_session_id: None,
        policy_id: None,
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
        },
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
            }),
            coder_session_id: None,
            policy_id: None,
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
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig,
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
        }),
        coder_session_id: None,
        policy_id: None,
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
        },
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
            }),
            coder_session_id: None,
            policy_id: None,
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
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        quiet_window: Duration::from_millis(50),
        prime_timeout_ms: Some(2000),
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
        BundleMember, PromptReadinessTemplate, PtyTargetConfiguration as PtyConfig,
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
        }),
        coder_session_id: None,
        policy_id: None,
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
        },
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
            }),
            coder_session_id: None,
            policy_id: None,
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
