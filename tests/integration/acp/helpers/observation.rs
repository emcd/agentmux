use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::{
    ChoicesQueueEvent, subscribe_choices_queue_events, subscribe_worker_readiness,
};
use agentmux::transports::WorkerReadinessState;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, watch};

use super::dispatch::dispatch_send_result;
use super::state::{read_worker_state, send_result};
pub(super) fn count_logged_method(log_path: &Path, method: &str) -> usize {
    fs::read_to_string(log_path)
        .map(|contents| {
            contents
                .matches(&format!("\"method\":\"{method}\""))
                .count()
        })
        .unwrap_or(0)
}

/// Polls the persistent ACP worker for `target_session` until it converges to
/// the expected readiness state, returning true on success within the timeout.
pub(in crate::acp) fn wait_for_worker_state(
    root: &Path,
    target_session: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    wait_for_any_worker_state(root, target_session, &[expected], timeout)
}

/// Polls the persistent ACP worker for `target_session` until it converges to
/// any of the expected readiness states, returning true on success.
pub(in crate::acp) fn wait_for_any_worker_state(
    root: &Path,
    target_session: &str,
    expected: &[&str],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(state) = read_worker_state(root, target_session)
            && expected.iter().any(|candidate| state == *candidate)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Subscribes to readiness-state transitions for `bravo` under the default
/// test bundle (`party`). Callers must invoke this BEFORE dispatching the
/// action that triggers the transition; the watch::Receiver yields the
/// current value on first borrow and every subsequent published transition.
pub(in crate::acp) fn subscribe_bravo_worker_state(
    root: &Path,
) -> watch::Receiver<Option<WorkerReadinessState>> {
    subscribe_worker_readiness("party", root, "bravo")
}

/// Subscribes to permission-queue mutation events for the runtime directory.
/// Subscriptions must be established BEFORE the action that publishes events;
/// the broadcast::Receiver only observes events that arrive after subscribe.
pub(in crate::acp) fn subscribe_bravo_permission_queue(
    root: &Path,
) -> broadcast::Receiver<ChoicesQueueEvent> {
    subscribe_choices_queue_events(root)
}

/// Polls a watch::Receiver for the ACP worker's readiness state until it
/// matches `expected`. Uses `has_changed` + `borrow_and_update` so it can be
/// called from synchronous test bodies without a tokio runtime; the channel
/// is updated in-process by the producer, so each poll is a cheap atomic
/// read with no filesystem or RPC cost.
pub(in crate::acp) fn await_acp_worker_state(
    receiver: &mut watch::Receiver<Option<WorkerReadinessState>>,
    expected: WorkerReadinessState,
    timeout: Duration,
) -> bool {
    await_acp_worker_any_state(receiver, &[expected], timeout)
}

pub(in crate::acp) fn await_acp_worker_any_state(
    receiver: &mut watch::Receiver<Option<WorkerReadinessState>>,
    expected: &[WorkerReadinessState],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(current) = *receiver.borrow_and_update()
            && expected.contains(&current)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Polls a broadcast::Receiver for the first permission-queue event matching
/// the predicate. Drops non-matching events; treats `Lagged` as a continue
/// (tests should subscribe before the action so lag is not expected).
pub(in crate::acp) fn await_permission_event<F>(
    receiver: &mut broadcast::Receiver<ChoicesQueueEvent>,
    mut matcher: F,
    timeout: Duration,
) -> bool
where
    F: FnMut(&ChoicesQueueEvent) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                if matcher(&event) {
                    return true;
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Closed) => return false,
        }
    }
}

