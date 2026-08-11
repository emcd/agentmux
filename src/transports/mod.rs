//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. [`diagnostics`] houses the
//! transport-neutral delivery-progress inscription context. Concrete transport
//! implementations live in `src/acp/`, `src/tmux/`, and `src/pty/` when the
//! `pty` Cargo feature is enabled.

pub mod contract;
pub mod diagnostics;
pub mod ui;
pub mod vocabulary;

pub use crate::acp::{AcpDriverServices, AcpTransport, AcpWorkerDriver};
#[cfg(feature = "pty")]
pub use crate::pty::{PtyTargetConfiguration, PtyTransport};
pub use crate::tmux::TmuxTransport;
pub use contract::{
    ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryMessage, GenerationFence,
    HandoverDimensions, LookMode, OutcomeFuture, OutputView, PartitionSink,
    PromptReadinessTemplate, SingleDeliveryOutcome, StartupContext, ThingToChoose, Transport,
    TransportError, TransportHealth, TransportImpl, TransportReadiness, TransportStatus,
    UnreachableSince,
};
pub use diagnostics::{
    DIAGNOSTIC_MESSAGE_IDS_MAXIMUM, DeliveryDiagnosticContext, emit_delivery_progress,
};
pub use ui::{
    UiBroadcastFn, UiBroadcastStatus, UiIncomingMessage, UiOutcomePhase, UiPhaseFn, UiTransport,
    UiTransportServices,
};
pub use vocabulary::{
    DeliveryPayloadMode, LookFreshness, LookSnapshotPayload, LookSnapshotSource, PackingUnitId,
    PartitionError, SendOutcome, StructuredEntry, SubmissionEvidence, ToolCallStatus,
    WorkerFailureReason, WorkerReadinessState,
};
