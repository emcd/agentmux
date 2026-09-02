//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay delivery worker dispatches every agent delivery operation through
//! the [`Transport`] trait. Concrete transports (ACP, Tmux, UI) each implement
//! the trait in their own module; the relay selects between them via the
//! [`TransportImpl`] enum, which delegates by `match` with no dynamic
//! allocation. Promoting UI (and the forward-declared Pubsub) to first-class
//! transports retires the relay's former `Acp/Tmux/Ui/Pubsub` routing fork.
//!
//! ## Write boundary: non-blocking, future-resolved
//!
//! The write methods ([`Transport::mailw`] for relay-framed envelopes,
//! [`Transport::raww`] for raw input) do not block. Each enqueues the write onto
//! the transport's own internal ordered channel and returns an [`OutcomeFuture`]
//! that resolves when the transport's internal delivery task drives that write
//! to a terminal [`SingleDeliveryOutcome`]. The transport owns that task, its
//! `spawn_blocking`, and any transport-local batching; the relay worker
//! concurrently submits new writes and collects resolved futures without
//! blocking on any single one.
//!
//! This retires the earlier "the sync core never crosses `.await`; the worker
//! owns `spawn_blocking`" invariant: ownership of the blocking delivery moves
//! into each transport. The legacy synchronous `deliver`/`prepare_delivery`/
//! `raw_write` seam has been removed now that every relay callsite delivers
//! through the write methods.
//!
//! ## Transport <-> relay interactions
//!
//! There is no generic inbound event channel. Each transport->relay interaction
//! uses its natural primitive:
//!
//! - **Choices** (tool-call permissions, and any future operator decision) are
//!   blocking requests: the relay injects a re-entrant [`Chooser`] via
//!   [`StartupContext`], which the transport invokes inline and blocks on until
//!   the operator decides. No transport->relay back-edge: the transport holds an
//!   opaque `Arc<dyn Fn>` typed here in `transports`.
//! - **Completion** resolves through the [`OutcomeFuture`] returned by
//!   [`Transport::mailw`]/[`Transport::raww`]: the transport's internal delivery
//!   task drives each write to a terminal [`SingleDeliveryOutcome`], and the
//!   worker fans out from the resolved future.
//! - **Output for `look`** is a concurrent read via [`Transport::give_output`],
//!   which hands the relay an [`OutputView`] handle the look request path can
//!   read without borrowing the worker-owned transport.
//!
//! ## Status
//!
//! Complete (`decouple-transport-layer`): the ACP transport lives in
//! `crate::acp` (driven by the `AcpWorkerDriver` lifecycle behind
//! [`TransportImpl::Acp`]) and the tmux transport in `crate::tmux`. The relay
//! delivery worker holds a [`TransportImpl`] per target and dispatches every
//! agent delivery through it.
//!
//! ## Layout
//!
//! This module is an import-only hub: every definition lives in a child, and
//! what appears here is the `mod` declarations plus the re-exports that keep
//! the contract addressable as one surface. `transport` holds the traits,
//! `dispatch` the [`TransportImpl`] enum, `delivery` the envelope and outcome
//! types, `choices` the operator-choice resolver and startup context, and
//! `status` the transport's self-report and look-window types.
//!
//! The children are private deliberately: they are an internal division of one
//! contract, not a second addressable surface. Every path that resolved at
//! `transports::contract::*` before the split still resolves through these
//! re-exports, and callers reaching these through `crate::transports` are
//! unaffected either way.

mod choices;
mod delivery;
mod dispatch;
mod executor;
mod status;
mod transport;

// Re-export the configuration prompt-readiness template into the transport
// contract namespace; the Tmux prompt probe consumes it and the transport wires
// it through the delivery context. It is defined once in `configuration`;
// re-exporting (rather than redefining) keeps the two in lockstep.
pub use crate::configuration::PromptReadinessTemplate;

pub use choices::{ChoiceMade, ChoiceToMake, Chooser, StartupContext, ThingToChoose};
pub(crate) use delivery::stopped_before_submission_outcome;
pub use delivery::{DeliveryEnvelope, DeliveryMessage, OutcomeFuture, SingleDeliveryOutcome};
pub use dispatch::{HandoverDimensions, TransportImpl};
pub use executor::{
    DeliveryExecutorContext, DeliveryWriter, MailboxConsumer, PlannedWrite, run_delivery_executor,
};
pub use status::{LookMode, TransportError, TransportReadiness, TransportStatus};
pub use transport::{
    GenerationFence, OutputView, PartitionSink, Transport, TransportHealth, UnreachableSince,
};
