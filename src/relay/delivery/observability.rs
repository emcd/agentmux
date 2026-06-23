//! In-process observability surface for ACP worker state and choices queue
//! mutations.
//!
//! These channels publish the same transitions that the relay already emits as
//! inscriptions, but deliver them synchronously to in-process subscribers. They
//! exist so callers (tests, the TUI, and future embedders) can wait for a
//! state change deterministically rather than polling a filesystem snapshot.
//!
//! Publishers are owned by global registries that are independent of the
//! per-worker registration HashMap. A subscriber can therefore call
//! [`subscribe_acp_worker_state`] before the worker exists, and continue to
//! receive transitions after the worker entry is unregistered during shutdown.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use tokio::sync::{broadcast, watch};

use super::async_worker::{AsyncWorkerKey, build_worker_key};
use crate::transports::AcpWorkerReadinessState;

const CHOICES_QUEUE_BROADCAST_CAPACITY: usize = 64;

/// Mutation events emitted by the relay's choices queue.
///
/// `Resolved` covers operator decisions (selected or cancelled by UI).
/// `Invalidated` covers system-driven removals (ACP worker respawn invalidates
/// pending records; relay shutdown cancels them). `reason_code` distinguishes
/// those cases and matches the persisted reason codes used elsewhere.
#[derive(Clone, Debug)]
pub enum ChoicesQueueEvent {
    Enqueued {
        choice_request_id: String,
        message_id: String,
        target_session: String,
    },
    Resolved {
        choice_request_id: String,
        target_session: String,
    },
    Invalidated {
        choice_request_id: String,
        target_session: String,
        reason_code: String,
    },
}

static WORKER_STATE_PUBLISHERS: OnceLock<
    Mutex<HashMap<AsyncWorkerKey, watch::Sender<Option<AcpWorkerReadinessState>>>>,
> = OnceLock::new();

static CHOICES_QUEUE_PUBLISHERS: OnceLock<
    Mutex<HashMap<PathBuf, broadcast::Sender<ChoicesQueueEvent>>>,
> = OnceLock::new();

fn worker_state_publishers()
-> &'static Mutex<HashMap<AsyncWorkerKey, watch::Sender<Option<AcpWorkerReadinessState>>>> {
    WORKER_STATE_PUBLISHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn choices_queue_publishers()
-> &'static Mutex<HashMap<PathBuf, broadcast::Sender<ChoicesQueueEvent>>> {
    CHOICES_QUEUE_PUBLISHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Subscribes to readiness-state transitions for a persistent ACP worker.
