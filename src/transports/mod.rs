//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. [`diagnostics`] houses the
//! transport-neutral delivery-progress inscription context. Concrete transport
//! implementations live in `src/acp/`, `src/tmux/`, and `src/pty/` when the
//! `pty` Cargo feature is enabled.
//!
//! The delivery and look vocabulary both call directions speak lives one level
//! up, in [`crate::protocol`], and is re-exported below so `crate::transports::`
//! paths keep resolving. It sits there rather than here because the relay depends
//! on it too, and a shared vocabulary owned by one side is not shared.

pub mod contract;
pub mod diagnostics;
pub mod ui;

pub use crate::acp::{AcpDriverServices, AcpTransport, AcpWorkerDriver};
pub use crate::protocol::{
    DeliveryPayloadMode, LookFreshness, LookSnapshotPayload, LookSnapshotSource, PackingUnitId,
    PartitionError, SendOutcome, StructuredEntry, SubmissionEvidence, ToolCallStatus,
    WorkerFailureReason, WorkerReadinessState,
};
#[cfg(feature = "pty")]
pub use crate::pty::{PtyTargetConfiguration, PtyTransport};
pub use crate::tmux::TmuxTransport;
pub(crate) use contract::stopped_before_submission_outcome;
pub use contract::{
    ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryExecutorContext, DeliveryMessage,
    DeliveryWriter, GenerationFence, LookMode, MailboxConsumer, OutputView, PeekDimensions,
    PlannedWrite, PromptReadinessTemplate, SingleDeliveryOutcome, StartupContext, ThingToChoose,
    Transport, TransportError, TransportHealth, TransportImpl, TransportReadiness, TransportStatus,
    UnreachableSince, receipt_runs, run_delivery_executor,
};
pub use diagnostics::{
    DIAGNOSTIC_MESSAGE_IDS_MAXIMUM, DeliveryDiagnosticContext, emit_delivery_progress,
};
pub use ui::{
    UiBroadcastFn, UiBroadcastStatus, UiIncomingMessage, UiOutcomePhase, UiPhaseFn, UiTransport,
    UiTransportServices,
};
