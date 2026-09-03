//! ACP delivery as a [`Transport`] implementation.
//!
//! `AcpTransport` owns the per-target `PersistentAcpWorkerRuntime` (moved here
//! from the relay delivery worker, which previously threaded it through
//! `spawn_blocking`). Its delivery-loop executor peeks its target's mailbox,
//! renders each entry into pane-envelope text, combines a contiguous run into
//! one ACP turn under the token budget, declares that run, and acknowledges what
//! the turn's framed write proved for each member of it.
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
//! The transport owns a [`WorkerReadinessState`] signal for its delivery
//! executor's own readiness check and the [`OutputView`] prime-wait, because it
//! cannot call relay's `set_worker_readiness`. The `AcpWorkerDriver` mirrors
//! transitions into the global worker-state registry (which external observers
//! and respawn/startup gating still read).
//!
//! **The relay reads no readiness level from this transport.** Whether a turn may
//! be submitted now is asked inside the executor, where it is the narrow
//! question: only `Available` qualifies, since a `Busy` worker is mid-turn and
//! cannot take another. The wider "runtime exists" reading that `Busy` also
//! satisfies is what the mirrored registry state carries for those other
//! observers, and it is the only one that leaves this crate.

mod api;
mod delivery;
mod output;
mod state;
mod turn;

pub use api::AcpTransport;
pub use delivery::AcpReachability;
