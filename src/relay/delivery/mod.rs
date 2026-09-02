pub(crate) mod admission;
pub(crate) mod async_worker;
mod choice_state;
// The relay side of the pull model's consumer seam. It is complete and reached
// only by its own construction until the cutover hands each transport an
// executor to drive it; scoped to the module so removing this is one edit at
// that point, exactly as the same allow on `admission::mailbox` is.
#[allow(dead_code)]
mod consumer;
mod dispatch;
pub(in crate::relay) mod fence;
pub(in crate::relay) mod guard;
pub(in crate::relay) mod observability;
mod partition;

pub(in crate::relay) use self::async_worker::{
    acp_session_is_ready, get_output_view, get_worker_readiness, stop_workers_for_bundle,
    wait_for_bundle_workers_stopped,
};
pub use self::choice_state::install_pending_choice_request_for_testing;
pub(in crate::relay) use self::choice_state::{
    ChoiceDecisionKind, ChoiceDecisionRequest, ChoiceEventContext, ChoiceResolutionOutcome,
    PendingChoiceRequest, emit_choices_snapshot_then_replay, list_pending_choice_requests,
    resolve_choice_request,
};
#[allow(unused_imports)]
pub(in crate::relay) use self::consumer::LedgerMailboxConsumer;
pub(in crate::relay) use self::dispatch::{
    enqueue_async_delivery, initialize_acp_target_for_startup, wait_for_async_delivery_shutdown,
};