///
/// The returned receiver yields the current value immediately (None if no
/// transition has been published yet) and then every subsequent transition.
/// Callers typically pair this with [`tokio::sync::watch::Receiver::changed`]
/// and read [`tokio::sync::watch::Receiver::borrow`].
pub fn subscribe_acp_worker_state(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> watch::Receiver<Option<AcpWorkerReadinessState>> {
    let key = build_worker_key(bundle_name, runtime_directory, target_session);
    let mut publishers = worker_state_publishers()
        .lock()
        .expect("worker state publishers mutex poisoned");
    publishers
        .entry(key)
        .or_insert_with(|| watch::channel(None).0)
        .subscribe()
}

/// Subscribes to mutation events on the choices queue for one runtime
/// directory.
///
/// The receiver only observes events that arrive after the subscription is
/// established. Slow consumers see [`tokio::sync::broadcast::error::RecvError::Lagged`]
/// and must catch up via [`crate::relay::list_pending_choice_requests`]
/// before resuming live consumption.
pub fn subscribe_choices_queue_events(
    runtime_directory: &Path,
) -> broadcast::Receiver<ChoicesQueueEvent> {
    let mut publishers = choices_queue_publishers()
        .lock()
        .expect("choices queue publishers mutex poisoned");
    publishers
        .entry(runtime_directory.to_path_buf())
        .or_insert_with(|| broadcast::channel(CHOICES_QUEUE_BROADCAST_CAPACITY).0)
        .subscribe()
}

pub(super) fn publish_acp_worker_state(key: &AsyncWorkerKey, state: AcpWorkerReadinessState) {
    let mut publishers = worker_state_publishers()
        .lock()
        .expect("worker state publishers mutex poisoned");
    let sender = publishers
        .entry(key.clone())
        .or_insert_with(|| watch::channel(None).0);
    // send returns Err only when there are no live receivers, which is fine.
    let _ = sender.send(Some(state));
}

pub(super) fn publish_choices_queue_event(runtime_directory: &Path, event: ChoicesQueueEvent) {
    let mut publishers = choices_queue_publishers()
        .lock()
        .expect("choices queue publishers mutex poisoned");
    let sender = publishers
        .entry(runtime_directory.to_path_buf())
        .or_insert_with(|| broadcast::channel(CHOICES_QUEUE_BROADCAST_CAPACITY).0);
    // send returns Err only when there are no live receivers, which is fine.
    let _ = sender.send(event);
}

// The publishers are crate-private (`pub(super)`) by design: they are the
// producer side of the observer pattern, called by in-process mutators. They
// must not become part of the relay's public surface, so exercising them
// directly from an external test crate would require a doc(hidden) escape
// hatch that would itself become unintended API. The end-to-end paths through
// `set_acp_worker_state` / `enqueue_choice_request` are covered by the
// ACP integration tests; this inline test only proves the publish/subscribe
// primitive itself wires watch and broadcast correctly when the publishers
// are invoked. One `#[test]` covers both channels because their setup is
// shared.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test(flavor = "current_thread")]
    async fn publishers_deliver_state_and_queue_events_to_subscribers() {
        let runtime_directory = PathBuf::from("/tmp/agentmux-observability-test-fake-runtime");
        let bundle_name = "obstest_bundle";
        let target_session = "obstest_target";

        // Worker-state watch: subscribe first, then publish, then await the
        // change notification and read the borrowed value.
        let mut state_receiver =
            subscribe_acp_worker_state(bundle_name, runtime_directory.as_path(), target_session);
        assert_eq!(*state_receiver.borrow_and_update(), None);
        let key = build_worker_key(bundle_name, runtime_directory.as_path(), target_session);
        publish_acp_worker_state(&key, AcpWorkerReadinessState::Unavailable);
        state_receiver
            .changed()
            .await
            .expect("watch sender outlives subscriber");
        assert_eq!(
            *state_receiver.borrow(),
            Some(AcpWorkerReadinessState::Unavailable)
        );

        // Choices-queue broadcast: subscribe first, publish two events,
        // then assert both arrive in order with their payload preserved.
        let mut queue_receiver = subscribe_choices_queue_events(runtime_directory.as_path());
        publish_choices_queue_event(
            runtime_directory.as_path(),
            ChoicesQueueEvent::Enqueued {
                choice_request_id: "perm-1".to_string(),
                message_id: "msg-1".to_string(),
                target_session: target_session.to_string(),
            },
        );
        publish_choices_queue_event(
            runtime_directory.as_path(),
            ChoicesQueueEvent::Invalidated {
                choice_request_id: "perm-1".to_string(),
                target_session: target_session.to_string(),
                reason_code: "runtime_choices_request_invalidated_by_respawn".to_string(),
            },
        );
        match queue_receiver.recv().await.expect("first event") {
            ChoicesQueueEvent::Enqueued {
                choice_request_id,
                message_id,
                target_session: actual_target,
            } => {
                assert_eq!(choice_request_id, "perm-1");
                assert_eq!(message_id, "msg-1");
                assert_eq!(actual_target, target_session);
            }
            other => panic!("expected Enqueued, got {other:?}"),
        }
        match queue_receiver.recv().await.expect("second event") {
            ChoicesQueueEvent::Invalidated {
                choice_request_id,
                target_session: actual_target,
                reason_code,
            } => {
                assert_eq!(choice_request_id, "perm-1");
                assert_eq!(actual_target, target_session);
                assert_eq!(
                    reason_code,
                    "runtime_choices_request_invalidated_by_respawn"
                );
            }
            other => panic!("expected Invalidated, got {other:?}"),
        }
    }
}
