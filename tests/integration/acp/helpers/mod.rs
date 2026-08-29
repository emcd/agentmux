//! Helpers for `acp::*` integration tests (split by helper ownership).
//!
//! `tests/integration/acp/helpers.rs` (1016 lines) is split into private
//! submodules by helper ownership; this facade re-exports the helpers
//! integration surface and owns no helpers itself.

mod dispatch;
mod guard;
mod observation;
mod state;
mod stub;

pub(super) use dispatch::{
    dispatch_look, dispatch_look_with_offset, dispatch_look_without_startup, dispatch_raww,
    dispatch_send, dispatch_send_result, dispatch_send_without_startup_result,
    dispatch_sized_send_result,
};
pub use guard::GuardedTempDir;
pub(super) use observation::{
    assert_acp_delivery_unavailable, await_acp_worker_state, await_permission_event,
    read_request_log, request_by_method, request_count_by_method, subscribe_bravo_permission_queue,
    subscribe_bravo_worker_state, wait_for_worker_state,
};
pub(super) use state::{flat_bundle_paths, persisted_state_path, read_worker_state, send_result};

// Shared re-exports that original `helpers.rs` exposed to siblings via `super::helpers::*`
pub(super) use agentmux::relay::ChoicesQueueEvent;
pub(super) use agentmux::transports::WorkerReadinessState;

pub(super) use stub::{AcpStubOptions, acp_child_pid_path, write_acp_stub, write_configuration};
