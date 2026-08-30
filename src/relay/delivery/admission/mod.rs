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
//! - **Handover dimensions** — an envelope whose canonical payload alone exceeds
//!   what its transport will ever accept is rejected, because queueing it would
//!   park a message no partition could carry.
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
//! - [`mailbox`] — the per-target ordered mailbox and the three operations a
//!   delivery-loop executor calls against it: peek, declare, acknowledge.
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
mod authorize;
mod config;
mod ledger;
// The pull model's relay side lands before the transports that drive it: the
// delivery-loop executors that peek, declare, and acknowledge are wired when the
// push-model handover is removed, so until then nothing outside this module's own
// tests calls any of it. Scoped to this module rather than to each operation, so
// that removing it is one edit at the point the executors arrive.
#[allow(dead_code, unused_imports)]
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
pub(in crate::relay) use self::authorize::{
    authorize_batch, declare_packing_unit, record_evidence_for_member, record_unit_evidence,
};
pub(in crate::relay) use self::config::delivery_configuration;
pub(in crate::relay) use self::ledger::AdmissionTargetKey;
pub(in crate::relay) use self::terminal::{TerminalTransition, terminalize};
