//! Async delivery workers: the registry that owns a target, the terminal
//! transition that resolves one member, and the reporting that publishes it.
//!
//! Split four ways along the boundary the module's own tests already followed:
//!
//! - [`registry`] — which worker owns a target and what state it is in.
//! - [`attempt`] — one construction attempt's completion and its waiters.
//! - [`reap`] — a registration ending, and the target state it takes with it.
//! - [`terminal`] — the single terminal transition and the evidence order.
//! - [`reporting`] — the observability floor, the sender's event, the receipt.
//!
//! The dependency runs one way through the middle: `terminal` decides an outcome
//! and hands it to `reporting` to publish, and `reporting` reaches back into
//! `registry` only to route a receipt to a live worker. `reap` sits beside
//! `registry` rather than inside it because it is the one operation here that
//! reaches past the registry into the admission ledger — an entry leaving is
//! what makes a target's mailbox nobody's.

#[cfg(feature = "pty")]
mod attempt;
mod reap;
pub(crate) mod registry;
mod reporting;
mod terminal;

/// Entry-owned construction completions for the bring-up prime wait: only
/// Pty records into them today (the one transport whose worker unregisters
/// on construction failure). Each attempt carries its own completion in its
/// own entry — there is no table to purge — so the handles and their
/// touchpoints are gated with it rather than compiled dead into every
/// build.
#[cfg(feature = "pty")]
pub(in crate::relay::delivery) use self::attempt::{
    PtyElection, adopt_registered_attempt, elect_pty_attempt,
    record_construction_error, worker_attempt_snapshot,
};
pub(in crate::relay::delivery) use self::reap::{
    bind_worker_consumer_generation, unregister_worker,
};
#[cfg(feature = "pty")]
pub(in crate::relay) use self::registry::set_worker_readiness_if_owner;
pub(in crate::relay::delivery) use self::registry::{
    AsyncDeliveryRegistry, AsyncWorkerKey, WorkerDispatch, WorkerOwner,
    build_worker_key, close_worker, drop_pending_async_tasks_on_stop,
    mark_worker_fail_stopped, register_worker_if_absent, release_pending_slot,
    task_uses_acp_transport, try_existing_worker,
    wait_for_async_delivery_shutdown, worker_exists, worker_stop_requested,
};
pub(in crate::relay) use self::registry::{
    acp_readiness_is_ready, acp_session_is_ready, get_output_view,
    get_worker_failure, get_worker_readiness, install_acp_worker_output_view,
    set_worker_failure, set_worker_readiness, stop_workers_for_bundle,
    wait_for_bundle_workers_stopped,
};

/// Test-only registry seam: installs an entry directly, so a test can make a
/// worker key routable without spawning a real worker. The `_with_readiness`
/// variant names the entry's readiness in the same insert. Re-exported under
/// the same `cfg` as their definitions rather than made unconditionally
/// visible.
#[cfg(test)]
pub(in crate::relay::delivery) use self::registry::register_worker;
#[cfg(all(test, feature = "pty"))]
pub(in crate::relay::delivery) use self::registry::register_worker_with_readiness;

pub(in crate::relay::delivery) use self::terminal::{
    complete_task_on_shutdown, complete_task_outcome,
    complete_task_outcome_from_trigger, complete_task_refusal,
    report_resolved_member, report_resolved_outcome,
};
