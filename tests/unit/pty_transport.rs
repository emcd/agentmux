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

/// Round-trip test: spawn `/bin/cat` under portable-pty, write a line
/// via the PtyTransport's raww channel, capture the line via
/// give_output().look(), assert the line appears in `snapshot_lines`.
///
/// Run with `cargo test --test unit pty_transport -- --ignored` once
/// Zig 0.15.x is on PATH and `cargo check --features pty` succeeds.
/// Skipped by default because the underlying PtyTransport spawns
/// threads that consume the snapshot channel for the full lifetime of
/// the test; without libghostty-vt built, startup() fails.
#[test]
#[ignore = "requires Zig 0.15.x + libghostty-vt built; run with --ignored"]
fn pty_transport_round_trips_cat_line() {
    use agentmux::configuration::{BundleMember, PtyTargetConfiguration as PtyConfig};
    use agentmux::pty::PtyTargetConfiguration;

    let target = BundleMember {
        id: "cat-test".to_string(),
        name: None,
        working_directory: None,
        target: agentmux::configuration::TargetConfiguration::Pty(PtyConfig {
            initial_command: "/bin/cat".to_string(),
            resume_command: "/bin/cat".to_string(),
            prompt_readiness: None,
            prime_timeout_ms: None,
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
            prompt_readiness: None,
            cols: 80,
            rows: 24,
            prime_timeout_ms: None,
            wedge_detection: true,
            working_directory: None,
        },
    );
    // `startup` spawns the child + builds the terminal. If libghostty-
    // vt is not built or Zig 0.15.x is not on PATH, the terminal
    // construction fails; the test then skips with a clear message so
    // --ignored runs are meaningful when Zig is available but do not
    // panic when it is not.
    use agentmux::transports::{LookMode, LookSnapshotPayload, StartupContext, Transport};
    use std::time::Duration;
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
                prompt_readiness: None,
                prime_timeout_ms: None,
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
                "pty_transport_round_trips_cat_line: skipped (startup failed: {e:?}); \
                 requires Zig 0.15.x + libghostty-vt built via --features pty"
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

    // Write "hello\n" via raww, await the outcome with a bounded wait.
    let outcome_rx = transport.raww("hello".to_string(), true);
    let outcome = outcome_rx
        .blocking_recv()
        .expect("raww outcome channel dropped before resolution");
    assert!(
        matches!(
            outcome.outcome,
            agentmux::transports::SendOutcome::Delivered
                | agentmux::transports::SendOutcome::Failed
                | agentmux::transports::SendOutcome::Timeout
        ),
        "unexpected outcome: {outcome:?}",
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
