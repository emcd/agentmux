//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay manages every transport's lifecycle through the [`Transport`]
//! trait. Concrete transports (ACP, Tmux, UI) each implement it in their own
//! module; the relay selects between them via the [`TransportImpl`] enum, which
//! delegates by `match` with no dynamic allocation. Promoting UI (and the
//! forward-declared Pubsub) to first-class transports retires the relay's former
//! `Acp/Tmux/Ui/Pubsub` routing fork.
//!
//! ## The trait carries no write
//!
//! Nothing is handed to a transport to deliver. Each transport owns one serial
//! [`DeliveryWriter`] driven by [`run_delivery_executor`], spawned during
//! [`Transport::startup`], which asks the relay's mailbox what is waiting for its
//! target and reports back what it did with it. The trait is therefore about
//! lifecycle — start, health, teardown, and the `look` handle — and the delivery
//! seam is [`MailboxConsumer`] plus [`DeliveryWriter`], not a method here.
//!
//! That is what retired the earlier "the sync core never crosses `.await`; the
//! worker owns `spawn_blocking`" invariant, and then the non-blocking
//! write-and-collect seam that replaced it. Ownership of the blocking delivery
//! moved into each transport, and then the decision of *when* to write moved with
//! it.
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
//! - **Completion** is reported through [`MailboxConsumer::ack`]: the executor
//!   that wrote a declared unit reports one evidence per member, and the relay
//!   terminalizes each of them from its own report.
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
pub use delivery::{DeliveryEnvelope, DeliveryMessage, SingleDeliveryOutcome};
pub use dispatch::{PeekDimensions, TransportImpl};
pub use executor::{
    DeliveryExecutorContext, DeliveryWriter, MailboxConsumer, PlannedWrite, receipt_runs,
    run_delivery_executor,
};
pub use status::{LookMode, TransportError, TransportReadiness, TransportStatus};
pub use transport::{GenerationFence, OutputView, Transport, TransportHealth, UnreachableSince};
