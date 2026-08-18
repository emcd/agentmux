//! Async delivery workers: the registry that owns a target, the terminal
//! transition that resolves one member, and the reporting that publishes it.
//!
//! Split three ways along the boundary the module's own tests already followed:
//!
//! - [`registry`] — which worker owns a target and what state it is in.
//! - [`terminal`] — the single terminal transition and the evidence order.
//! - [`reporting`] — the observability floor, the sender's event, the receipt.
//!
//! The dependency runs one way through the middle: `terminal` decides an outcome
//! and hands it to `reporting` to publish, and `reporting` reaches back into
//! `registry` only to route a receipt to a live worker.

mod registry;
mod reporting;
mod terminal;

pub(in crate::relay::delivery) use self::registry::{
    AsyncWorkerKey, WorkerDispatch, WorkerOwner, build_worker_key, close_worker,
    drop_pending_async_tasks_on_shutdown, mark_worker_fail_stopped, register_worker_if_absent,
    release_pending_slot, task_uses_acp_transport, try_existing_worker, unregister_worker,
    wait_for_async_delivery_shutdown, worker_exists,
};
pub(in crate::relay) use self::registry::{
    acp_readiness_is_ready, acp_session_is_ready, get_output_view, get_worker_failure,
    get_worker_readiness, install_acp_worker_output_view, set_worker_failure, set_worker_readiness,
};

/// Test-only registry seam: installs an entry directly, so a test can make a
/// worker key routable without spawning a real worker. Re-exported under the same
/// `cfg` as its definition rather than made unconditionally visible.
#[cfg(test)]
pub(in crate::relay::delivery) use self::registry::register_worker;

pub(in crate::relay::delivery) use self::terminal::{
    complete_task_on_shutdown, complete_task_outcome, complete_task_outcome_from_trigger,
    complete_task_refusal,
};
