mod acp_client;
mod acp_delivery;
mod acp_state;
mod async_worker;
mod choice_state;
mod dispatch;
pub(in crate::relay) mod observability;
mod quiescence;
mod results;
mod ui_delivery;

pub(in crate::relay) use self::acp_state::derive_acp_look_snapshot;
pub use self::async_worker::AcpWorkerReadinessState;
pub(in crate::relay) use self::async_worker::{
    acp_session_ready_for_startup, get_acp_worker_snapshot, get_acp_worker_state,
};
pub use self::choice_state::install_pending_choice_request_for_testing;
pub(in crate::relay) use self::choice_state::{
    ChoiceDecisionKind, ChoiceDecisionRequest, ChoiceEventContext, ChoiceResolutionOutcome,
    PendingChoiceRequest, emit_choices_snapshot_then_replay, enqueue_choice_request,
    list_pending_choice_requests, resolve_choice_request, wait_for_choice_resolution,
};
pub(in crate::relay) use self::dispatch::{
    await_acp_worker_prime_for_look, deliver_one_target, enqueue_async_delivery,
    enqueue_sync_delivery, initialize_acp_target_for_startup, prompt_batch_settings,
    wait_for_async_delivery_shutdown,
};
pub(in crate::relay) use self::quiescence::QuiescenceOptions;
