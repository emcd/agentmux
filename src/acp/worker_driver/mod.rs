//! The ACP worker lifecycle driver.
//!
//! [`AcpWorkerDriver`] owns the per-target [`AcpTransport`] and its
//! bootstrap/respawn lifecycle. It is held by `TransportImpl::Acp`, so the relay
//! delivery worker drives ACP startup and recovery through the generic transport
//! handle without naming any ACP type. The driver depends only downward on
//! `crate::transports`, `crate::configuration`, and `crate::runtime` — never on
//! `crate::relay`.
//!
//! ## Relay touchpoints as injected closures
//!
//! The lifecycle reaches relay-owned registries (worker-state mirror, look
//! OutputView publish, choice-queue invalidation, UI stream broadcast) and the
//! relay choice queue (the [`Chooser`]). Each is injected as an opaque
//! `Arc<dyn Fn>` (or value) in [`AcpDriverServices`], constructed relay-side
//! closing over relay services; the driver invokes them without a back-edge,
//! mirroring the `Chooser` pattern from Slice 2b.

mod driver;
mod respawn;
mod services;

pub use driver::AcpWorkerDriver;
pub use services::{
    AcpDriverServices, BroadcastUiFn, InvalidateChoicesFn, MirrorStateFn, PublishOutputFn,
    RecordFailureFn,
};
