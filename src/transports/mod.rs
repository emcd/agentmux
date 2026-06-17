//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. Concrete transport implementations land
//! in `src/acp/` (Slice 2) and `src/tmux/` (Slice 3).

pub mod contract;
pub mod vocabulary;

pub use crate::acp::AcpTransport;
pub use crate::tmux::TmuxTransport;
pub use contract::{
    ChoiceMade, ChoiceToMake, Chooser, DeliveryContext, DeliveryEnvelope, DeliveryResult, LookMode,
    LookSnapshotPayload, OutputView, PromptReadinessTemplate, RawWriteResult,
    SingleDeliveryOutcome, StartupContext, ThingToChoose, Transport, TransportError, TransportImpl,
    TransportReadiness, TransportStatus,
};
pub use vocabulary::{AcpLookFreshness, AcpLookSnapshotSource, DeliveryPayloadMode, SendOutcome};
