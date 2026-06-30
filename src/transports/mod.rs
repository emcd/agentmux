//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. Concrete transport implementations land
//! in `src/acp/` (Slice 2) and `src/tmux/` (Slice 3).

pub mod contract;
pub mod ui;
pub mod vocabulary;

pub use crate::acp::{AcpDriverServices, AcpTransport, AcpWorkerDriver};
pub use crate::tmux::TmuxTransport;
pub use contract::{
    ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryMessage, DeliveryWaitError,
    LookMode, OutcomeFuture, OutputView, PromptReadinessTemplate, SingleDeliveryOutcome,
    StartupContext, ThingToChoose, Transport, TransportError, TransportImpl, TransportReadiness,
    TransportStatus,
};
pub use ui::{
    UiBroadcastFn, UiBroadcastStatus, UiIncomingMessage, UiOutcomePhase, UiPhaseFn, UiTransport,
    UiTransportServices,
};
pub use vocabulary::{
    DeliveryPayloadMode, LookFreshness, LookSnapshotPayload, LookSnapshotSource, SendOutcome,
    StructuredEntry, ToolCallStatus, WorkerReadinessState,
};
