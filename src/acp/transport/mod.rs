//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). [`Transport::mailw`] hands a structured delivery message
//! to the internal delivery task, which renders each message into pane-envelope
//! text, combines a contiguous group into one ACP turn under the token budget,
//! and resolves the future for each contributing task.
//!
//! The framed `session/prompt` write is the delivery boundary: every member of
//! the group resolves `Delivered` immediately when the write succeeds, before
//! replay-buffer locks or `on_dispatched` run. Active-prompt refusal and
//! serialization failure resolve `not_submitted`; a write or flush error that
//! cannot prove zero bytes left resolves `submission_unknown`. The turn's later
//! completion, permission requests, or connection close are target-health
//! observability — they drive readiness and the respawn signal, never a second
//! delivery outcome for an already-resolved member.
//!
//! Choices (tool-call permissions) resolve through the relay-injected
//! [`Chooser`] (see [`crate::acp::permission`]); the transport never calls the
//! relay choice queue directly. The `look` path reads output through the
//! [`OutputView`] handle published by [`Transport::give_output`].
//!
//! ## Readiness
//!
//! The transport owns an [`WorkerReadinessState`] signal for
//! [`is_ready_for_handover`] and the [`OutputView`] prime-wait, because it
//! cannot call relay's `set_worker_readiness`. The `AcpWorkerDriver` mirrors
//! transitions into the global worker-state registry (which external observers
//! and respawn/startup gating still read).
//!
//! Handover readiness is the narrow question: only `Available` qualifies, since
//! a `Busy` worker is mid-turn and cannot take another. The wider
//! "runtime exists" reading that `Busy` also satisfies is what the mirrored
//! registry state carries for those other observers.
//!
//! [`is_ready_for_handover`]: Transport::is_ready_for_handover

mod api;
mod delivery;
mod output;
mod state;
mod turn;

pub use api::AcpTransport;
#[doc(hidden)]
pub use api::WriteChannelGuard;
