//! The per-target async delivery worker, split along its own lifecycle:
//!
//! - [`spawn`] — what a worker is built from, and building one generation.
//! - [`run`] — the produce-and-collect loop that drives a generation.
//! - [`stop`] — ending a generation, bounded by the same fence the watchdog uses.
//! - [`gate`] — whether a target will take a handover right now.
//! - [`intake`] — building a task's payload and placing it in its mailbox.
//! - [`submit`] — forming a batch, authorizing it whole, and writing it.
//!
//! The dependency runs outward from [`run`]: it owns the loop and calls into
//! [`intake`] as each task arrives, into [`gate`] and [`submit`] for the two
//! decisions a delivery turns on, into [`stop`] for every ending, and back into
//! [`spawn`] for a replacement generation after a positive fence verdict.
//! Nothing calls [`run`] but [`spawn`], which is where the tokio task is
//! created.
//!
//! The five items other `dispatch` modules consume are re-exported here at the
//! visibility they carried when this was one file. They are declared
//! `pub(in crate::relay::delivery::dispatch)` at their definition sites rather
//! than `pub(super)`: an item is not re-exportable above its own declared
//! visibility, so a `pub(super)` in a grandchild cannot be published to a
//! sibling of the parent.

mod gate;
mod intake;
mod run;
mod spawn;
mod stop;
mod submit;

pub(super) use self::spawn::{
    AcpWorkerBootstrap, InflightMember, WorkerTransportContext, WorkerTransportSource,
    spawn_async_delivery_worker,
};
