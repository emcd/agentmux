pub(crate) mod admission;
mod async_worker;
mod choice_state;
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
pub(in crate::relay) use self::dispatch::{
    enqueue_async_delivery, initialize_acp_target_for_startup, wait_for_async_delivery_shutdown,
};
