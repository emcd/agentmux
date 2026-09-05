//! Relay-owned admission control for the async delivery queue.
//!
//! Admission is the first of the four delivery events. It runs at the request
//! boundary, before `queued` is returned, and it is the only place a send is
//! refused for capacity: once an entry is admitted the relay waits for its target
//! indefinitely rather than resolving it on a clock, so the queue's growth has to
//! be bounded here or nowhere.
//!
//! Three refusals live here, and each rejects at the request boundary rather than
//! queueing something that cannot progress:
//!
//! - **Quota** — an entry reserves envelope count and canonical payload bytes
//!   against both a per-target and a relay-global limit. The reservation is
//!   atomic across both: a single lock covers the check and the increment, so two
//!   concurrent sends cannot both observe headroom that only one of them can have.
//! - **Peek dimensions** — an envelope whose canonical payload alone exceeds
//!   what its transport will ever accept is rejected, because queueing it would
//!   park a message no packing unit could carry.
//! - **Pubsub** — a forward-declared stub with no delivery path is refused
//!   synchronously, so no work is authorized merely to discover it.
//!
//! Reserved quota is released at terminalization and nowhere else. Release is
//! keyed on the entry's message id and is idempotent: an id the ledger never
//! admitted (a relay-originated terminal-outcome receipt, which bypasses
//! admission because nothing accepted it) releases nothing, and a second release
//! of the same entry is a no-op.
//!
//! Split along the ledger's own lifecycle:
//!
//! - [`config`] — the published `[delivery]` table and the quota bounds drawn
//!   from it.
//! - [`ledger`] — the process-global state every event below mutates, and the one
//!   lock that guards it.
//! - [`admit`] — the reservation, its three refusals, and its rollback.
//! - [`mailbox`] — the per-target ordered mailbox: the enqueue that fills an
//!   admitted entry's position, the three operations a delivery-loop executor
//!   calls against it (peek, declare, acknowledge), who is entitled to call
//!   them, the doorbell that tells a consumer a peek is worth making, and the
//!   reap that reclaims the whole thing when a target's registration goes away.
//! - [`authorize`] — the all-or-none batch transition, packing units, evidence.
//! - [`terminal`] — the single terminal transition, which is also the only quota
//!   release.
//! - [`reporting`] — what the relay *says* about the queue; it resolves nothing.
//!
//! Every locking path takes [`ledger::lock_ledger`] exactly once, at the head of
//! its entry point, and holds that one guard for the whole operation. The lock is
//! a `std::sync::Mutex` and therefore not reentrant, so a helper that acquired it
//! a second time inside a held section would deadlock rather than merely split
//! the operation. No module below may add a locking helper of its own.

mod admit;
mod config;
mod ledger;
// The one interleaving this subsystem's serialization rests on is unobservable
// from outside the lock, so the boundary reports it from inside. Test-only and
// absent from the shipped binary; see the module for why a seam is warranted
// here and why it is not the production-shaped kind this project declines.
#[cfg(test)]
mod lock_boundary;
mod mailbox;
mod reporting;
mod terminal;

pub use self::config::configure_delivery;
pub use self::reporting::{
    UndeliveredReporting, configured_undelivered_reporting, report_undelivered_queue,
};

pub(in crate::relay) use self::admit::{
    admit, canonical_payload_bytes, resolve_target_session_type, rollback_admission,
    target_is_relay_wide,
};
pub(in crate::relay) use self::config::delivery_configuration;
pub(in crate::relay) use self::ledger::AdmissionTargetKey;
// `TargetReap` names a reap's answer, which only the test that holds the reap
// to its contract inspects: production reads the reap for its effect, and names
// the type through `reap_target`'s own signature rather than importing it.
#[cfg(test)]
pub(in crate::relay) use self::mailbox::TargetReap;
pub(in crate::relay) use self::mailbox::{
    Acknowledgment, EnqueueRejection, GenerationRejection, ResolvedMember, ack,
    claim_consumer_generation, declaration_age, declare, enqueue, peek, reap_target,
    register_doorbell, replace_consumer_generation, resolve_target_entries,
};
pub(in crate::relay) use self::terminal::{TerminalTransition, terminalize};
