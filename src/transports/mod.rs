//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. [`quiescence`] houses the
//! cross-transport wedge/prime quiescence state machine consumed by every
//! coder transport. Concrete transport implementations land in `src/acp/`
//! (Slice 2), `src/tmux/` (Slice 3), and `src/pty/` (when the `pty` Cargo
//! feature is enabled).

pub mod contract;
pub mod quiescence;
pub mod ui;
pub mod vocabulary;

pub use crate::acp::{AcpDriverServices, AcpTransport, AcpWorkerDriver};
#[cfg(feature = "pty")]
pub use crate::pty::{PtyTargetConfiguration, PtyTransport};
pub use crate::tmux::TmuxTransport;
pub use contract::{
    ChoiceMade, ChoiceToMake, Chooser, DeliveryEnvelope, DeliveryMessage, DeliveryWaitError,
    LookMode, OutcomeFuture, OutputView, PromptReadinessTemplate, SingleDeliveryOutcome,
    StartupContext, ThingToChoose, Transport, TransportError, TransportImpl, TransportReadiness,
    TransportStatus,
};
pub use quiescence::{
    EMPTY_PANE_MISMATCH_PREFIX, ReadinessMismatch, WEDGE_CONSECUTIVE_TICKS, WedgeObservation,
    WedgeProbe, mismatch_is_wedge_class, wait_for_quiescent_three_state,
};
pub use ui::{
    UiBroadcastFn, UiBroadcastStatus, UiIncomingMessage, UiOutcomePhase, UiPhaseFn, UiTransport,
    UiTransportServices,
};
pub use vocabulary::{
    DeliveryPayloadMode, LookFreshness, LookSnapshotPayload, LookSnapshotSource, SendOutcome,
    StructuredEntry, ToolCallStatus, WorkerReadinessState,
};
