//! The per-target async delivery worker, split along its own lifecycle:
//!
//! - [`spawn`] — what a worker is built from, and building one generation.
//! - [`run`] — the loop that drives a generation and fills its mailbox.
//! - [`stop`] — ending a generation, bounded by the same fence the watchdog uses.
//! - [`intake`] — building a task's payload and placing it in its mailbox.
//!
//! The dependency runs outward from [`run`]: it owns the loop and calls into
//! [`intake`] as each task arrives, into [`stop`] for every ending, and back
//! into [`spawn`] for a replacement generation after a positive fence verdict.
//! Nothing calls [`run`] but [`spawn`], which is where the tokio task is
//! created.
//!
//! **There is no gate module and no submit module**, and their absence is the
//! shape of the pull model at this seam. A gate decided whether a target would
//! take a handover, and a submit formed a batch and wrote it; the relay makes
//! neither decision now. Readiness is judged inside the owning transport's
//! delivery-loop executor, which is also what writes — so what is left here is
//! custody and supervision, and both of those are the relay's alone.
//!
//! The items other `dispatch` modules consume are re-exported here at the
//! visibility they carried when this was one file. They are declared
//! `pub(in crate::relay::delivery::dispatch)` at their definition sites rather
//! than `pub(super)`: an item is not re-exportable above its own declared
//! visibility, so a `pub(super)` in a grandchild cannot be published to a
//! sibling of the parent.

mod intake;
mod run;
mod spawn;
mod stop;

pub(super) use self::spawn::{
    AcpWorkerBootstrap, WorkerTransportContext, WorkerTransportSource, spawn_async_delivery_worker,
};