/// Maximum wall-clock time `assert_acp_delivery_unavailable` will wait for a
/// failed-stage ACP worker to settle `Unavailable` before panicking. Sized to
/// align with `agentmux::acp::client::ACP_OPERATION_TIMEOUT` so the test
/// budget cannot be exhausted faster than the production bootstrap
/// legitimately can; the previous 10 s value undersized under the
/// `cargo nextest run --release` full-suite load profile (debug-mode full
/// suite and isolated runs in the same wall-time budget stayed clean, so
/// the constraint is load-profile-specific). On exhaustion the panic
/// message carries the observed wall time, last seen watch state, and
/// readiness-poll state so a future release-mode nextest flake can be
/// triaged from the panic alone rather than re-deriving context from
/// relay inscriptions.
pub(super) const ACP_FAIL_LOAD_SETTLE_BUDGET: Duration = Duration::from_secs(30);

/// Asserts that an ACP send to `bravo` does not succeed: either the relay
/// rejects the enqueue outright with `runtime_acp_worker_unavailable`, or it
/// accepts the async dispatch and the persistent worker settles `unavailable`.
///
/// Subscribes to the worker readiness channel BEFORE dispatching so the
/// terminal `Unavailable` transition cannot be missed if it fires before the
/// test code reaches the await. Uses [`ACP_FAIL_LOAD_SETTLE_BUDGET`] (30 s,
/// aligned with `ACP_OPERATION_TIMEOUT`) so the test outlasts any legitimate
/// path the production bootstrap can take; the test thread is parked on the
/// in-process watch channel, so the wider deadline costs nothing in the
/// happy case.
pub(in crate::acp) fn assert_acp_delivery_unavailable(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) {
    let root = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    let mut receiver = subscribe_bravo_worker_state(root);
    match dispatch_send_result(config_root, tmux_socket) {
        Err(error) => assert_eq!(error.code, "runtime_acp_worker_unavailable"),
        Ok(response) => {
            let _result = send_result(response);
            let started = Instant::now();
            // The level first, the transition second, and the order matters.
            //
            // A watch fires only on transitions, and this receiver is subscribed
            // after the worker may already have reached its terminal state — in
            // which case there is nothing further to observe and waiting on the
            // watch burns the whole budget before failing. Reading the registry
            // is not the weaker check: it reads the same published state, just
            // without requiring that we were listening when it changed.
            //
            // Waiting on the transition alone made this assertion depend on the
            // send being refused synchronously, which depended in turn on the
            // worker being Unavailable at that exact instant rather than
            // mid-respawn — a coincidence of timing, not a property.
            //
            // The watch is still needed for the other ordering, where the worker
            // is mid-respawn at this point and settles shortly after.
            let settled = read_worker_state(root, "bravo").as_deref() == Some("unavailable")
                || await_acp_worker_state(
                    &mut receiver,
                    WorkerReadinessState::Unavailable,
                    ACP_FAIL_LOAD_SETTLE_BUDGET,
                );
            if !settled {
                let elapsed = started.elapsed();
                let last_state_label = (*receiver.borrow())
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "None (no transitions published)".to_string());
                let poll_state = read_worker_state(root, "bravo")
                    .unwrap_or_else(|| "None (worker not registered)".to_string());
                panic!(
                    "ACP worker did not settle unavailable within {ACP_FAIL_LOAD_SETTLE_BUDGET:?} \
                     after a failed startup stage; observed wall time {elapsed:?}, last watch \
                     state = {last_state_label}, readiness poll state = {poll_state}"
                );
            }
        }
    }
}

/// Qualifies a bare target id with the fixture bundle namespace, mirroring the
/// client-side fill-in the relay now requires. Already-qualified ids pass
/// through unchanged.
pub(in crate::acp) fn read_request_log(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read ACP request log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("parse ACP request JSON line"))
        .collect()
}

pub(in crate::acp) fn request_by_method<'a>(requests: &'a [Value], method: &str) -> &'a Value {
    requests
        .iter()
        .find(|request| request.get("method").and_then(Value::as_str) == Some(method))
        .unwrap_or_else(|| panic!("missing ACP request for method '{method}'"))
}

pub(in crate::acp) fn request_count_by_method(requests: &[Value], method: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.get("method").and_then(Value::as_str) == Some(method))
        .count()
}
